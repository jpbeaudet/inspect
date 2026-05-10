//! Audit-log retention + snapshot orphan GC.
//!
//! Driven by `inspect audit gc` and (when `[audit] retention` is set in
//! `~/.inspect/config.toml`) by a cheap-path lazy trigger that fires on
//! every `AuditStore::append` so that long-running installations never
//! see `~/.inspect/audit/` grow unbounded.
//!
//! The GC walks every entry across every JSONL file under the audit
//! directory, decides which entries to keep based on the
//! [`RetentionPolicy`], rewrites the affected JSONL files (atomic
//! tmp→rename), and finally sweeps `~/.inspect/audit/snapshots/` for
//! `sha256-<hex>` files no longer referenced by any retained entry.
//! Snapshots referenced by a retained entry are **never** deleted —
//! that's the revert contract.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{anyhow, Context, Result};
use chrono::{Duration, Utc};
use serde::Serialize;

use crate::paths::{audit_dir, set_file_mode_0600, snapshots_dir};
use crate::safety::audit::{AuditEntry, RevertKind};

/// `inspect audit gc --keep <X>` operand. Constructed by
/// [`parse_retention`] from the operator's CLI input or from the
/// `[audit] retention` config value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetentionPolicy {
    /// Keep entries whose timestamp is within `now - duration`.
    Duration(Duration),
    /// Keep the newest `n` entries per namespace (extracted from the
    /// `selector` field; entries with no namespace prefix group under
    /// the sentinel `_`).
    Count(usize),
}

/// Parse a `--keep` value. Accepted shapes:
///
/// - `90d`, `4w`, `12h`, `15m` — duration suffixes (`d`/`w`/`h`/`m`).
/// - `100` — bare positive integer = entry count per namespace.
///
/// Anything else returns a clear error pointing at the supported forms.
pub fn parse_retention(raw: &str) -> Result<RetentionPolicy> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(anyhow!(
            "--keep value is empty; expected '<N>' (entries per namespace) or '<N>d'/'<N>w'/'<N>h'/'<N>m' (duration)"
        ));
    }
    // Bare integer → entry count.
    if s.chars().all(|c| c.is_ascii_digit()) {
        let n: usize = s
            .parse()
            .with_context(|| format!("--keep '{s}': not a valid count"))?;
        if n == 0 {
            return Err(anyhow!(
                "--keep 0 would delete every entry; refusing. Use 'inspect audit gc --keep 1' to keep just the newest, or pass an explicit duration"
            ));
        }
        return Ok(RetentionPolicy::Count(n));
    }
    let (digits, unit) = s.split_at(s.len().saturating_sub(1));
    let n: i64 = digits
        .parse()
        .with_context(|| format!("--keep '{s}': leading digits not parseable"))?;
    if n <= 0 {
        return Err(anyhow!(
            "--keep '{s}': duration must be positive (got {n}{unit})"
        ));
    }
    let dur = match unit {
        "d" => Duration::days(n),
        "w" => Duration::weeks(n),
        "h" => Duration::hours(n),
        "m" => Duration::minutes(n),
        other => {
            return Err(anyhow!(
                "--keep '{s}': unknown unit '{other}'; supported: d (days), w (weeks), h (hours), m (minutes)"
            ));
        }
    };
    Ok(RetentionPolicy::Duration(dur))
}

/// Deletion summary returned by [`run_gc`]. Stable shape — mirrored
/// byte-for-byte by the `--json` output of `inspect audit gc`.
#[derive(Debug, Clone, Serialize, Default)]
pub struct GcReport {
    pub dry_run: bool,
    pub policy: String,
    pub entries_total: usize,
    pub entries_kept: usize,
    pub deleted_entries: usize,
    pub deleted_snapshots: usize,
    pub freed_bytes: u64,
    pub deleted_ids: Vec<String>,
    pub deleted_snapshot_hashes: Vec<String>,
}

/// Run the retention pass. With `dry_run=true`, walks the audit log
/// and computes what *would* be deleted but never modifies the
/// filesystem.
pub fn run_gc(policy: &RetentionPolicy, dry_run: bool) -> Result<GcReport> {
    let dir = audit_dir();
    let files = jsonl_files(&dir)?;

    // Read every entry across every file, tagged by source path so we
    // know which file to rewrite when we delete an entry.
    let mut tagged: Vec<(PathBuf, AuditEntry)> = Vec::new();
    for f in &files {
        for e in read_jsonl_entries(f)? {
            tagged.push((f.clone(), e));
        }
    }
    let entries_total = tagged.len();

    let delete_set: BTreeSet<String> = match policy {
        RetentionPolicy::Duration(d) => {
            let cutoff = Utc::now() - *d;
            tagged
                .iter()
                .filter(|(_, e)| e.ts < cutoff)
                .map(|(_, e)| e.id.clone())
                .collect()
        }
        RetentionPolicy::Count(n) => {
            // Group by namespace (extracted from selector). Within each
            // group, keep the newest `n`; the rest are deleted.
            let mut by_ns: BTreeMap<String, Vec<&AuditEntry>> = BTreeMap::new();
            for (_, e) in &tagged {
                by_ns
                    .entry(namespace_of(&e.selector).into())
                    .or_default()
                    .push(e);
            }
            let mut out = BTreeSet::new();
            for (_, mut group) in by_ns {
                group.sort_by_key(|e| std::cmp::Reverse(e.ts));
                if group.len() > *n {
                    for e in &group[*n..] {
                        out.insert(e.id.clone());
                    }
                }
            }
            out
        }
    };

    // Snapshot hashes referenced by every entry that survives. A
    // snapshot file under `~/.inspect/audit/snapshots/sha256-<hex>` is
    // an orphan when its hash is in no kept entry.
    let mut kept_hashes: BTreeSet<String> = BTreeSet::new();
    for (_, e) in &tagged {
        if delete_set.contains(&e.id) {
            continue;
        }
        collect_snapshot_hashes(e, &mut kept_hashes);
    }

    // Compute orphans by walking the snapshot dir.
    let snaps_dir = snapshots_dir();
    let mut orphan_files: Vec<(PathBuf, u64, String)> = Vec::new();
    if snaps_dir.exists() {
        for ent in std::fs::read_dir(&snaps_dir)
            .with_context(|| format!("reading {}", snaps_dir.display()))?
        {
            let ent = ent?;
            let path = ent.path();
            if !path.is_file() {
                continue;
            }
            let hex_owned = {
                let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                // Ignore `.part` temp files and anything not a snapshot.
                let Some(hex) = name.strip_prefix("sha256-") else {
                    continue;
                };
                if hex.contains('.') {
                    continue;
                }
                if kept_hashes.contains(hex) {
                    continue;
                }
                hex.to_string()
            };
            let len = ent.metadata().map(|m| m.len()).unwrap_or(0);
            orphan_files.push((path, len, hex_owned));
        }
    }

    let mut report = GcReport {
        dry_run,
        policy: format_policy(policy),
        entries_total,
        entries_kept: entries_total - delete_set.len(),
        deleted_entries: delete_set.len(),
        deleted_snapshots: orphan_files.len(),
        freed_bytes: orphan_files.iter().map(|(_, len, _)| *len).sum(),
        deleted_ids: delete_set.iter().cloned().collect(),
        deleted_snapshot_hashes: orphan_files.iter().map(|(_, _, h)| h.clone()).collect(),
    };

    if dry_run {
        return Ok(report);
    }

    // Mutating phase. Rewrite each affected JSONL file once, dropping
    // the deleted entries; preserve original file order for the rest.
    // Tally bytes freed by JSONL rewrites and add to freed_bytes so the
    // report's freed_bytes covers both snapshot files AND audit log
    // shrinkage.
    let mut by_file: BTreeMap<PathBuf, Vec<AuditEntry>> = BTreeMap::new();
    for (f, e) in tagged {
        by_file.entry(f).or_default().push(e);
    }
    let mut audit_log_freed: u64 = 0;
    for (path, entries) in by_file {
        let any_deleted = entries.iter().any(|e| delete_set.contains(&e.id));
        if !any_deleted {
            continue;
        }
        let kept: Vec<AuditEntry> = entries
            .into_iter()
            .filter(|e| !delete_set.contains(&e.id))
            .collect();
        let before_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if kept.is_empty() {
            std::fs::remove_file(&path)
                .with_context(|| format!("removing emptied audit file {}", path.display()))?;
            audit_log_freed = audit_log_freed.saturating_add(before_bytes);
        } else {
            rewrite_jsonl_atomic(&path, &kept)
                .with_context(|| format!("rewriting {}", path.display()))?;
            let after_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            audit_log_freed =
                audit_log_freed.saturating_add(before_bytes.saturating_sub(after_bytes));
        }
    }
    report.freed_bytes = report.freed_bytes.saturating_add(audit_log_freed);

    for (path, _, _) in orphan_files {
        std::fs::remove_file(&path)
            .with_context(|| format!("removing orphan snapshot {}", path.display()))?;
    }

    // Touch the lazy-trigger marker so subsequent appends within the
    // same minute don't re-scan after a fresh manual gc.
    let _ = touch_lazy_marker();

    Ok(report)
}

/// Best-effort lazy retention check called from
/// `AuditStore::append`. Reads `~/.inspect/config.toml` to discover
/// `[audit] retention`; returns immediately if unset. Otherwise checks
/// whether the oldest JSONL file's mtime is older than the retention
/// threshold and, if so, runs a full GC pass. Cheap-path guard via a
/// `~/.inspect/audit/.gc-checked` marker file: if its mtime is within
/// the last 60 seconds, skip entirely.
///
/// Returns `Ok(None)` for the no-op cheap path, `Ok(Some(report))`
/// when GC actually ran. Errors are deliberately swallowed by the
/// caller — we never let GC failure break an audit append.
pub fn maybe_run_lazy_gc() -> Result<Option<GcReport>> {
    let policy = match crate::config::global::load()?.audit.retention {
        Some(raw) => parse_retention(&raw)
            .with_context(|| format!("parsing config [audit] retention = '{raw}'"))?,
        None => return Ok(None),
    };

    if !cheap_path_should_check()? {
        return Ok(None);
    }
    // Update the marker first — even if the rotation pass hits a
    // transient error, we don't want to retry every audit append for
    // the next minute.
    touch_lazy_marker()?;

    let dir = audit_dir();
    let files = jsonl_files(&dir)?;
    let oldest_mtime = files
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .filter_map(|m| m.modified().ok())
        .min();
    let Some(oldest) = oldest_mtime else {
        return Ok(None);
    };
    let cutoff_age = match &policy {
        RetentionPolicy::Duration(d) => *d,
        RetentionPolicy::Count(_) => {
            // For count-based policies we can't pre-decide from mtime
            // alone; run a full pass once per check window.
            return Ok(Some(run_gc(&policy, false)?));
        }
    };
    let now = SystemTime::now();
    let oldest_age = now
        .duration_since(oldest)
        .unwrap_or(std::time::Duration::ZERO);
    let cutoff_secs = cutoff_age.num_seconds().max(0) as u64;
    if oldest_age.as_secs() <= cutoff_secs {
        return Ok(None);
    }
    Ok(Some(run_gc(&policy, false)?))
}

/// Cheap-path guard: returns `true` if the marker is missing or older
/// than 60s. Doesn't update the marker — call [`touch_lazy_marker`]
/// separately when actually proceeding.
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
    Ok(elapsed.as_secs() >= 60)
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
    audit_dir().join(".gc-checked")
}

fn jsonl_files(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut out = Vec::new();
    for ent in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let ent = ent?;
        let path = ent.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

fn read_jsonl_entries(path: &Path) -> Result<Vec<AuditEntry>> {
    use std::io::{BufRead, BufReader};
    let f = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let r = BufReader::new(f);
    let mut out = Vec::new();
    for line in r.lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(e) = serde_json::from_str::<AuditEntry>(&line) {
            out.push(e);
        }
    }
    Ok(out)
}

fn rewrite_jsonl_atomic(path: &Path, entries: &[AuditEntry]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).ok();
    let tmp = parent.join(format!(
        ".{}.gctmp.{}",
        path.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "audit.jsonl".into()),
        std::process::id()
    ));
    use std::io::Write as _;
    let mut buf = String::with_capacity(entries.len() * 256);
    for e in entries {
        let line = serde_json::to_string(e).context("serializing audit entry")?;
        buf.push_str(&line);
        buf.push('\n');
    }
    {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .mode(0o600)
                .open(&tmp)
                .with_context(|| format!("creating {}", tmp.display()))?;
            f.write_all(buf.as_bytes())
                .with_context(|| format!("writing {}", tmp.display()))?;
            f.sync_all().ok();
        }
        #[cfg(not(unix))]
        {
            std::fs::write(&tmp, buf.as_bytes())
                .with_context(|| format!("writing {}", tmp.display()))?;
        }
    }
    std::fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    let _ = set_file_mode_0600(path);
    Ok(())
}

/// Extract the namespace token from a selector. Selectors are
/// formatted as `<ns>`, `<ns>/<svc>`, `<ns>/<svc>:<path>`, or `*` /
/// empty for global verbs. Anything that doesn't begin with a
/// non-empty `<ns>` token groups under the sentinel `_`.
fn namespace_of(selector: &str) -> &str {
    let s = selector.trim();
    if s.is_empty() {
        return "_";
    }
    let ns = match s.find('/') {
        Some(i) => &s[..i],
        None => s,
    };
    if ns.is_empty() || ns == "*" {
        "_"
    } else {
        ns
    }
}

/// Walk every snapshot reference on an audit entry — `previous_hash`,
/// `new_hash`, `snapshot` (path), and the `revert.payload` for
/// `state_snapshot` and `composite` reverts. Composite payloads carry
/// nested `state_snapshot` records that pin further hashes.
fn collect_snapshot_hashes(e: &AuditEntry, out: &mut BTreeSet<String>) {
    if let Some(h) = &e.previous_hash {
        out.insert(strip_sha256_prefix(h).to_string());
    }
    if let Some(h) = &e.new_hash {
        out.insert(strip_sha256_prefix(h).to_string());
    }
    if let Some(p) = &e.snapshot {
        if let Some(name) = Path::new(p).file_name().and_then(|s| s.to_str()) {
            if let Some(hex) = name.strip_prefix("sha256-") {
                out.insert(hex.to_string());
            }
        }
    }
    if let Some(rev) = &e.revert {
        match rev.kind {
            RevertKind::StateSnapshot => {
                out.insert(strip_sha256_prefix(&rev.payload).to_string());
            }
            RevertKind::Composite => collect_composite_hashes(&rev.payload, out),
            RevertKind::CommandPair | RevertKind::Unsupported => {}
        }
    }
}

/// Composite revert payloads are JSON arrays of
/// `{kind, payload, ...}` records. Recurse so nested
/// `state_snapshot` payloads (and theoretically nested `composite`
/// payloads) keep their hashes pinned. Best-effort: malformed JSON is
/// ignored — the GC must never delete a snapshot it can't prove is
/// orphaned.
fn collect_composite_hashes(payload: &str, out: &mut BTreeSet<String>) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return;
    };
    let Some(arr) = value.as_array() else {
        return;
    };
    for v in arr {
        let kind = v.get("kind").and_then(|k| k.as_str()).unwrap_or("");
        let inner_payload = v
            .get("payload")
            .and_then(|p| p.as_str())
            .unwrap_or_default();
        match kind {
            "state_snapshot" => {
                out.insert(strip_sha256_prefix(inner_payload).to_string());
            }
            "composite" => collect_composite_hashes(inner_payload, out),
            _ => {}
        }
    }
}

fn strip_sha256_prefix(h: &str) -> &str {
    h.strip_prefix("sha256:")
        .or_else(|| h.strip_prefix("sha256-"))
        .unwrap_or(h)
}

fn format_policy(p: &RetentionPolicy) -> String {
    match p {
        RetentionPolicy::Duration(d) => {
            let total = d.num_seconds();
            if total % (3600 * 24 * 7) == 0 {
                format!("{}w", total / (3600 * 24 * 7))
            } else if total % (3600 * 24) == 0 {
                format!("{}d", total / (3600 * 24))
            } else if total % 3600 == 0 {
                format!("{}h", total / 3600)
            } else {
                format!("{}m", total / 60)
            }
        }
        RetentionPolicy::Count(n) => n.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_retention_durations() {
        assert_eq!(
            parse_retention("90d").unwrap(),
            RetentionPolicy::Duration(Duration::days(90))
        );
        assert_eq!(
            parse_retention("4w").unwrap(),
            RetentionPolicy::Duration(Duration::weeks(4))
        );
        assert_eq!(
            parse_retention("12h").unwrap(),
            RetentionPolicy::Duration(Duration::hours(12))
        );
        assert_eq!(
            parse_retention("15m").unwrap(),
            RetentionPolicy::Duration(Duration::minutes(15))
        );
    }

    #[test]
    fn parse_retention_count() {
        assert_eq!(parse_retention("100").unwrap(), RetentionPolicy::Count(100));
        assert_eq!(parse_retention("1").unwrap(), RetentionPolicy::Count(1));
    }

    #[test]
    fn parse_retention_rejects_zero() {
        assert!(parse_retention("0").is_err());
    }

    #[test]
    fn parse_retention_rejects_unknown_unit() {
        let err = parse_retention("5y").unwrap_err().to_string();
        assert!(err.contains("unknown unit"), "{err}");
    }

    #[test]
    fn parse_retention_rejects_empty() {
        assert!(parse_retention("").is_err());
        assert!(parse_retention("   ").is_err());
    }

    #[test]
    fn parse_retention_rejects_negative() {
        assert!(parse_retention("-1d").is_err());
    }

    #[test]
    fn namespace_of_selectors() {
        assert_eq!(namespace_of("arte"), "arte");
        assert_eq!(namespace_of("arte/atlas-vault"), "arte");
        assert_eq!(namespace_of("arte/atlas-vault:/etc/foo"), "arte");
        assert_eq!(namespace_of(""), "_");
        assert_eq!(namespace_of("*"), "_");
        assert_eq!(namespace_of("/leading-slash"), "_");
    }

    #[test]
    fn format_policy_pretty() {
        assert_eq!(
            format_policy(&RetentionPolicy::Duration(Duration::days(90))),
            "90d"
        );
        assert_eq!(
            format_policy(&RetentionPolicy::Duration(Duration::weeks(4))),
            "4w"
        );
        assert_eq!(
            format_policy(&RetentionPolicy::Duration(Duration::hours(12))),
            "12h"
        );
        assert_eq!(
            format_policy(&RetentionPolicy::Duration(Duration::minutes(15))),
            "15m"
        );
        assert_eq!(format_policy(&RetentionPolicy::Count(100)), "100");
    }

    #[test]
    fn collect_composite_extracts_nested_state_snapshot_hashes() {
        let payload = serde_json::json!([
            { "kind": "state_snapshot", "payload": "deadbeef", "preview": "" },
            { "kind": "command_pair", "payload": "rm /tmp/x", "preview": "" },
            { "kind": "state_snapshot", "payload": "sha256:cafebabe", "preview": "" },
        ])
        .to_string();
        let mut out = BTreeSet::new();
        collect_composite_hashes(&payload, &mut out);
        assert!(out.contains("deadbeef"));
        assert!(out.contains("cafebabe"));
        assert_eq!(out.len(), 2);
    }
}
