//! `--since-last` cursor (P10, v0.1.1).
//!
//! Each `inspect logs --since-last` / `inspect grep --since-last` invocation
//! reads a tiny per-(namespace, service) state file under
//! `~/.inspect/cursors/<ns>/<service>.kv`, uses the recorded `last_call`
//! unix timestamp as the effective `--since`, and rewrites the file with
//! the start time of the current run.
//!
//! Field-pitfall driver: P10 in [INSPECT_v0.1.1_PATCH_SPEC.md]. Operators
//! iterating on a service typed `--since 5m` over and over and lost the
//! exact resume point between calls (especially across long debug
//! sessions where 5 minutes was sometimes too much, sometimes not enough).
//!
//! Format: 4 keys, one per line, `key=value`. We never persist user
//! credentials or selector text, only timestamps and the namespace/service
//! identifiers needed to debug stale state.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};

use crate::paths::{cursor_file, cursors_dir, set_dir_mode_0700, set_file_mode_0600};

#[derive(Debug, Clone)]
pub struct Cursor {
    pub ns: String,
    pub service: String,
    /// Unix timestamp of the previous successful run's start.
    pub last_call: u64,
    /// Unix timestamp of the most recent log line we observed (0 when
    /// unknown — we currently rely on `last_call` for `--since`, but
    /// keep this field so future P10 iterations can switch to true
    /// line-timestamp tracking without a format break).
    pub last_ts: u64,
}

impl Cursor {
    pub fn now(ns: &str, service: &str) -> Self {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            ns: ns.to_string(),
            service: service.to_string(),
            last_call: n,
            last_ts: 0,
        }
    }

    pub fn load(ns: &str, service: &str) -> Result<Option<Self>> {
        let p = cursor_file(ns, service);
        if !p.exists() {
            return Ok(None);
        }
        let body = fs::read_to_string(&p)
            .with_context(|| format!("read cursor {}", p.display()))?;
        let mut last_call: u64 = 0;
        let mut last_ts: u64 = 0;
        let mut file_ns = String::new();
        let mut file_svc = String::new();
        for line in body.lines() {
            let (k, v) = match line.split_once('=') {
                Some(kv) => kv,
                None => continue,
            };
            match k.trim() {
                "ns" => file_ns = v.trim().to_string(),
                "service" => file_svc = v.trim().to_string(),
                "last_call" => last_call = v.trim().parse().unwrap_or(0),
                "last_ts" => last_ts = v.trim().parse().unwrap_or(0),
                _ => {}
            }
        }
        if file_ns.is_empty() {
            file_ns = ns.to_string();
        }
        if file_svc.is_empty() {
            file_svc = service.to_string();
        }
        Ok(Some(Self {
            ns: file_ns,
            service: file_svc,
            last_call,
            last_ts,
        }))
    }

    pub fn save(&self) -> Result<()> {
        let p = cursor_file(&self.ns, &self.service);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("mkdir -p {}", parent.display()))?;
            // Best effort; cursors_dir() and the per-namespace dir
            // both want 0700.
            let _ = set_dir_mode_0700(&cursors_dir());
            let _ = set_dir_mode_0700(parent);
        }
        let body = format!(
            "ns={}\nservice={}\nlast_call={}\nlast_ts={}\n",
            self.ns, self.service, self.last_call, self.last_ts
        );
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&p)
            .with_context(|| format!("open cursor {}", p.display()))?;
        f.write_all(body.as_bytes())
            .with_context(|| format!("write cursor {}", p.display()))?;
        drop(f);
        set_file_mode_0600(&p).map_err(|e| anyhow!("chmod 0600 cursor: {e}"))?;
        Ok(())
    }
}

/// `--reset-cursor`: delete the file (idempotent).
pub fn reset(ns: &str, service: &str) -> Result<bool> {
    let p: PathBuf = cursor_file(ns, service);
    if p.exists() {
        fs::remove_file(&p).with_context(|| format!("rm {}", p.display()))?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Default fallback when `--since-last` is asked for but no cursor
/// exists yet. Honors `INSPECT_SINCE_LAST_DEFAULT`, else "5m".
pub fn default_since() -> String {
    std::env::var("INSPECT_SINCE_LAST_DEFAULT").unwrap_or_else(|_| "5m".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_load_save() {
        // Use a sandboxed INSPECT_HOME so we don't pollute the user's.
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("INSPECT_HOME", tmp.path());

        let mut c = Cursor::now("arte", "pulse");
        c.last_call = 1_700_000_000;
        c.last_ts = 1_700_000_042;
        c.save().unwrap();

        let loaded = Cursor::load("arte", "pulse").unwrap().unwrap();
        assert_eq!(loaded.ns, "arte");
        assert_eq!(loaded.service, "pulse");
        assert_eq!(loaded.last_call, 1_700_000_000);
        assert_eq!(loaded.last_ts, 1_700_000_042);
    }

    #[test]
    fn reset_removes_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("INSPECT_HOME", tmp.path());

        let c = Cursor::now("arte", "atlas");
        c.save().unwrap();
        assert!(reset("arte", "atlas").unwrap());
        assert!(!reset("arte", "atlas").unwrap());
        assert!(Cursor::load("arte", "atlas").unwrap().is_none());
    }
}
