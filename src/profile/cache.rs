//! On-disk profile cache at `~/.inspect/profiles/<ns>.yaml`.
//!
//! - File mode 0600, directory 0700 (bible §security).
//! - Atomic write (tempfile + rename).
//! - Local edits (`groups`, `aliases`, `local_overrides`) preserved across
//!   re-discovery via `merge_local_edits`.
//! - TTL: 7 days by default; configurable via `INSPECT_PROFILE_TTL_DAYS`.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};

use super::schema::Profile;
use crate::error::ConfigError;
use crate::paths;

pub const DEFAULT_TTL_DAYS: u64 = 7;
const PROFILES_DIRNAME: &str = "profiles";

/// Directory holding all per-namespace profiles.
pub fn profiles_dir() -> PathBuf {
    paths::inspect_home().join(PROFILES_DIRNAME)
}

/// Path to a single profile.
pub fn profile_path(namespace: &str) -> PathBuf {
    profiles_dir().join(format!("{namespace}.yaml"))
}

/// Path to the drift marker for a namespace. Written by the async drift
/// checker when the live host fingerprint diverges from the cached profile.
pub fn drift_marker_path(namespace: &str) -> PathBuf {
    profiles_dir().join(format!("{namespace}.drift"))
}

/// Ensure the profiles directory exists with mode 0700.
pub fn ensure_profiles_dir() -> std::result::Result<PathBuf, ConfigError> {
    paths::ensure_home()?;
    let dir = profiles_dir();
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| ConfigError::Io {
            path: dir.display().to_string(),
            source: e,
        })?;
    }
    paths::set_dir_mode_0700(&dir)?;
    Ok(dir)
}

/// Load a profile if one exists. Returns `Ok(None)` when no cache file is
/// present.
pub fn load_profile(namespace: &str) -> Result<Option<Profile>> {
    let path = profile_path(namespace);
    if !path.exists() {
        return Ok(None);
    }
    paths::check_file_mode_0600(&path).map_err(anyhow::Error::from)?;
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading profile '{}'", path.display()))?;
    let p: Profile = serde_yaml::from_str(&text)
        .with_context(|| format!("parsing profile '{}'", path.display()))?;
    Ok(Some(p))
}

/// Atomically write a profile to disk with mode 0600. Preserves the user's
/// local edits if a previous profile is on disk.
pub fn save_profile(profile: &Profile) -> Result<PathBuf> {
    ensure_profiles_dir().map_err(anyhow::Error::from)?;
    let path = profile_path(&profile.namespace);

    // Merge any user-owned sections from the existing profile.
    let merged = match load_profile(&profile.namespace) {
        Ok(Some(prev)) => merge_local_edits(profile.clone(), &prev),
        _ => profile.clone(),
    };

    let yaml = serde_yaml::to_string(&merged)
        .with_context(|| format!("serializing profile for '{}'", profile.namespace))?;

    let dir = profiles_dir();
    let mut tmp = tempfile::NamedTempFile::new_in(&dir)
        .with_context(|| format!("creating tempfile in '{}'", dir.display()))?;
    use std::io::Write;
    tmp.write_all(yaml.as_bytes())
        .context("writing profile bytes")?;
    tmp.flush().context("flushing profile")?;
    tmp.as_file()
        .sync_all()
        .context("fsync profile tempfile")?;
    let tmp_path = tmp.into_temp_path();
    tmp_path
        .persist(&path)
        .with_context(|| format!("renaming profile into '{}'", path.display()))?;
    paths::set_file_mode_0600(&path).map_err(anyhow::Error::from)?;
    Ok(path)
}

/// Carry the user-owned sections from `prev` into `incoming`.
pub fn merge_local_edits(mut incoming: Profile, prev: &Profile) -> Profile {
    if !prev.groups.is_empty() {
        for (k, v) in &prev.groups {
            incoming.groups.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    if !prev.aliases.is_empty() {
        for (k, v) in &prev.aliases {
            incoming.aliases.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    if incoming.local_overrides.is_none() {
        incoming.local_overrides = prev.local_overrides.clone();
    }
    incoming
}

/// `true` iff the profile is older than the configured TTL.
pub fn is_stale(profile: &Profile) -> bool {
    let ttl_days = std::env::var("INSPECT_PROFILE_TTL_DAYS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_TTL_DAYS);
    let ttl = Duration::from_secs(ttl_days * 24 * 3600);

    let Ok(then) = chrono::DateTime::parse_from_rfc3339(&profile.discovered_at) else {
        return true;
    };
    let then_sys: SystemTime = then.into();
    SystemTime::now()
        .duration_since(then_sys)
        .map(|d| d > ttl)
        .unwrap_or(false)
}

/// Write the drift marker for a namespace. Best-effort.
pub fn write_drift_marker(namespace: &str, current_fp: &str, cached_fp: &str) -> Result<()> {
    ensure_profiles_dir().map_err(anyhow::Error::from)?;
    let path = drift_marker_path(namespace);
    let body = format!(
        "namespace: {namespace}\ncached_fingerprint: {cached_fp}\ncurrent_fingerprint: {current_fp}\nobserved_at: {}\n",
        chrono::Utc::now().to_rfc3339()
    );
    std::fs::write(&path, body)
        .with_context(|| format!("writing drift marker '{}'", path.display()))?;
    paths::set_file_mode_0600(&path).map_err(anyhow::Error::from)?;
    Ok(())
}

/// Remove the drift marker if one exists.
pub fn clear_drift_marker(namespace: &str) {
    let _ = std::fs::remove_file(drift_marker_path(namespace));
}

/// Read the drift marker if one exists.
pub fn read_drift_marker(namespace: &str) -> Option<String> {
    let path = drift_marker_path(namespace);
    std::fs::read_to_string(path).ok()
}

#[allow(dead_code)]
pub fn profile_exists(namespace: &str) -> bool {
    profile_path(namespace).exists()
}

#[allow(dead_code)]
pub fn profile_age_secs(profile: &Profile) -> Option<u64> {
    let then = chrono::DateTime::parse_from_rfc3339(&profile.discovered_at).ok()?;
    let then_sys: SystemTime = then.into();
    SystemTime::now().duration_since(then_sys).ok().map(|d| d.as_secs())
}

#[allow(dead_code)]
pub fn rfc3339_now() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[allow(dead_code)]
pub fn delete_profile(namespace: &str) -> Result<()> {
    let path = profile_path(namespace);
    if path.exists() {
        std::fs::remove_file(&path)
            .with_context(|| format!("removing '{}'", path.display()))?;
    }
    clear_drift_marker(namespace);
    Ok(())
}

#[allow(dead_code)]
pub fn cache_root() -> &'static Path {
    // Kept for future phases that need a stable cache root.
    Path::new(PROFILES_DIRNAME)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::schema::*;
    use std::collections::BTreeMap;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    struct HomeGuard {
        _g: MutexGuard<'static, ()>,
        _dir: tempfile::TempDir,
    }

    fn temp_home() -> HomeGuard {
        let g = env_lock();
        let d = tempfile::tempdir().unwrap();
        std::env::set_var(crate::paths::INSPECT_HOME_ENV, d.path());
        HomeGuard { _g: g, _dir: d }
    }

    #[test]
    fn save_and_load_round_trip() {
        let _g = temp_home();
        let p = Profile::empty("arte", "h", &rfc3339_now());
        save_profile(&p).unwrap();
        let back = load_profile("arte").unwrap().unwrap();
        assert_eq!(back.namespace, "arte");
        assert_eq!(back.host, "h");
    }

    #[test]
    fn merge_preserves_user_groups_and_aliases() {
        let _g = temp_home();
        let mut prev = Profile::empty("arte", "h", &rfc3339_now());
        prev.groups
            .insert("storage".into(), vec!["postgres".into(), "redis".into()]);
        prev.aliases.insert("api".into(), "pulse".into());
        save_profile(&prev).unwrap();

        let mut incoming = Profile::empty("arte", "h", &rfc3339_now());
        incoming.services.push(Service {
            name: "atlas".into(),
            container_id: None,
            image: None,
            ports: vec![],
            health: None,
            health_status: None,
            log_driver: None,
            log_readable_directly: false,
            mounts: vec![],
            kind: ServiceKind::Container,
            depends_on: vec![],
        });
        save_profile(&incoming).unwrap();
        let back = load_profile("arte").unwrap().unwrap();
        assert_eq!(back.groups.get("storage").unwrap().len(), 2);
        assert_eq!(back.aliases.get("api").unwrap(), "pulse");
        assert_eq!(back.services.len(), 1);
    }

    #[test]
    fn stale_when_old() {
        let mut p = Profile::empty("arte", "h", "2000-01-01T00:00:00Z");
        assert!(is_stale(&p));
        p.discovered_at = chrono::Utc::now().to_rfc3339();
        assert!(!is_stale(&p));
    }

    #[test]
    fn drift_marker_round_trip() {
        let _g = temp_home();
        let _ = ensure_profiles_dir().unwrap();
        write_drift_marker("arte", "abc", "def").unwrap();
        let body = read_drift_marker("arte").unwrap();
        assert!(body.contains("cached_fingerprint: def"));
        clear_drift_marker("arte");
        assert!(read_drift_marker("arte").is_none());
    }

    #[test]
    fn unused_imports_silenced() {
        // Touch the BTreeMap import to keep clippy happy when no test uses it.
        let _ = BTreeMap::<String, String>::new();
    }
}
