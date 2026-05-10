//! Rotation + retention + compression for
//! `~/.inspect/history/<ns>-<YYYY-MM-DD>.log`.
//!
//! Three knobs in `~/.inspect/config.toml [history]`:
//!
//! - `retain_days = 90` — delete transcript files older than N days.
//! - `max_total_mb = 500` — cap total bytes across all transcripts;
//!   evict oldest-first when over.
//! - `compress_after_days = 7` — gzip transcript files older than N
//!   days. Compressed files keep their original name with a `.gz`
//!   suffix (`<ns>-<YYYY-MM-DD>.log.gz`) so the date stays parseable.
//!
//! The rotate pass walks `~/.inspect/history/`, parses each filename
//! into `(namespace, date, compressed?)`, applies the three rules in
//! order:
//!
//! 1. Delete files whose date is older than `retain_days`.
//! 2. Gzip files whose date is older than `compress_after_days` and
//!    that aren't already gzipped.
//! 3. Evict oldest files until total bytes ≤ `max_total_mb`.
//!
//! The `inspect history rotate` verb runs this pass explicitly. A
//! lazy trigger fires from `transcript::finalize` gated by a
//! once-per-day marker (`~/.inspect/history/.rotated`) so a busy
//! session doesn't re-scan the directory after every verb.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use chrono::{Duration, NaiveDate, Utc};
use serde::Serialize;

use crate::paths::set_file_mode_0600;

/// Compiled history retention policy. Built from
/// [`crate::config::global::HistoryConfig`] at the start of every
/// rotate pass. Defaults match the spec values.
#[derive(Debug, Clone)]
pub struct HistoryPolicy {
    pub retain_days: u32,
    pub max_total_mb: u32,
    pub compress_after_days: u32,
}

impl Default for HistoryPolicy {
    fn default() -> Self {
        Self {
            retain_days: 90,
            max_total_mb: 500,
            compress_after_days: 7,
        }
    }
}

impl HistoryPolicy {
    pub fn from_config(cfg: &crate::config::global::HistoryConfig) -> Self {
        Self {
            retain_days: cfg.retain_days.unwrap_or(90),
            max_total_mb: cfg.max_total_mb.unwrap_or(500),
            compress_after_days: cfg.compress_after_days.unwrap_or(7),
        }
    }
}

/// Rotation report — mirrored byte-for-byte by the `--json` output
/// of `inspect history rotate`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RotateReport {
    pub deleted_count: usize,
    pub deleted_bytes: u64,
    pub compressed_count: usize,
    pub compressed_bytes_before: u64,
    pub compressed_bytes_after: u64,
    pub evicted_count: usize,
    pub evicted_bytes: u64,
    pub total_bytes_after: u64,
    pub deleted_files: Vec<String>,
    pub compressed_files: Vec<String>,
    pub evicted_files: Vec<String>,
}

/// Run the full rotate pass against `~/.inspect/history/`.
///
/// `policy` overrides the on-disk config. Pass `None` to load from
/// `~/.inspect/config.toml [history]`.
pub fn run_rotate(policy: Option<HistoryPolicy>) -> Result<RotateReport> {
    let policy = match policy {
        Some(p) => p,
        None => HistoryPolicy::from_config(&crate::config::global::load()?.history),
    };
    let dir = crate::transcript::history_dir();
    if !dir.exists() {
        return Ok(RotateReport::default());
    }

    let mut entries = list_transcripts(&dir)?;
    let today = Utc::now().date_naive();

    let mut report = RotateReport::default();

    // Step 1: delete files older than retain_days.
    let retain_cutoff = today - Duration::days(policy.retain_days as i64);
    entries.retain(|e| {
        if e.date < retain_cutoff {
            let len = std::fs::metadata(&e.path).map(|m| m.len()).unwrap_or(0);
            if std::fs::remove_file(&e.path).is_ok() {
                report.deleted_count += 1;
                report.deleted_bytes = report.deleted_bytes.saturating_add(len);
                if let Some(name) = e.path.file_name().and_then(|s| s.to_str()) {
                    report.deleted_files.push(name.to_string());
                }
            }
            false
        } else {
            true
        }
    });

    // Step 2: gzip files older than compress_after_days that are not
    // already compressed. Today's file is never gzipped (we may still
    // be writing to it).
    let compress_cutoff = today - Duration::days(policy.compress_after_days as i64);
    let mut compressed_entries: Vec<TranscriptFile> = Vec::new();
    for mut e in std::mem::take(&mut entries) {
        if e.compressed || e.date >= compress_cutoff {
            compressed_entries.push(e);
            continue;
        }
        match compress_in_place(&e.path) {
            Ok((gz_path, before, after)) => {
                report.compressed_count += 1;
                report.compressed_bytes_before =
                    report.compressed_bytes_before.saturating_add(before);
                report.compressed_bytes_after = report.compressed_bytes_after.saturating_add(after);
                if let Some(name) = gz_path.file_name().and_then(|s| s.to_str()) {
                    report.compressed_files.push(name.to_string());
                }
                e.path = gz_path;
                e.compressed = true;
                compressed_entries.push(e);
            }
            Err(_) => {
                // Best-effort: keep the original on compress failure.
                compressed_entries.push(e);
            }
        }
    }
    entries = compressed_entries;

    // Step 3: enforce max_total_mb cap. Sort oldest-first; evict
    // until we're under the cap (or only today's file remains).
    let cap_bytes: u64 = (policy.max_total_mb as u64).saturating_mul(1024 * 1024);
    let mut total: u64 = entries
        .iter()
        .map(|e| std::fs::metadata(&e.path).map(|m| m.len()).unwrap_or(0))
        .sum();
    if total > cap_bytes {
        entries.sort_by_key(|e| e.date);
        let mut idx = 0usize;
        while total > cap_bytes && idx < entries.len() {
            let e = &entries[idx];
            // Never evict a transcript dated today — it is the
            // currently-active one.
            if e.date == today {
                idx += 1;
                continue;
            }
            let len = std::fs::metadata(&e.path).map(|m| m.len()).unwrap_or(0);
            if std::fs::remove_file(&e.path).is_ok() {
                report.evicted_count += 1;
                report.evicted_bytes = report.evicted_bytes.saturating_add(len);
                if let Some(name) = e.path.file_name().and_then(|s| s.to_str()) {
                    report.evicted_files.push(name.to_string());
                }
                total = total.saturating_sub(len);
            }
            idx += 1;
        }
    }
    report.total_bytes_after = total;

    // Touch the lazy marker so subsequent finalizes don't re-scan
    // until tomorrow.
    let _ = touch_lazy_marker();

    Ok(report)
}

/// Best-effort lazy rotation called from `transcript::finalize`.
/// Gated by a once-per-day marker so a busy session does not pay the
/// full FS scan cost on every verb. Errors are deliberately
/// swallowed by the caller.
pub fn maybe_run_lazy() -> Result<Option<RotateReport>> {
    if !cheap_path_should_check()? {
        return Ok(None);
    }
    touch_lazy_marker()?;
    Ok(Some(run_rotate(None)?))
}

fn cheap_path_should_check() -> Result<bool> {
    let marker = marker_path();
    let Ok(meta) = std::fs::metadata(&marker) else {
        return Ok(true);
    };
    let Ok(modified) = meta.modified() else {
        return Ok(true);
    };
    let elapsed = SystemTime::now()
        .duration_since(modified)
        .unwrap_or(std::time::Duration::ZERO);
    // 23 hours: enough to fire daily even with mild clock skew, not so
    // tight that a midnight-boundary verb fires twice.
    Ok(elapsed.as_secs() >= 23 * 3600)
}

fn touch_lazy_marker() -> Result<()> {
    let marker = marker_path();
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&marker, b"").with_context(|| format!("touching {}", marker.display()))?;
    let _ = set_file_mode_0600(&marker);
    Ok(())
}

fn marker_path() -> PathBuf {
    crate::transcript::history_dir().join(".rotated")
}

#[derive(Debug, Clone)]
pub(crate) struct TranscriptFile {
    pub path: PathBuf,
    pub namespace: String,
    pub date: NaiveDate,
    pub compressed: bool,
}

/// List every transcript file under `dir` with a parseable
/// `<ns>-<YYYY-MM-DD>.log[.gz]` filename. Files that don't match
/// the naming convention are silently skipped — never deleted.
pub(crate) fn list_transcripts(dir: &Path) -> Result<Vec<TranscriptFile>> {
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    for ent in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let ent = ent?;
        let path = ent.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if let Some(parsed) = parse_transcript_name(name) {
            out.push(TranscriptFile {
                path,
                namespace: parsed.0,
                date: parsed.1,
                compressed: parsed.2,
            });
        }
    }
    Ok(out)
}

/// Parse `<ns>-<YYYY-MM-DD>.log` or `<ns>-<YYYY-MM-DD>.log.gz` into
/// `(ns, date, compressed)`. Returns `None` for any other shape.
pub(crate) fn parse_transcript_name(name: &str) -> Option<(String, NaiveDate, bool)> {
    let (stem, compressed) = if let Some(s) = name.strip_suffix(".log.gz") {
        (s, true)
    } else if let Some(s) = name.strip_suffix(".log") {
        (s, false)
    } else {
        return None;
    };
    // Stem looks like `<ns>-YYYY-MM-DD`. Date is the trailing 10
    // chars; everything before the leading `-` is the namespace.
    if stem.len() < 12 {
        return None;
    }
    let (ns_part, date_part) = stem.split_at(stem.len() - 10);
    let ns = ns_part.strip_suffix('-')?.to_string();
    let date = NaiveDate::parse_from_str(date_part, "%Y-%m-%d").ok()?;
    if ns.is_empty() {
        return None;
    }
    Some((ns, date, compressed))
}

fn compress_in_place(path: &Path) -> Result<(PathBuf, u64, u64)> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("reading {} for compression", path.display()))?;
    let before = bytes.len() as u64;
    let gz_path = {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("transcript.log");
        parent.join(format!("{name}.gz"))
    };
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&bytes)
        .with_context(|| format!("gzip-encoding {}", path.display()))?;
    let compressed = encoder.finish().context("finalizing gzip stream")?;
    let after = compressed.len() as u64;

    // Write atomically via a `.part` sibling so a crash mid-encode
    // doesn't leave a half-written `.gz` next to the original.
    let tmp = {
        let parent = gz_path.parent().unwrap_or_else(|| Path::new("."));
        let name = gz_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("compressed.gz");
        parent.join(format!(".{name}.part"))
    };
    std::fs::write(&tmp, &compressed).with_context(|| format!("writing {}", tmp.display()))?;
    let _ = set_file_mode_0600(&tmp);
    std::fs::rename(&tmp, &gz_path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), gz_path.display()))?;
    let _ = set_file_mode_0600(&gz_path);
    std::fs::remove_file(path).with_context(|| format!("removing original {}", path.display()))?;
    Ok((gz_path, before, after))
}

/// Helper for `inspect history show` / `inspect history list`. Reads
/// a transcript file, transparently decompressing if it is `.log.gz`.
pub fn read_transcript(path: &Path) -> Result<Vec<u8>> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    if path.extension().and_then(|s| s.to_str()) == Some("gz") {
        let mut dec = flate2::read::GzDecoder::new(bytes.as_slice());
        let mut out = Vec::with_capacity(bytes.len() * 4);
        dec.read_to_end(&mut out)
            .with_context(|| format!("gunzipping {}", path.display()))?;
        Ok(out)
    } else {
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_name_log() {
        let p = parse_transcript_name("arte-2026-04-28.log").unwrap();
        assert_eq!(p.0, "arte");
        assert_eq!(p.1, NaiveDate::from_ymd_opt(2026, 4, 28).unwrap());
        assert!(!p.2);
    }

    #[test]
    fn parse_name_log_gz() {
        let p = parse_transcript_name("arte-2026-04-28.log.gz").unwrap();
        assert_eq!(p.0, "arte");
        assert!(p.2);
    }

    #[test]
    fn parse_name_with_hyphenated_namespace() {
        let p = parse_transcript_name("arte-prod-east-2026-04-28.log").unwrap();
        assert_eq!(p.0, "arte-prod-east");
        assert_eq!(p.1, NaiveDate::from_ymd_opt(2026, 4, 28).unwrap());
        assert!(!p.2);
    }

    #[test]
    fn parse_name_rejects_other_shapes() {
        assert!(parse_transcript_name("README.md").is_none());
        assert!(parse_transcript_name(".gc-checked").is_none());
        assert!(parse_transcript_name("arte-not-a-date.log").is_none());
        assert!(parse_transcript_name("-2026-04-28.log").is_none());
    }

    #[test]
    fn policy_defaults_match_spec() {
        let p = HistoryPolicy::default();
        assert_eq!(p.retain_days, 90);
        assert_eq!(p.max_total_mb, 500);
        assert_eq!(p.compress_after_days, 7);
    }
}
