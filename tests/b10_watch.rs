//! B10 (v0.1.2) — `inspect watch <target> --until-<kind>`.
//!
//! End-to-end surface tests driven through `INSPECT_MOCK_REMOTE_FILE`
//! so the suite stays SSH-free. Coverage:
//!
//! * Help text documents every predicate kind and the 124 timeout
//!   exit code (operator contract surface).
//! * `--until-cmd` with `--equals` exits 0 the first time the mock
//!   returns the matching value (happy path).
//! * `--until-cmd` reaches the 124 timeout when the predicate never
//!   trips, and the `[inspect] watch timed out ...` line lands on
//!   stderr (script-friendly).
//! * Mutually exclusive predicate kinds are rejected by clap.

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
        let sb = Self { _g: g, home, mock };
        sb.write_servers_toml(&["arte"]);
        sb.write_profile("arte", &[("atlas", "luminary/atlas:1")]);
        sb
    }

    fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("inspect").unwrap();
        c.env("INSPECT_HOME", self.home.path())
            .env("INSPECT_MOCK_REMOTE_FILE", self.mock.path())
            .env("INSPECT_NON_INTERACTIVE", "1")
            .env_remove("CODESPACES");
        c
    }

    fn write_servers_toml(&self, names: &[&str]) {
        let mut body = String::from("schema_version = 1\n\n");
        for n in names {
            body.push_str(&format!(
                "[namespaces.{n}]\nhost = \"{n}.example.invalid\"\nuser = \"deploy\"\nport = 22\n\n"
            ));
        }
        let path = self.home.path().join("servers.toml");
        std::fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    fn write_profile(&self, ns: &str, services: &[(&str, &str)]) {
        let dir = self.home.path().join("profiles");
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
            "schema_version: 1\nnamespace: {ns}\nhost: {ns}.example.invalid\ndiscovered_at: 2099-01-01T00:00:00+00:00\nremote_tooling:\n  rg: false\n  jq: false\n  journalctl: false\n  sed: false\n  grep: true\n  netstat: false\n  ss: true\n  systemctl: false\n  docker: true\nservices:\n{svc_yaml}volumes: []\nimages: []\nnetworks: []\n"
        );
        let path = dir.join(format!("{ns}.yaml"));
        std::fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
}

// -----------------------------------------------------------------------------
// Help surface
// -----------------------------------------------------------------------------

#[test]
fn watch_help_lists_predicate_kinds() {
    Command::cargo_bin("inspect")
        .unwrap()
        .args(["watch", "--help"])
        .assert()
        .success()
        .stdout(contains("--until-cmd"))
        .stdout(contains("--until-log"))
        .stdout(contains("--until-sql"))
        .stdout(contains("--until-http"));
}

#[test]
fn watch_help_documents_124_timeout_exit_code() {
    // The timeout exit code is part of the operator contract — if it
    // ever silently changes, every CI script reading $? will quietly
    // break. Pin it in help so the contract is visible.
    Command::cargo_bin("inspect")
        .unwrap()
        .args(["watch", "--help"])
        .assert()
        .success()
        .stdout(contains("124"));
}

#[test]
fn watch_help_lists_cmd_comparators() {
    Command::cargo_bin("inspect")
        .unwrap()
        .args(["watch", "--help"])
        .assert()
        .success()
        .stdout(contains("--equals"))
        .stdout(contains("--matches"))
        .stdout(contains("--changes"))
        .stdout(contains("--stable-for"));
}

// -----------------------------------------------------------------------------
// Mutual exclusion
// -----------------------------------------------------------------------------

#[test]
fn watch_rejects_two_predicate_kinds() {
    // clap groups should reject this at parse time with a clear message.
    let assert = Command::cargo_bin("inspect")
        .unwrap()
        .args([
            "watch",
            "arte/atlas",
            "--until-cmd",
            "true",
            "--until-log",
            "ready",
        ])
        .assert()
        .failure();
    let out = assert.get_output();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("cannot be used")
            || combined.contains("conflict")
            || combined.contains("not be used"),
        "expected clap mutual-exclusion message, got: {combined}"
    );
}

#[test]
fn watch_requires_a_predicate_kind() {
    // Plain selector with no --until-* must fail (clap group required).
    Command::cargo_bin("inspect")
        .unwrap()
        .args(["watch", "arte/atlas"])
        .assert()
        .failure();
}

#[test]
fn watch_comparator_requires_until_cmd() {
    // `--equals` without `--until-cmd` should be rejected by clap's
    // `requires` constraint.
    Command::cargo_bin("inspect")
        .unwrap()
        .args([
            "watch",
            "arte/atlas",
            "--until-log",
            "ready",
            "--equals",
            "x",
        ])
        .assert()
        .failure();
}

// -----------------------------------------------------------------------------
// --until-cmd happy path
// -----------------------------------------------------------------------------

#[test]
fn watch_until_cmd_equals_matches_immediately() {
    // Mock returns "active" for any command — first poll matches.
    let mock = json!([
        { "match": "docker exec", "stdout": "active\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    let out = sb
        .cmd()
        .args([
            "watch",
            "arte/atlas",
            "--until-cmd",
            "systemctl is-active atlas",
            "--equals",
            "active",
            "--interval",
            "1s",
            "--timeout",
            "10s",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "expected exit 0, got {:?}, stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    // The matched value is echoed to stdout so pipelines can consume it.
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("active"),
        "stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn watch_until_cmd_default_matches_on_exit_zero() {
    // No comparator → exit 0 = match.
    let mock = json!([
        { "match": "docker exec", "stdout": "", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    let out = sb
        .cmd()
        .args([
            "watch",
            "arte/atlas",
            "--until-cmd",
            "true",
            "--interval",
            "1s",
            "--timeout",
            "5s",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// -----------------------------------------------------------------------------
// --until-cmd timeout → exit 124
// -----------------------------------------------------------------------------

#[test]
fn watch_until_cmd_timeout_exits_124() {
    // Mock returns "pending" — never matches "ready" → must time out.
    let mock = json!([
        { "match": "docker exec", "stdout": "pending\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    let out = sb
        .cmd()
        .args([
            "watch",
            "arte/atlas",
            "--until-cmd",
            "echo pending",
            "--equals",
            "ready",
            "--interval",
            "1s",
            "--timeout",
            "2s",
        ])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(124),
        "expected timeout exit 124, got {:?}, stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("timed out"),
        "expected timeout marker on stderr, got {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
