//! Phase 5 surface tests: write verbs against a mocked remote.
//!
//! All tests run with INSPECT_NON_INTERACTIVE so the safety gate's
//! confirmation paths take their non-interactive branches.

use std::sync::{Mutex, MutexGuard, OnceLock};

use assert_cmd::Command;
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
            .env("INSPECT_NON_INTERACTIVE", "1")
            .env_remove("CODESPACES");
        c
    }
    fn home(&self) -> &std::path::Path {
        self.home.path()
    }
}

fn write_servers_toml(home: &std::path::Path, names: &[&str]) {
    let mut body = String::from("schema_version = 1\n\n");
    for n in names {
        body.push_str(&format!(
            "[namespaces.{n}]\nhost = \"{n}.example.invalid\"\nuser = \"deploy\"\nport = 22\n\n"
        ));
    }
    let path = home.join("servers.toml");
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn write_profile(home: &std::path::Path, ns: &str, services: &[(&str, &str)]) {
    let dir = home.join("profiles");
    std::fs::create_dir_all(&dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut svc_yaml = String::new();
    for (name, image) in services {
        svc_yaml.push_str(&format!(
            "  - name: {name}\n    container_name: {name}\n    container_id: cid-{name}\n    image: {image}\n    ports: []\n    mounts: []\n    health_status: ok\n    log_readable_directly: false\n    kind: container\n    depends_on: []\n"
        ));
    }
    let body = format!(
        "schema_version: 1\nnamespace: {ns}\nhost: {ns}.example.invalid\ndiscovered_at: 2099-01-01T00:00:00+00:00\nremote_tooling:\n  rg: false\n  jq: false\n  journalctl: false\n  sed: true\n  grep: true\n  netstat: false\n  ss: true\n  systemctl: false\n  docker: true\nservices:\n{svc_yaml}volumes: []\nimages: []\nnetworks: []\n"
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
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("pulse", "luminary/pulse:1")]);
}

// -----------------------------------------------------------------------------
// Lifecycle: restart / stop / start / reload
// -----------------------------------------------------------------------------

#[test]
fn restart_dry_run_default_is_safe() {
    let sb = Sandbox::new(json!([]));
    standard_setup(&sb);
    sb.cmd()
        .args(["restart", "arte/pulse"])
        .assert()
        .success()
        .stdout(contains("DRY RUN"))
        .stdout(contains("arte/pulse"))
        .stdout(contains("--apply"));
}

#[test]
fn restart_apply_invokes_docker_restart() {
    let mock = json!([
        { "match": "docker restart", "stdout": "pulse\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    sb.cmd()
        .args(["restart", "arte/pulse", "--apply"])
        .assert()
        .success()
        .stdout(contains("restarted"))
        .stdout(contains("1 ok"));
}

#[test]
fn stop_apply_failure_exits_error() {
    let mock = json!([
        { "match": "docker stop", "stderr": "no such container", "exit": 1 }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    sb.cmd()
        .args(["stop", "arte/pulse", "--apply"])
        .assert()
        .failure()
        .stdout(contains("FAILED"));
}

// -----------------------------------------------------------------------------
// rm / mkdir / touch
// -----------------------------------------------------------------------------

#[test]
fn rm_dry_run_lists_target() {
    let sb = Sandbox::new(json!([]));
    standard_setup(&sb);
    sb.cmd()
        .args(["rm", "arte/pulse:/tmp/x"])
        .assert()
        .success()
        .stdout(contains("DRY RUN"));
}

#[test]
fn rm_apply_without_yes_blocks_in_non_interactive() {
    let mock = json!([
        { "match": "rm -f", "stdout": "", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    // Non-interactive + --apply + Confirm::Always policy must abort
    // unless --yes is given.
    sb.cmd()
        .args(["rm", "arte/pulse:/tmp/x", "--apply"])
        .assert()
        .failure()
        .stderr(contains("requires interactive confirmation"));
}

#[test]
fn rm_apply_with_yes_proceeds() {
    let mock = json!([
        { "match": "rm -f", "stdout": "", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    sb.cmd()
        .args(["rm", "arte/pulse:/tmp/x", "--apply", "-y"])
        .assert()
        .success();
}

#[test]
fn mkdir_apply_runs_mkdir_p() {
    let mock = json!([
        { "match": "mkdir -p", "stdout": "", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    sb.cmd()
        .args(["mkdir", "arte/pulse:/var/data/new", "--apply"])
        .assert()
        .success();
}

#[test]
fn touch_apply_runs_touch() {
    let mock = json!([
        { "match": "touch --", "stdout": "", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    sb.cmd()
        .args(["touch", "arte/pulse:/var/data/keep", "--apply"])
        .assert()
        .success();
}

// -----------------------------------------------------------------------------
// chmod / chown validation
// -----------------------------------------------------------------------------

#[test]
fn chmod_rejects_unsafe_mode() {
    let sb = Sandbox::new(json!([]));
    standard_setup(&sb);
    sb.cmd()
        .args(["chmod", "arte/pulse:/etc/x", "rm -rf /"])
        .assert()
        .failure()
        .stderr(contains("mode"));
}

#[test]
fn chmod_octal_dry_run_ok() {
    let sb = Sandbox::new(json!([]));
    standard_setup(&sb);
    sb.cmd()
        .args(["chmod", "arte/pulse:/etc/x", "0644"])
        .assert()
        .success()
        .stdout(contains("DRY RUN"));
}

#[test]
fn chown_rejects_unsafe_owner() {
    let sb = Sandbox::new(json!([]));
    standard_setup(&sb);
    sb.cmd()
        .args(["chown", "arte/pulse:/etc/x", "root; rm -rf /"])
        .assert()
        .failure()
        .stderr(contains("owner"));
}

// -----------------------------------------------------------------------------
// exec
// -----------------------------------------------------------------------------

#[test]
fn exec_requires_apply() {
    let sb = Sandbox::new(json!([]));
    standard_setup(&sb);
    sb.cmd()
        .args(["exec", "arte/pulse", "--", "echo", "hi"])
        .assert()
        .success()
        .stdout(contains("DRY RUN"));
}

#[test]
fn exec_apply_runs_command() {
    let mock = json!([
        { "match": "docker exec", "stdout": "hi\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    sb.cmd()
        .args([
            "exec",
            "arte/pulse",
            "--apply",
            "--allow-exec",
            "--",
            "echo",
            "hi",
        ])
        .assert()
        .success();
}

// -----------------------------------------------------------------------------
// edit
// -----------------------------------------------------------------------------

#[test]
fn edit_rejects_non_sed_expr() {
    let sb = Sandbox::new(json!([]));
    standard_setup(&sb);
    sb.cmd()
        .args(["edit", "arte/pulse:/etc/app.conf", "not-a-sed-expr"])
        .assert()
        .failure()
        .stderr(contains("sed"));
}

#[test]
fn edit_dry_run_renders_diff() {
    let mock = json!([
        // First match wins; the edit verb only reads via `cat`.
        { "match": "cat --", "stdout": "level=info\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    sb.cmd()
        .args(["edit", "arte/pulse:/etc/app.conf", "s/info/debug/"])
        .assert()
        .success()
        .stdout(contains("DRY RUN"))
        .stdout(contains("info"))
        .stdout(contains("debug"));
}

#[test]
fn edit_no_op_dry_run() {
    let mock = json!([
        { "match": "cat --", "stdout": "level=info\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    sb.cmd()
        .args(["edit", "arte/pulse:/etc/app.conf", "s/nope/also-nope/"])
        .assert()
        .success()
        .stdout(contains("no change"));
}

#[test]
fn edit_apply_writes_and_audits() {
    let mock = json!([
        // `cat --` for the read; `base64 -d` for the write atomic push.
        { "match": "cat --", "stdout": "level=info\n", "exit": 0 },
        { "match": "base64 -d", "stdout": "", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    sb.cmd()
        .args([
            "edit",
            "arte/pulse:/etc/app.conf",
            "s/info/debug/",
            "--apply",
        ])
        .assert()
        .success()
        .stdout(contains("edited"));
    // audit ls should now find the entry.
    sb.cmd()
        .args(["audit", "ls"])
        .assert()
        .success()
        .stdout(contains("edit"))
        .stdout(contains("arte/pulse:/etc/app.conf"));
}

// -----------------------------------------------------------------------------
// cp
// -----------------------------------------------------------------------------

#[test]
fn cp_push_dry_run_diffs() {
    let mock = json!([
        { "match": "cat --", "stdout": "old\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    let local = sb.home().join("local.txt");
    std::fs::write(&local, b"new\n").unwrap();
    sb.cmd()
        .args([
            "cp",
            local.to_str().unwrap(),
            "arte/pulse:/etc/app.conf",
            "--diff",
        ])
        .assert()
        .success()
        .stdout(contains("DRY RUN"));
}

#[test]
fn cp_pull_apply_writes_local_file() {
    let mock = json!([
        { "match": "base64 --", "stdout": "aGVsbG8K\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    let local = sb.home().join("out.txt");
    sb.cmd()
        .args([
            "cp",
            "arte/pulse:/etc/banner",
            local.to_str().unwrap(),
            "--apply",
        ])
        .assert()
        .success();
    let got = std::fs::read(&local).unwrap();
    assert_eq!(got, b"hello\n");
}

// -----------------------------------------------------------------------------
// audit
// -----------------------------------------------------------------------------

#[test]
fn audit_grep_no_match_exits_one() {
    let sb = Sandbox::new(json!([]));
    standard_setup(&sb);
    sb.cmd()
        .args(["audit", "grep", "no-such-pattern-xyz"])
        .assert()
        .code(1);
}

#[test]
fn audit_ls_empty_is_ok() {
    let sb = Sandbox::new(json!([]));
    standard_setup(&sb);
    sb.cmd().args(["audit", "ls"]).assert().success();
}

// -----------------------------------------------------------------------------
// safety: large-fanout interlock
// -----------------------------------------------------------------------------

#[test]
fn restart_large_fanout_blocks_in_non_interactive() {
    // 11 services > threshold 10; --apply without --yes-all aborts.
    let mock = json!([
        { "match": "docker restart", "stdout": "ok\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    let services: Vec<(String, String)> = (0..11)
        .map(|i| (format!("svc{i}"), "luminary/x:1".to_string()))
        .collect();
    let refs: Vec<(&str, &str)> = services
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    write_profile(sb.home(), "arte", &refs);
    sb.cmd()
        .args(["restart", "arte/*", "--apply"])
        .assert()
        .failure()
        .stderr(contains("yes-all"));
}

#[test]
fn restart_large_fanout_yes_all_proceeds() {
    let mock = json!([
        { "match": "docker restart", "stdout": "ok\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    let services: Vec<(String, String)> = (0..11)
        .map(|i| (format!("svc{i}"), "luminary/x:1".to_string()))
        .collect();
    let refs: Vec<(&str, &str)> = services
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    write_profile(sb.home(), "arte", &refs);
    sb.cmd()
        .args(["restart", "arte/*", "--apply", "--yes-all"])
        .assert()
        .success()
        .stdout(contains("11 ok"));
}
