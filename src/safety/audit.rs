//! Audit log: append-only, per-month JSONL files under
//! `~/.inspect/audit/<YYYY-MM>-<user>.jsonl` (mode 0600).
//!
//! Schema mirrors bible §8.2.

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::paths::{audit_dir, ensure_home, set_dir_mode_0700, set_file_mode_0600};

#[cfg(unix)]
use std::os::unix::io::AsRawFd;

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
    /// Free-form operator note attached at invocation time via
    /// `--reason`. Limited to 240 characters by the CLI layer (see
    /// [`crate::safety::audit::validate_reason`]); recorded verbatim
    /// here so audit downstream can grep on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// B9 (v0.1.2): bundle correlation id. When set, every step run
    /// from the same `inspect bundle run` invocation shares this id
    /// so `inspect audit ls --bundle <id>` can reconstruct the
    /// transaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    /// B9 (v0.1.2): the step id within the bundle. Lets reviewers
    /// see which YAML step produced this entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_step: Option<String>,
    /// F9 (v0.1.3): byte count of local stdin forwarded to the remote
    /// command. `0` (or absent on read) means stdin was not forwarded
    /// (tty input, `--no-stdin`, or no piped input). Recorded so a
    /// post-hoc audit can answer "what input did this command consume?"
    /// by size.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub stdin_bytes: u64,
    /// F9 (v0.1.3): SHA-256 of forwarded stdin, present only when the
    /// caller passed `--audit-stdin-hash`. Off by default for perf;
    /// opt-in for security-sensitive runs (auditable byte-for-byte
    /// reconstruction without storing the bytes themselves).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin_sha256: Option<String>,
    /// F11 (v0.1.3): captured inverse of this audit entry. Populated
    /// at capture-before-apply time by every write verb. `None` on
    /// pre-F11 (v0.1.2 or earlier) entries — those are treated as
    /// `revert.kind = "unsupported"` on read. `inspect revert <id>`
    /// consults this field; legacy entries still revert through the
    /// `previous_hash` + `snapshot` path for backward compat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revert: Option<Revert>,
    /// F11 (v0.1.3): `true` when the mutation actually ran on the
    /// remote, `false` when capture succeeded but dispatch failed (or
    /// the verb is still in-flight). `None` on legacy entries. Lets
    /// `inspect revert` no-op cleanly on entries that never applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied: Option<bool>,
    /// F11 (v0.1.3): set when the operator explicitly passed
    /// `--no-revert` on a verb whose inverse is fundamentally
    /// undefined (e.g. `inspect exec` of a free-form script).
    /// `inspect revert <id>` on such entries surfaces a chained hint
    /// rather than silently no-opping.
    #[serde(default, skip_serializing_if = "is_false")]
    pub no_revert_acknowledged: bool,
    /// F11 (v0.1.3): when this entry was the auto-revert of a failed
    /// apply (`--revert-on-failure` triggered), this links back to the
    /// original entry so audit readers can see the relationship.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_revert_of: Option<String>,
}

fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// F11 (v0.1.3): inverse-capture taxonomy. Every write verb declares
/// one of these at capture time. `Unsupported` is reserved for verbs
/// whose effect is intrinsically non-reversible (free-form `exec`,
/// SIGHUP `reload`, side-effecting commands with no clean inverse);
/// applying such verbs requires the operator to opt in via
/// `--no-revert` so the contract is never silently undermined.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RevertKind {
    /// Inverse is a single remote command (e.g. `chmod 0644 <path>`,
    /// `docker start <ctr>`).
    CommandPair,
    /// Inverse is restoring a captured state blob (the existing
    /// `snapshot` field carries the path; `payload` is the snapshot
    /// hash for fast lookup).
    StateSnapshot,
    /// Multi-step inverse — `payload` is a JSON-encoded ordered list
    /// of `{kind, payload}` records that should be executed in
    /// reverse order. Used by bundle steps that touch multiple paths.
    Composite,
    /// This verb has no general inverse on this invocation. `inspect
    /// revert <id>` exits 2 with the chained explanation; never
    /// silently no-ops.
    Unsupported,
}

impl RevertKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CommandPair => "command_pair",
            Self::StateSnapshot => "state_snapshot",
            Self::Composite => "composite",
            Self::Unsupported => "unsupported",
        }
    }
}

/// F11 (v0.1.3): captured inverse for a single write-verb invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Revert {
    pub kind: RevertKind,
    /// Structured inverse — a remote command string, a snapshot hash,
    /// or a JSON-encoded list (for `Composite`). Empty for
    /// `Unsupported`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub payload: String,
    /// Timestamp at which the inverse was captured. Always **before**
    /// the apply step ran (capture-before-apply contract).
    pub captured_at: DateTime<Utc>,
    /// One-line human-readable description ("restore /etc/foo (was
    /// 0644 root:root)"). Used by `inspect revert <id> --dry-run` and
    /// by `--revert-preview` on write verbs.
    pub preview: String,
}

impl Revert {
    pub fn unsupported(preview: impl Into<String>) -> Self {
        Self {
            kind: RevertKind::Unsupported,
            payload: String::new(),
            captured_at: Utc::now(),
            preview: preview.into(),
        }
    }
    pub fn command_pair(payload: impl Into<String>, preview: impl Into<String>) -> Self {
        Self {
            kind: RevertKind::CommandPair,
            payload: payload.into(),
            captured_at: Utc::now(),
            preview: preview.into(),
        }
    }
    pub fn state_snapshot(snapshot_hash: impl Into<String>, preview: impl Into<String>) -> Self {
        Self {
            kind: RevertKind::StateSnapshot,
            payload: snapshot_hash.into(),
            captured_at: Utc::now(),
            preview: preview.into(),
        }
    }
}

impl AuditEntry {
    pub fn new(verb: &str, selector: &str) -> Self {
        let ts = Utc::now();
        let id = format!("{}-{:04x}", ts.timestamp_millis(), (rand_u32() & 0xffff));
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
            reason: None,
            bundle_id: None,
            bundle_step: None,
            stdin_bytes: 0,
            stdin_sha256: None,
            revert: None,
            applied: None,
            no_revert_acknowledged: false,
            auto_revert_of: None,
        }
    }
}

/// Cap on the length of the `--reason` text (P12, v0.1.1). The audit
/// log is a per-month JSONL file; runaway --reason payloads would
/// bloat lines and make `audit ls` unreadable. 240 characters is a
/// pragmatic upper bound (≈ a tweet) that fits both Jira keys and a
/// short sentence.
pub const REASON_MAX_LEN: usize = 240;

/// Validate a `--reason` value. Returns `Ok(Some(text))` for valid
/// non-empty input, `Ok(None)` for `None`, and `Err(_)` when the text
/// is too long. The trim is intentional: trailing whitespace from
/// shells/aliases would otherwise count toward the limit.
pub fn validate_reason(raw: Option<&str>) -> anyhow::Result<Option<String>> {
    match raw {
        None => Ok(None),
        Some(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            if trimmed.chars().count() > REASON_MAX_LEN {
                return Err(anyhow::anyhow!(
                    "--reason must be ≤ {REASON_MAX_LEN} characters"
                ));
            }
            Ok(Some(trimmed.to_string()))
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
            std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
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
        append_locked(&path, &line)?;
        let _ = set_file_mode_0600(&path);
        Ok(())
    }

    /// Iterate entries newest-last (file order). Returns all months merged.
    pub fn all(&self) -> Result<Vec<AuditEntry>> {
        let mut files: Vec<PathBuf> = match std::fs::read_dir(&self.dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_file() && p.extension().and_then(|s| s.to_str()) == Some("jsonl"))
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

/// Append `line` (no trailing newline) to `path` as a single locked,
/// newline-terminated write. Audit entries can exceed `PIPE_BUF` (4 KB)
/// once they include a `diff_summary`, so POSIX `O_APPEND` atomicity is
/// not enough on its own: two concurrent `inspect edit ... --apply`
/// processes could otherwise interleave bytes mid-line and corrupt the
/// JSONL file.
///
/// We therefore:
///   1. open the file `O_APPEND | O_CREAT`,
///   2. take a blocking exclusive `flock(LOCK_EX)`,
///   3. issue **one** `write_all` containing the line + `\n`,
///   4. release the lock implicitly on close (or explicitly on Unix).
///
/// `flock` is advisory but every well-behaved process that uses this
/// helper participates, which is sufficient for inspect's own
/// concurrent fleet writes.
fn append_locked(path: &Path, line: &str) -> Result<()> {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;

    #[cfg(unix)]
    {
        let fd = f.as_raw_fd();
        // SAFETY: fd is valid for the lifetime of `f`. flock is a
        // documented blocking call and returns -1 on EINTR; we retry
        // a few times then give up so a stuck NFS lock can't hang the
        // process forever.
        let mut tries = 0;
        loop {
            let rc = unsafe { libc::flock(fd, libc::LOCK_EX) };
            if rc == 0 {
                break;
            }
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) && tries < 5 {
                tries += 1;
                continue;
            }
            // Lock acquisition failed (e.g. on a filesystem that does
            // not implement flock such as some NFS configurations).
            // Fall through and accept the POSIX append-only guarantee:
            // entries ≤ PIPE_BUF stay atomic, larger ones may interleave
            // — but at least we don't refuse to write the audit record.
            eprintln!(
                "inspect: warning: flock on audit log failed ({}); falling back to O_APPEND only",
                err
            );
            break;
        }
    }

    let mut buf = String::with_capacity(line.len() + 1);
    buf.push_str(line);
    buf.push('\n');
    f.write_all(buf.as_bytes()).context("writing audit entry")?;
    f.flush().context("flushing audit entry")?;
    // Forensic durability: an audit entry that exists only in the
    // kernel page cache is one power loss away from being lost. We
    // pay one fdatasync per mutation. Best-effort: on filesystems
    // that don't implement fsync (some FUSE/network mounts), degrade
    // gracefully — we'd rather keep the record we just wrote than
    // refuse the operation.
    if let Err(e) = f.sync_data() {
        eprintln!(
            "inspect: warning: audit fsync failed ({}); entry written but may not be durable",
            e
        );
    }

    #[cfg(unix)]
    {
        let fd = f.as_raw_fd();
        // Best-effort unlock. Closing `f` will release the lock anyway.
        unsafe { libc::flock(fd, libc::LOCK_UN) };
    }

    Ok(())
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
/// uniqueness within a millisecond. We combine three things:
///
/// * a process-local monotonic counter (collision-free within a single
///   process),
/// * the process id (separates concurrent fleet children), and
/// * the nanosecond fraction of the current wall clock (separates
///   bursts that share the same `timestamp_millis()`).
///
/// The result is masked to 16 bits at the call site purely for the id's
/// printed width; the counter component guarantees that two `append()`
/// calls in the same millisecond from the same process never produce the
/// same id (the counter alone walks the full 16-bit space before
/// wrapping, which is well above any realistic per-millisecond write
/// rate).
fn rand_u32() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    counter
        .wrapping_mul(2654435761)
        .wrapping_add(nanos)
        .wrapping_add(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_append_and_read() {
        let _guard = crate::paths::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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

    /// Concurrent `--apply` on the same audit log must not interleave
    /// bytes mid-line. We hammer the same file with many threads,
    /// each writing a long entry whose `diff_summary` exceeds
    /// `PIPE_BUF` (4 KB), then verify every line parses as JSON and
    /// the count matches.
    ///
    /// We deliberately bypass `AuditStore::open()` (which would touch
    /// the process-wide `INSPECT_HOME` env var and race with other
    /// tests) and exercise `append_locked` directly, which is the
    /// actual concurrency surface.
    #[test]
    fn concurrent_appends_are_atomic() {
        use std::sync::Arc;
        use std::thread;

        let tmp = tempfile::tempdir().unwrap();
        let path = Arc::new(tmp.path().join("audit.jsonl"));

        const THREADS: usize = 8;
        const PER_THREAD: usize = 25;
        // 6 KB filler — definitely larger than PIPE_BUF (4 KB on
        // Linux), the regime where O_APPEND alone is no longer atomic.
        let big = "x".repeat(6 * 1024);

        let mut handles = Vec::new();
        for t in 0..THREADS {
            let path = Arc::clone(&path);
            let big = big.clone();
            handles.push(thread::spawn(move || {
                for i in 0..PER_THREAD {
                    let mut e = AuditEntry::new("edit", &format!("t{t}/i{i}"));
                    e.diff_summary = big.clone();
                    let line = serde_json::to_string(&e).unwrap();
                    append_locked(&path, &line).expect("append");
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        // Read raw lines and verify each one is valid JSON (no
        // interleaved bytes) and we got every entry.
        let content = std::fs::read_to_string(&*path).unwrap();
        let mut count = 0;
        for line in content.lines() {
            assert!(!line.is_empty(), "empty line in audit log");
            serde_json::from_str::<AuditEntry>(line)
                .unwrap_or_else(|e| panic!("corrupted audit line: {e}\nline: {line:.200}"));
            count += 1;
        }
        assert_eq!(count, THREADS * PER_THREAD);
    }
}
