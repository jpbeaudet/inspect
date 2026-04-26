//! Audit log: append-only, per-month JSONL files under
//! `~/.inspect/audit/<YYYY-MM>-<user>.jsonl` (mode 0600).
//!
//! Schema mirrors bible §8.2.

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::paths::{audit_dir, ensure_home, set_dir_mode_0700, set_file_mode_0600};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub schema_version: u32,
    pub id: String, // ULID-ish: <ts-millis>-<rand4>
    pub ts: DateTime<Utc>,
    pub user: String,
    pub host: String,
    pub verb: String,
    pub selector: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub args: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub diff_summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    pub exit: i32,
    pub duration_ms: u64,
    /// `true` if this entry is itself a revert.
    #[serde(default)]
    pub is_revert: bool,
    /// Optional reference to the audit id this revert restored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverts: Option<String>,
}

impl AuditEntry {
    pub fn new(verb: &str, selector: &str) -> Self {
        let ts = Utc::now();
        let id = format!(
            "{}-{:04x}",
            ts.timestamp_millis(),
            (rand_u32() & 0xffff)
        );
        Self {
            schema_version: 1,
            id,
            ts,
            user: whoami().unwrap_or_else(|| "unknown".into()),
            host: hostname().unwrap_or_else(|| "unknown".into()),
            verb: verb.to_string(),
            selector: selector.to_string(),
            args: String::new(),
            diff_summary: String::new(),
            previous_hash: None,
            new_hash: None,
            snapshot: None,
            exit: 0,
            duration_ms: 0,
            is_revert: false,
            reverts: None,
        }
    }
}

pub struct AuditStore {
    dir: PathBuf,
}

impl AuditStore {
    pub fn open() -> Result<Self> {
        let _ = ensure_home();
        let dir = audit_dir();
        if !dir.exists() {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("creating {}", dir.display()))?;
        }
        let _ = set_dir_mode_0700(&dir);
        Ok(Self { dir })
    }

    fn current_path(&self) -> PathBuf {
        let now = Utc::now();
        let user = whoami().unwrap_or_else(|| "unknown".into());
        self.dir
            .join(format!("{}-{user}.jsonl", now.format("%Y-%m")))
    }

    pub fn append(&self, entry: &AuditEntry) -> Result<()> {
        let path = self.current_path();
        let line = serde_json::to_string(entry)?;
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        writeln!(f, "{line}").context("writing audit entry")?;
        let _ = set_file_mode_0600(&path);
        Ok(())
    }

    /// Iterate entries newest-last (file order). Returns all months merged.
    pub fn all(&self) -> Result<Vec<AuditEntry>> {
        let mut files: Vec<PathBuf> = match std::fs::read_dir(&self.dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && p.extension().and_then(|s| s.to_str()) == Some("jsonl")
                })
                .collect(),
            Err(_) => return Ok(vec![]),
        };
        files.sort();
        let mut out = Vec::new();
        for f in files {
            let h = std::fs::File::open(&f)?;
            let r = BufReader::new(h);
            for line in r.lines().map_while(Result::ok) {
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(e) = serde_json::from_str::<AuditEntry>(&line) {
                    out.push(e);
                }
            }
        }
        Ok(out)
    }

    pub fn find(&self, id_prefix: &str) -> Result<Option<AuditEntry>> {
        Ok(self
            .all()?
            .into_iter()
            .find(|e| e.id.starts_with(id_prefix)))
    }
}

fn whoami() -> Option<String> {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .ok()
        .filter(|s| !s.is_empty())
}

fn hostname() -> Option<String> {
    if let Ok(h) = std::env::var("HOSTNAME") {
        if !h.is_empty() {
            return Some(h);
        }
    }
    let out = std::process::Command::new("hostname").output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Tiny entropy source — we don't need crypto-grade for an audit id, just
/// uniqueness within a millisecond. Mixing pid + nanos avoids pulling in
/// `rand` for one call site.
fn rand_u32() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    nanos.wrapping_mul(2654435761).wrapping_add(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_append_and_read() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("INSPECT_HOME", tmp.path());
        let s = AuditStore::open().unwrap();
        let mut e = AuditEntry::new("edit", "arte/atlas:/etc/atlas.conf");
        e.diff_summary = "1 file, +1 -1".into();
        s.append(&e).unwrap();
        let all = s.all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].verb, "edit");
        assert_eq!(all[0].selector, "arte/atlas:/etc/atlas.conf");
    }
}
