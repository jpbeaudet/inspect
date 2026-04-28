//! F8 (v0.1.3) — runtime tier of the discovery cache.
//!
//! Background: the existing [`Profile`] cache at
//! `~/.inspect/profiles/<ns>.yaml` is the **inventory tier** — slow-changing
//! facts (container existence, names, images, declared ports, compose
//! membership). It is rewritten by `inspect setup` and otherwise persists
//! for `INSPECT_PROFILE_TTL_DAYS` (default 7d).
//!
//! That tier is wrong for the post-mutation "did the fix take?" workflow
//! the third field user hit on a Keycloak/Vault deployment: after
//! `inspect restart arte/atlas-vault --apply`, `inspect status` and
//! `inspect why` continued to report the pre-restart cached
//! `health_status`, even though the daemon now reported the service
//! healthy.
//!
//! The fix is a **runtime tier** — fast-changing facts (running state,
//! health status, restart count) — stored separately from the inventory
//! tier with its own short TTL (default 10s) and its own invalidation
//! triggers:
//!   - manual: `--refresh` flag on the read verb,
//!   - automatic: TTL expiry,
//!   - mutation: every successful write verb (`restart`, `stop`,
//!     `start`, `reload`) wipes the runtime cache for the affected
//!     namespace, so the very next read auto-refreshes.
//!
//! Storage: `~/.inspect/cache/<ns>/runtime.json`. JSON (not YAML) so the
//! per-verb hot path doesn't pay yaml-serde overhead. Atomic write +
//! mode 0600 inside a 0700 directory, matching every other inspect
//! cache file. The directory tree is created lazily on first write.
//!
//! Provenance is surfaced through [`SourceInfo`], which the read verbs
//! attach to the `OutputDoc.meta.source` field and prepend as a single
//! `SOURCE:` line to human output. The shape is a v0.1.3 contract
//! enshrined in `tests/phase_f_v013.rs`.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths;
use crate::profile::schema::HealthStatus;

/// Schema version on every runtime snapshot. Bumped on breaking
/// changes; older snapshots are silently discarded on first read.
pub const RUNTIME_SCHEMA_VERSION: u32 = 1;

/// Default runtime TTL: 10 seconds. Field-tuned: the 3rd user's
/// post-restart "did it take?" check happens within seconds, but we
/// don't want to hammer `docker inspect` on every consecutive
/// `inspect status` either. Override via `INSPECT_RUNTIME_TTL_SECS`
/// (`0` disables caching, `never` disables auto-expiry — both honored
/// by [`runtime_ttl`]).
pub const DEFAULT_RUNTIME_TTL_SECS: u64 = 10;

/// Env-var override for the runtime TTL.
pub const RUNTIME_TTL_ENV: &str = "INSPECT_RUNTIME_TTL_SECS";

const CACHE_DIRNAME: &str = "cache";
const RUNTIME_FILENAME: &str = "runtime.json";

// -----------------------------------------------------------------------------
// On-disk schema
// -----------------------------------------------------------------------------

/// Per-service runtime state. Mirrors the fields the read verbs need
/// most often: is the container actually running right now, is its
/// health probe passing, and how many times has docker restarted it
/// (a non-zero restart count after a deploy is the "flapping" smoke
/// signal F4 will lean on).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceRuntime {
    /// Real container name as docker reports it. Matches
    /// [`crate::profile::schema::Service::container_name`]. We key on
    /// the real name, never the user-facing service name, to defeat
    /// the v0.1.0 phantom-service bug.
    pub container_name: String,
    /// `true` iff `docker ps` lists this container.
    pub running: bool,
    /// Health probe outcome at fetch time. `None` when the container
    /// has no health check declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_status: Option<HealthStatus>,
    /// Docker restart count at fetch time (cumulative since container
    /// creation). Useful for spotting flapping services.
    #[serde(default)]
    pub restart_count: u32,
}

/// Top-level runtime snapshot persisted at
/// `~/.inspect/cache/<ns>/runtime.json`.
///
/// `fetched_at_unix_secs` is stored as an integer rather than RFC3339
/// because (a) it's exclusively machine-readable on this hot path and
/// (b) integer arithmetic for TTL math has no parsing failure mode.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSnapshot {
    pub schema_version: u32,
    pub namespace: String,
    pub fetched_at_unix_secs: u64,
    #[serde(default)]
    pub services: Vec<ServiceRuntime>,
    /// F8 observability: monotonically increasing count of how many
    /// times this namespace's runtime has been refreshed (live-fetched
    /// then persisted) since the cache file was first created. Surfaced
    /// by `inspect cache show`. `#[serde(default)] = 0` so caches
    /// written before this field landed (still schema_version=1) load
    /// without loss. Reset to 0 on `inspect cache clear`.
    #[serde(default)]
    pub refresh_count: u64,
}

impl RuntimeSnapshot {
    pub fn new(namespace: &str, services: Vec<ServiceRuntime>) -> Self {
        Self {
            schema_version: RUNTIME_SCHEMA_VERSION,
            namespace: namespace.to_string(),
            fetched_at_unix_secs: now_unix_secs(),
            services,
            refresh_count: 0,
        }
    }

    /// Look up a service's runtime state by its real container name.
    pub fn lookup(&self, container_name: &str) -> Option<&ServiceRuntime> {
        self.services.iter().find(|s| s.container_name == container_name)
    }

    /// Wall-clock age of this snapshot. `None` if the system clock
    /// went backwards or the snapshot is from the future (clock skew
    /// after machine sleep — treat as "fresh" so we don't hammer the
    /// remote on every call).
    pub fn age(&self) -> Option<Duration> {
        let now = now_unix_secs();
        if now >= self.fetched_at_unix_secs {
            Some(Duration::from_secs(now - self.fetched_at_unix_secs))
        } else {
            None
        }
    }
}

// -----------------------------------------------------------------------------
// SourceInfo — the SOURCE: provenance contract
// -----------------------------------------------------------------------------

/// How a read verb's data was obtained, surfaced to the operator on
/// every read invocation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceMode {
    /// Just fetched from the remote. No cache was used.
    Live,
    /// Served from cache; runtime tier is within TTL.
    Cached,
    /// Served from cache, but the cache is past its TTL (typically
    /// because an auto-refresh failed). The verb still served data
    /// rather than erroring out — degraded mode.
    Stale,
}

/// Provenance attached to every read-verb response. Serializes as the
/// `meta.source` field on JSON envelopes and as the leading `SOURCE:`
/// line in human output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceInfo {
    pub mode: SourceMode,
    /// Age of the runtime tier in seconds. `None` when the verb did
    /// not consult the runtime tier (e.g. a future inventory-only
    /// verb).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_age_s: Option<u64>,
    /// Age of the inventory tier (cached profile) in seconds. `None`
    /// when the namespace has no cached profile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inventory_age_s: Option<u64>,
    /// `true` iff any consulted tier is past its TTL. Mirrors
    /// `mode == Stale` for runtime-tier verbs; broader once
    /// inventory-only verbs adopt this.
    pub stale: bool,
    /// Optional one-line reason when `mode == Stale` — usually the
    /// auto-refresh failure cause. Surfaced in the human SOURCE: line
    /// after the `— stale, …` separator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl SourceInfo {
    /// Render the human-mode `SOURCE:` line (no trailing newline).
    /// Used by every read verb before delegating to `render_doc`.
    pub fn human_line(&self) -> String {
        match self.mode {
            SourceMode::Live => "SOURCE:  live".to_string(),
            SourceMode::Cached => {
                let runtime = self.runtime_age_s.unwrap_or(0);
                let inv = self
                    .inventory_age_s
                    .map(|s| format!(", inventory: {}", fmt_age(s)))
                    .unwrap_or_default();
                format!(
                    "SOURCE:  cached {} ago (runtime: {}{inv})",
                    fmt_age(runtime),
                    fmt_age(runtime),
                )
            }
            SourceMode::Stale => {
                let runtime = self.runtime_age_s.unwrap_or(0);
                let why = self
                    .reason
                    .as_deref()
                    .map(|r| format!(" ({r})"))
                    .unwrap_or_default();
                format!(
                    "SOURCE:  cached {} ago — stale{why}, run with --refresh to re-fetch",
                    fmt_age(runtime)
                )
            }
        }
    }

    /// JSON projection used as the `meta.source` field on read-verb
    /// envelopes. Stable shape — pinned by `tests/phase_f_v013.rs`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

/// Format a duration in seconds as a compact human string: `47s`,
/// `3m12s`, `1h05m`. Matches the F8 spec example
/// `cached 47s ago (runtime: 47s, inventory: 3m12s)`.
fn fmt_age(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h{:02}m", secs / 3600, (secs / 60) % 60)
    }
}

// -----------------------------------------------------------------------------
// On-disk paths
// -----------------------------------------------------------------------------

/// Root of the per-namespace cache tree (the runtime tier sits inside).
pub fn cache_root() -> PathBuf {
    paths::inspect_home().join(CACHE_DIRNAME)
}

/// Per-namespace cache directory.
pub fn ns_cache_dir(namespace: &str) -> PathBuf {
    cache_root().join(namespace)
}

/// Path to a namespace's runtime snapshot.
pub fn runtime_path(namespace: &str) -> PathBuf {
    ns_cache_dir(namespace).join(RUNTIME_FILENAME)
}

/// Ensure the cache root + per-ns subdirectory exist, with mode 0700
/// (matching every other inspect cache directory). Creates them lazily.
pub fn ensure_ns_cache_dir(namespace: &str) -> Result<PathBuf> {
    let root = cache_root();
    if !root.exists() {
        std::fs::create_dir_all(&root)
            .with_context(|| format!("creating cache root '{}'", root.display()))?;
    }
    paths::set_dir_mode_0700(&root).map_err(anyhow::Error::from)?;
    let dir = ns_cache_dir(namespace);
    if !dir.exists() {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating cache dir '{}'", dir.display()))?;
    }
    paths::set_dir_mode_0700(&dir).map_err(anyhow::Error::from)?;
    Ok(dir)
}

// -----------------------------------------------------------------------------
// Load / save / clear
// -----------------------------------------------------------------------------

/// Load a runtime snapshot if one exists. Returns `Ok(None)` when no
/// cache file is present, when the file's schema version doesn't match
/// the current code (silently discarded — pre-v0.2.0 contract), or when
/// the file is unreadable / malformed (treated as a cold cache; logged
/// at debug level only so a corrupted cache never fails a read).
pub fn load_runtime(namespace: &str) -> Option<RuntimeSnapshot> {
    let path = runtime_path(namespace);
    if !path.exists() {
        return None;
    }
    // Permission check is best-effort: a too-permissive cache is logged
    // and discarded rather than failing the verb (degraded mode > broken
    // verb on the read hot path).
    if paths::check_file_mode_0600(&path).is_err() {
        return None;
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return None,
    };
    let snap: RuntimeSnapshot = match serde_json::from_str(&text) {
        Ok(s) => s,
        Err(_) => return None,
    };
    if snap.schema_version != RUNTIME_SCHEMA_VERSION {
        return None;
    }
    if snap.namespace != namespace {
        return None;
    }
    Some(snap)
}

/// Atomically write a runtime snapshot to disk with mode 0600.
pub fn save_runtime(snap: &RuntimeSnapshot) -> Result<PathBuf> {
    let dir = ensure_ns_cache_dir(&snap.namespace)?;
    let path = dir.join(RUNTIME_FILENAME);
    let body = serde_json::to_vec_pretty(snap)
        .with_context(|| format!("serializing runtime snapshot for '{}'", snap.namespace))?;
    let mut tmp = tempfile::NamedTempFile::new_in(&dir)
        .with_context(|| format!("creating runtime tempfile in '{}'", dir.display()))?;
    use std::io::Write;
    tmp.write_all(&body).context("writing runtime bytes")?;
    tmp.flush().context("flushing runtime")?;
    tmp.as_file()
        .sync_all()
        .context("fsync runtime tempfile")?;
    let tmp_path = tmp.into_temp_path();
    tmp_path
        .persist(&path)
        .with_context(|| format!("renaming runtime into '{}'", path.display()))?;
    paths::set_file_mode_0600(&path).map_err(anyhow::Error::from)?;
    Ok(path)
}

/// F8: a refresh that *bumps* `refresh_count`, preserving the prior
/// count from any existing on-disk snapshot. Use this from the cache
/// orchestrator's live-fetch path; never from raw [`save_runtime`]
/// callers (tests, fixture writers) — those are intentionally
/// snapshot-as-given.
pub fn save_refreshed(mut snap: RuntimeSnapshot) -> Result<PathBuf> {
    let prior = load_runtime(&snap.namespace)
        .map(|s| s.refresh_count)
        .unwrap_or(0);
    snap.refresh_count = prior.saturating_add(1);
    save_runtime(&snap)
}

/// F8: advisory file lock around the cache-refresh critical section.
///
/// On unix (the only supported platform) this uses `flock(2)` with
/// `LOCK_EX` against a sidecar `.lock` file inside the per-namespace
/// cache directory. The lock is held for the full body of `f` and
/// released when its file descriptor drops. `flock` is advisory —
/// only inspect itself respects it — which is fine: nothing else
/// touches `~/.inspect/cache/<ns>/`.
///
/// **What this protects:** two concurrent `inspect status arte` (or
/// status during a `restart`) racing to refresh would otherwise both
/// `fetch_live` then both `save_refreshed`, double-counting
/// `refresh_count` and wasting one of the two docker round-trips.
/// The lock serializes them; the second waiter sees the first's
/// fresh snapshot and can return `Cached` without fetching.
///
/// **Failure mode:** if the lock cannot be acquired (foreign FS that
/// doesn't implement flock, or the lock file is unlinked under us),
/// we fall through and run `f` unlocked — degraded behavior, never a
/// hard error on the read hot path.
#[cfg(unix)]
pub fn with_runtime_lock<R>(namespace: &str, f: impl FnOnce() -> R) -> R {
    use std::os::unix::io::AsRawFd;
    let dir = match ensure_ns_cache_dir(namespace) {
        Ok(d) => d,
        Err(_) => return f(), // can't make the dir → run unlocked
    };
    let lock_path = dir.join(".lock");
    let file = match std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
    {
        Ok(f) => f,
        Err(_) => return f(),
    };
    // Best-effort 0600 on the lock file too.
    let _ = paths::set_file_mode_0600(&lock_path);
    // SAFETY: libc::flock takes a raw fd and an op. LOCK_EX blocks
    // until the lock is acquired; LOCK_UN is implicit on fd close.
    let fd = file.as_raw_fd();
    let rc = unsafe { libc::flock(fd, libc::LOCK_EX) };
    let result = f();
    if rc == 0 {
        // Explicit unlock (also happens on drop, but be explicit).
        unsafe {
            libc::flock(fd, libc::LOCK_UN);
        }
    }
    drop(file);
    result
}

#[cfg(not(unix))]
pub fn with_runtime_lock<R>(_namespace: &str, f: impl FnOnce() -> R) -> R {
    f()
}

/// Wipe the runtime snapshot for a single namespace. Called by every
/// successful write verb (lifecycle restart/stop/start/reload) so the
/// very next read auto-refreshes from live state — the post-mutation
/// "did it take?" workflow the 3rd field user needed.
///
/// Best-effort: a missing file is not an error; an unlink failure is
/// silently swallowed (a stale snapshot is still better than failing
/// the just-completed mutation, and the next `--refresh` will fix it).
pub fn clear_runtime(namespace: &str) {
    let path = runtime_path(namespace);
    let _ = std::fs::remove_file(&path);
}

/// Wipe the entire cache tree (every namespace, every tier). Backs
/// `inspect cache clear` with no namespace argument.
pub fn clear_all() -> Result<()> {
    let root = cache_root();
    if root.exists() {
        std::fs::remove_dir_all(&root)
            .with_context(|| format!("clearing cache root '{}'", root.display()))?;
    }
    Ok(())
}

/// List every namespace that currently has a cache directory, even if
/// the runtime file is missing inside it. Used by `inspect cache show`
/// (no-arg form) to iterate.
pub fn list_cached_namespaces() -> Vec<String> {
    let root = cache_root();
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&root) {
        for entry in rd.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                if let Some(name) = entry.file_name().to_str() {
                    out.push(name.to_string());
                }
            }
        }
    }
    out.sort();
    out
}

// -----------------------------------------------------------------------------
// TTL configuration
// -----------------------------------------------------------------------------

/// Resolved runtime TTL.
///
/// - `Some(Duration)` — the snapshot is fresh while younger than this.
/// - `None` — caching disabled (every read fetches live).
///
/// Spec values (env var `INSPECT_RUNTIME_TTL_SECS`):
/// - unset → [`DEFAULT_RUNTIME_TTL_SECS`]
/// - `"0"` → caching disabled
/// - `"never"` → effectively infinite TTL (only manual `--refresh`
///   invalidates) — represented as `Some(Duration::MAX)`
/// - any non-negative integer → that many seconds
/// - any malformed value → silently falls back to default (the read
///   path must never error on a misconfigured TTL)
pub fn runtime_ttl() -> Option<Duration> {
    match std::env::var(RUNTIME_TTL_ENV).ok().as_deref() {
        Some("0") => None,
        Some("never") => Some(Duration::MAX),
        Some(s) if !s.is_empty() => match s.parse::<u64>() {
            Ok(n) => Some(Duration::from_secs(n)),
            Err(_) => Some(Duration::from_secs(DEFAULT_RUNTIME_TTL_SECS)),
        },
        _ => Some(Duration::from_secs(DEFAULT_RUNTIME_TTL_SECS)),
    }
}

/// `true` iff a snapshot is past its configured runtime TTL.
pub fn is_runtime_stale(snap: &RuntimeSnapshot) -> bool {
    let Some(ttl) = runtime_ttl() else {
        return true; // caching disabled — every cached snapshot is "stale"
    };
    let Some(age) = snap.age() else {
        return false; // future-dated (clock skew) — treat as fresh
    };
    age > ttl
}

// -----------------------------------------------------------------------------
// Inventory-tier age helper (read-only — does not modify Profile cache)
// -----------------------------------------------------------------------------

/// Age of the cached profile (inventory tier) for a namespace. `None`
/// when no profile is on disk or its `discovered_at` is unparseable.
pub fn inventory_age(namespace: &str) -> Option<Duration> {
    let p = crate::profile::cache::load_profile(namespace).ok().flatten()?;
    let then = chrono::DateTime::parse_from_rfc3339(&p.discovered_at).ok()?;
    let then_sys: SystemTime = then.into();
    SystemTime::now().duration_since(then_sys).ok()
}

// -----------------------------------------------------------------------------
// Internal helpers
// -----------------------------------------------------------------------------

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
pub fn touch_runtime_age(namespace: &str, age: Duration) -> Result<()> {
    // Test helper: rewrite the snapshot's fetched_at to be `age` seconds
    // in the past, simulating an aged cache without sleeping.
    let mut snap = load_runtime(namespace)
        .ok_or_else(|| anyhow::anyhow!("no runtime snapshot for {namespace}"))?;
    let now = now_unix_secs();
    snap.fetched_at_unix_secs = now.saturating_sub(age.as_secs());
    save_runtime(&snap)?;
    Ok(())
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::MutexGuard;

    fn env_lock() -> MutexGuard<'static, ()> {
        crate::paths::TEST_ENV_LOCK
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
        std::env::remove_var(RUNTIME_TTL_ENV);
        HomeGuard { _g: g, _dir: d }
    }

    fn snap(ns: &str) -> RuntimeSnapshot {
        RuntimeSnapshot::new(
            ns,
            vec![ServiceRuntime {
                container_name: "atlas".into(),
                running: true,
                health_status: Some(HealthStatus::Ok),
                restart_count: 0,
            }],
        )
    }

    #[test]
    fn save_and_load_round_trip() {
        let _g = temp_home();
        let s = snap("arte");
        save_runtime(&s).unwrap();
        let back = load_runtime("arte").unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn missing_returns_none() {
        let _g = temp_home();
        assert!(load_runtime("arte").is_none());
    }

    #[test]
    fn schema_version_mismatch_silently_discards() {
        let _g = temp_home();
        let dir = ensure_ns_cache_dir("arte").unwrap();
        let path = dir.join(RUNTIME_FILENAME);
        std::fs::write(&path, b"{\"schema_version\": 999, \"namespace\": \"arte\", \"fetched_at_unix_secs\": 0, \"services\": []}").unwrap();
        crate::paths::set_file_mode_0600(&path).unwrap();
        assert!(load_runtime("arte").is_none());
    }

    #[test]
    fn malformed_returns_none_not_error() {
        let _g = temp_home();
        let dir = ensure_ns_cache_dir("arte").unwrap();
        let path = dir.join(RUNTIME_FILENAME);
        std::fs::write(&path, b"not json at all").unwrap();
        crate::paths::set_file_mode_0600(&path).unwrap();
        assert!(load_runtime("arte").is_none());
    }

    #[test]
    fn ttl_default_is_ten_seconds() {
        let _g = temp_home();
        assert_eq!(runtime_ttl(), Some(Duration::from_secs(10)));
    }

    #[test]
    fn ttl_zero_disables_caching() {
        let _g = temp_home();
        std::env::set_var(RUNTIME_TTL_ENV, "0");
        assert_eq!(runtime_ttl(), None);
        std::env::remove_var(RUNTIME_TTL_ENV);
    }

    #[test]
    fn ttl_never_is_max() {
        let _g = temp_home();
        std::env::set_var(RUNTIME_TTL_ENV, "never");
        assert_eq!(runtime_ttl(), Some(Duration::MAX));
        std::env::remove_var(RUNTIME_TTL_ENV);
    }

    #[test]
    fn is_stale_respects_ttl() {
        let _g = temp_home();
        let mut s = snap("arte");
        // Just-fetched: not stale.
        assert!(!is_runtime_stale(&s));
        // Aged 60s with default 10s TTL: stale.
        s.fetched_at_unix_secs = now_unix_secs().saturating_sub(60);
        assert!(is_runtime_stale(&s));
    }

    #[test]
    fn clear_runtime_removes_file() {
        let _g = temp_home();
        save_runtime(&snap("arte")).unwrap();
        assert!(load_runtime("arte").is_some());
        clear_runtime("arte");
        assert!(load_runtime("arte").is_none());
    }

    #[test]
    fn clear_runtime_missing_is_noop() {
        let _g = temp_home();
        clear_runtime("never-existed"); // must not panic
    }

    #[test]
    fn fmt_age_compact() {
        assert_eq!(fmt_age(0), "0s");
        assert_eq!(fmt_age(47), "47s");
        assert_eq!(fmt_age(192), "3m12s");
        assert_eq!(fmt_age(3900), "1h05m");
    }

    #[test]
    fn source_human_line_shapes() {
        let live = SourceInfo {
            mode: SourceMode::Live,
            runtime_age_s: Some(0),
            inventory_age_s: Some(120),
            stale: false,
            reason: None,
        };
        assert_eq!(live.human_line(), "SOURCE:  live");

        let cached = SourceInfo {
            mode: SourceMode::Cached,
            runtime_age_s: Some(47),
            inventory_age_s: Some(192),
            stale: false,
            reason: None,
        };
        assert_eq!(
            cached.human_line(),
            "SOURCE:  cached 47s ago (runtime: 47s, inventory: 3m12s)"
        );

        let stale = SourceInfo {
            mode: SourceMode::Stale,
            runtime_age_s: Some(120),
            inventory_age_s: None,
            stale: true,
            reason: Some("docker daemon unreachable".into()),
        };
        let line = stale.human_line();
        assert!(line.starts_with("SOURCE:  cached 2m00s ago — stale"));
        assert!(line.contains("docker daemon unreachable"));
        assert!(line.contains("--refresh"));
    }

    #[test]
    fn list_cached_namespaces_finds_dirs() {
        let _g = temp_home();
        save_runtime(&snap("arte")).unwrap();
        save_runtime(&snap("prod-eu")).unwrap();
        let names = list_cached_namespaces();
        assert_eq!(names, vec!["arte".to_string(), "prod-eu".to_string()]);
    }

    #[test]
    fn touch_runtime_age_helper_works() {
        let _g = temp_home();
        save_runtime(&snap("arte")).unwrap();
        touch_runtime_age("arte", Duration::from_secs(60)).unwrap();
        let back = load_runtime("arte").unwrap();
        assert!(is_runtime_stale(&back));
    }
}
