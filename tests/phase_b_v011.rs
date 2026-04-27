//! Phase B acceptance tests for v0.1.1.
//!
//! Pins the Phase B patches from `INSPECT_v0.1.1_PATCH_SPEC.md` against
//! regressions:
//!
//! - **P6** `inspect run` (read-only counterpart to `exec`).
//! - **P7** `--allow-exec` flag has been removed from `inspect exec`.
//! - **P3** `--match` / `--exclude` regex pushdown on logs + grep.
//! - **P10** `--since-last` cursor / `--reset-cursor` round-trip.
//! - **P12** `--reason` end-to-end on write verbs (audit log + filter +
//!   240-char limit).

use std::sync::{Mutex, MutexGuard, OnceLock};

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use serde_json::json;

fn lock() -> MutexGuard<'static, ()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

struct Sandbox {
    _g: MutexGuard<'static, ()>,
    home: tempfile::TempDir,
    mock: tempfile::NamedTempFile,
}

impl Sandbox {
    fn new(mock_responses: serde_json::Value) -> Self {
        let g = lock();
        let home = tempfile::tempdir().unwrap();
        let mock = tempfile::Builder::new()
            .prefix("inspect-mock-")
            .suffix(".json")
            .tempfile()
            .unwrap();
        std::fs::write(mock.path(), serde_json::to_string(&mock_responses).unwrap()).unwrap();
        Self { _g: g, home, mock }
    }
    fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("inspect").unwrap();
        c.env("INSPECT_HOME", self.home.path())
            .env("INSPECT_MOCK_REMOTE_FILE", self.mock.path())
            .env_remove("CODESPACES");
        c
    }
    fn home(&self) -> &std::path::Path {
        self.home.path()
    }
}

fn write_servers_toml(home: &std::path::Path, ns: &str) {
    let body = format!(
        "schema_version = 1\n\n\
         [namespaces.{ns}]\n\
         host = \"{ns}.example.invalid\"\n\
         user = \"deploy\"\n\
         port = 22\n"
    );
    let path = home.join("servers.toml");
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn write_profile(
    home: &std::path::Path,
    ns: &str,
    services: &[(&str, &str)], // (name, container_name)
) {
    let dir = home.join("profiles");
    std::fs::create_dir_all(&dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut svc_yaml = String::new();
    for (name, container) in services {
        svc_yaml.push_str(&format!(
            "  - name: {name}\n    \
                container_name: {container}\n    \
                container_id: cid-{container}\n    \
                image: img/{name}:1\n    \
                ports: []\n    \
                mounts: []\n    \
                health_status: ok\n    \
                log_readable_directly: false\n    \
                kind: container\n    \
                depends_on: []\n"
        ));
    }
    let body = format!(
        "schema_version: 1\n\
         namespace: {ns}\n\
         host: {ns}.example.invalid\n\
         discovered_at: 2099-01-01T00:00:00+00:00\n\
         remote_tooling:\n  \
           rg: false\n  \
           jq: false\n  \
           journalctl: false\n  \
           sed: false\n  \
           grep: true\n  \
           netstat: false\n  \
           ss: true\n  \
           systemctl: false\n  \
           docker: true\n\
         services:\n{svc_yaml}volumes: []\nimages: []\nnetworks: []\n"
    );
    let path = dir.join(format!("{ns}.yaml"));
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn standard_setup(sb: &Sandbox) {
    write_servers_toml(sb.home(), "arte");
    write_profile(sb.home(), "arte", &[("pulse", "luminary-pulse")]);
}

// -----------------------------------------------------------------------------
// P6: `inspect run` (read-only)
// -----------------------------------------------------------------------------

/// `inspect run` exits cleanly and streams output. No `--apply`, no
/// audit entry written.
#[test]
fn p6_run_streams_and_exits_zero() {
    let mock = json!([
        { "match": "docker exec 'luminary-pulse'", "stdout": "ok-line\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    sb.cmd()
        .args(["run", "arte/pulse", "--", "echo", "ok"])
        .assert()
        .success()
        .stdout(contains("ok-line"));
}

/// `inspect run` is NOT audited. The audit dir should remain empty
/// (no jsonl file created) after a successful run invocation.
#[test]
fn p6_run_writes_no_audit_entry() {
    let mock = json!([
        { "match": "docker exec 'luminary-pulse'", "stdout": "x\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    sb.cmd()
        .args(["run", "arte/pulse", "--", "true"])
        .assert()
        .success();
    let audit_dir = sb.home().join("audit");
    if audit_dir.exists() {
        let entries: Vec<_> = std::fs::read_dir(&audit_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
            .collect();
        assert!(
            entries.is_empty(),
            "expected no audit jsonl, got {entries:?}"
        );
    }
}

/// `--reason` on `inspect run` is informational; we echo it to stderr
/// at the start so terminal/shell history captures the operator's
/// intent. Validate the prefix is present.
#[test]
fn p6_run_reason_echoed_to_stderr() {
    let mock = json!([
        { "match": "docker exec 'luminary-pulse'", "stdout": "x\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    sb.cmd()
        .args([
            "run",
            "arte/pulse",
            "--reason",
            "INC-123 investigating drift",
            "--",
            "true",
        ])
        .assert()
        .success()
        .stderr(contains("# reason: INC-123 investigating drift"));
}

// -----------------------------------------------------------------------------
// P7: --allow-exec gone
// -----------------------------------------------------------------------------

#[test]
fn p7_exec_rejects_allow_exec_flag() {
    let mock = json!([]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    // Place `--allow-exec` BEFORE the selector so clap parses it as a
    // flag (not as a trailing-var-arg cmd token).
    sb.cmd()
        .args(["exec", "--allow-exec", "arte/pulse", "--apply", "--", "id"])
        .assert()
        .failure()
        .stderr(contains("--allow-exec").or(contains("unexpected argument")));
}

// -----------------------------------------------------------------------------
// P12: --reason end-to-end
// -----------------------------------------------------------------------------

#[test]
fn p12_exec_records_reason_in_audit() {
    let mock = json!([
        { "match": "docker exec 'luminary-pulse'", "stdout": "ok\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    sb.cmd()
        .args([
            "exec",
            "arte/pulse",
            "--apply",
            "--yes",
            "--reason",
            "INC-42 rotate token",
            "--",
            "echo",
            "ok",
        ])
        .assert()
        .success();

    sb.cmd()
        .args(["audit", "ls"])
        .assert()
        .success()
        .stdout(contains("INC-42 rotate token"));
}

#[test]
fn p12_audit_ls_filter_by_reason() {
    let mock = json!([
        { "match": "docker exec 'luminary-pulse'", "stdout": "ok\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    // First call: a reason that should match the filter.
    sb.cmd()
        .args([
            "exec",
            "arte/pulse",
            "--apply",
            "--yes",
            "--reason",
            "ROTATE secrets",
            "--",
            "echo",
            "ok",
        ])
        .assert()
        .success();
    // Second call: a reason that should be filtered out.
    sb.cmd()
        .args([
            "exec",
            "arte/pulse",
            "--apply",
            "--yes",
            "--reason",
            "investigation",
            "--",
            "echo",
            "ok",
        ])
        .assert()
        .success();

    sb.cmd()
        .args(["audit", "ls", "--reason", "rotate"])
        .assert()
        .success()
        .stdout(contains("ROTATE secrets"))
        .stdout(contains("investigation").not());
}

#[test]
fn p12_reason_too_long_is_rejected() {
    let mock = json!([
        { "match": "docker exec 'luminary-pulse'", "stdout": "ok\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    let big = "x".repeat(241);
    sb.cmd()
        .args([
            "exec",
            "arte/pulse",
            "--apply",
            "--yes",
            "--reason",
            &big,
            "--",
            "echo",
            "ok",
        ])
        .assert()
        .failure()
        .stderr(contains("240"));
}

// -----------------------------------------------------------------------------
// P3: --match / --exclude pushdown
// -----------------------------------------------------------------------------

/// `inspect logs --match X` must inject `| grep -E -- 'X'` server-side.
#[test]
fn p3_logs_match_pushes_down_grep() {
    // Mock entry matches the COMBINED command so we can assert the
    // pipeline got built. Match key includes `grep -E -- 'error'`.
    let mock = json!([
        {
            "match": "docker logs 'luminary-pulse' 2>&1 | grep -E -- 'error'",
            "stdout": "2025-01-01 error: boom\n",
            "exit": 0
        }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    sb.cmd()
        .args(["logs", "arte/pulse", "--match", "error"])
        .assert()
        .success()
        .stdout(contains("error: boom"));
}

/// `--exclude X` pushes down `| grep -vE -- 'X'`.
#[test]
fn p3_logs_exclude_pushes_down_grep_v() {
    let mock = json!([
        {
            "match": "| grep -vE -- 'healthcheck'",
            "stdout": "real line\n",
            "exit": 0
        }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    sb.cmd()
        .args(["logs", "arte/pulse", "--exclude", "healthcheck"])
        .assert()
        .success()
        .stdout(contains("real line"));
}

/// Multiple `--match` flags OR together: `(?:a)|(?:b)`.
#[test]
fn p3_logs_match_repeated_uses_alternation() {
    let mock = json!([
        {
            "match": "grep -E -- '(?:err)|(?:warn)'",
            "stdout": "line a\n",
            "exit": 0
        }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    sb.cmd()
        .args(["logs", "arte/pulse", "--match", "err", "--match", "warn"])
        .assert()
        .success()
        .stdout(contains("line a"));
}

// -----------------------------------------------------------------------------
// P10: --since-last cursor round-trip
// -----------------------------------------------------------------------------

#[test]
fn p10_since_last_creates_cursor_file() {
    let mock = json!([
        // First-call fallback: `--since-last` with no cursor expands
        // to `--since 5m`. We match on the `luminary-pulse` portion
        // alone so any since/until variant counts as success.
        { "match": "luminary-pulse", "stdout": "a\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    sb.cmd()
        .args(["logs", "arte/pulse", "--since-last"])
        .assert()
        .success();
    let cursor = sb.home().join("cursors").join("arte").join("pulse.kv");
    assert!(
        cursor.exists(),
        "cursor file not created at {}",
        cursor.display()
    );
    let body = std::fs::read_to_string(&cursor).unwrap();
    assert!(body.contains("ns=arte"));
    assert!(body.contains("service=pulse"));
    assert!(body.contains("last_call="));
}

#[test]
fn p10_reset_cursor_removes_file() {
    let mock = json!([
        { "match": "luminary-pulse", "stdout": "a\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    sb.cmd()
        .args(["logs", "arte/pulse", "--since-last"])
        .assert()
        .success();
    let cursor = sb.home().join("cursors").join("arte").join("pulse.kv");
    assert!(cursor.exists());
    sb.cmd()
        .args(["logs", "arte/pulse", "--reset-cursor"])
        .assert()
        .success();
    assert!(!cursor.exists(), "cursor not deleted after --reset-cursor");
}

#[test]
fn p10_since_and_since_last_conflict() {
    let mock = json!([]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    sb.cmd()
        .args(["logs", "arte/pulse", "--since", "5m", "--since-last"])
        .assert()
        .failure();
}
