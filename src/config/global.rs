//! L5 (v0.1.3): global behavior config at `~/.inspect/config.toml`.
//!
//! Distinct from `servers.toml` (per-namespace runtime config); this
//! file holds cross-cutting policy that is *not* keyed on a server —
//! today just the audit retention policy, but reserved for future
//! global toggles (cache TTLs, history rotation, etc.) without
//! polluting the per-server schema.
//!
//! Layout:
//!
//! ```toml
//! [audit]
//! retention = "90d"   # or "100" for entry count; unset = no automatic GC
//! ```
//!
//! The file is optional. A missing file (or a file with only a subset
//! of tables present) yields an [`GlobalConfig`] with all-defaults.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths::inspect_home;

/// Top-level shape of `~/.inspect/config.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GlobalConfig {
    #[serde(default)]
    pub audit: AuditConfig,
    #[serde(default)]
    pub history: HistoryConfig,
}

/// `[audit]` table.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuditConfig {
    /// Retention policy expressed as the same string the CLI accepts:
    /// `90d` / `4w` / `12h` / `15m` / `100`. `None` (table absent or
    /// key absent) disables automatic GC; `inspect audit gc` still
    /// works manually.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention: Option<String>,
}

/// `[history]` table — F18 (v0.1.3) transcript retention policy.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HistoryConfig {
    /// Days of transcripts to keep. Older files are deleted on every
    /// `inspect history rotate` (or lazy fire from `transcript::finalize`).
    /// `None` ⇒ default 90.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retain_days: Option<u32>,
    /// Cap (megabytes) on total transcript bytes across all
    /// namespaces. When over, oldest files are evicted first
    /// (today's file is never evicted while it's the active write
    /// target). `None` ⇒ default 500.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_total_mb: Option<u32>,
    /// Days after which a transcript is gzipped on rotate.
    /// `<ns>-<YYYY-MM-DD>.log` becomes `<ns>-<YYYY-MM-DD>.log.gz`.
    /// `None` ⇒ default 7.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compress_after_days: Option<u32>,
}

/// Path to the global config file (`~/.inspect/config.toml`).
pub fn config_toml() -> PathBuf {
    inspect_home().join("config.toml")
}

/// Load `~/.inspect/config.toml`. Returns the all-defaults
/// [`GlobalConfig`] if the file is missing — that is the common case
/// and not an error. A malformed file is reported with the file path
/// and parse error so the operator can fix it.
pub fn load() -> Result<GlobalConfig> {
    let path = config_toml();
    if !path.exists() {
        return Ok(GlobalConfig::default());
    }
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let parsed: GlobalConfig =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_defaults() {
        let _g = crate::paths::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("INSPECT_HOME", tmp.path());
        let cfg = load().unwrap();
        assert!(cfg.audit.retention.is_none());
    }

    #[test]
    fn parses_audit_retention() {
        let _g = crate::paths::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("INSPECT_HOME", tmp.path());
        std::fs::write(
            tmp.path().join("config.toml"),
            "[audit]\nretention = \"90d\"\n",
        )
        .unwrap();
        let cfg = load().unwrap();
        assert_eq!(cfg.audit.retention.as_deref(), Some("90d"));
    }

    #[test]
    fn empty_file_is_defaults() {
        let _g = crate::paths::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("INSPECT_HOME", tmp.path());
        std::fs::write(tmp.path().join("config.toml"), "").unwrap();
        let cfg = load().unwrap();
        assert!(cfg.audit.retention.is_none());
    }
}
