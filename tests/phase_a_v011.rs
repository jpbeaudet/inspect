//! Phase A acceptance tests for v0.1.1.
//!
//! Pins the three Phase A patches from `INSPECT_v0.1.1_PATCH_SPEC.md`
//! against regressions:
//!
//! - **P2** Phantom service names: `inspect logs arte/api` must build
//!   a `docker logs <real-container>` command, not `docker logs api`.
//! - **P1** `--follow` streaming: the streaming code path (default
//!   trait impl over the mock runner) delivers every line and exits
//!   cleanly when the remote command finishes.
//! - (P8 fallback is covered by `tests/help_contract.rs`.)

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

/// Write a profile where each service can have a distinct `name` and
/// `container_name` (the v0.1.0 phantom-service bug surface).
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

// -----------------------------------------------------------------------------
// P2: phantom service names
// -----------------------------------------------------------------------------

/// `inspect logs arte/api` must target the real container name, not
/// the user-facing service token. The mock runner only matches a
/// command containing `docker logs luminary-api`; if the build still
/// emitted `docker logs api`, the mock would return its 127 fallback
/// and the assertion below would fail.
#[test]
fn p2_logs_uses_real_container_name() {
    let mock = json!([
        {
            "match": "docker logs 'luminary-api'",
            "stdout": "2025-01-01T00:00:00Z hello from api\n2025-01-01T00:00:01Z still alive\n",
            "exit": 0
        }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), "arte");
    write_profile(sb.home(), "arte", &[("api", "luminary-api")]);

    sb.cmd()
        .args(["logs", "arte/api"])
        .assert()
        .success()
        .stdout(contains("hello from api"))
        .stdout(contains("still alive"));
}

/// Negative pin: the v0.1.0 bug shape was `docker logs <name>`, where
/// `<name>` matched the selector token. We assert the new build does
/// NOT match a mock keyed on the token alone (which would only fire
/// if the bug were present).
#[test]
fn p2_logs_does_not_use_phantom_name() {
    let mock = json!([
        // This entry would only fire if the regression came back: the
        // builder would emit `docker logs 'api' ...`, the substring
        // `docker logs 'api'` appears, mock returns "phantom" output.
        // After P2 the builder emits `docker logs 'luminary-api'`,
        // which does NOT contain `docker logs 'api'`.
        {
            "match": "docker logs 'api'",
            "stdout": "PHANTOM PHANTOM PHANTOM\n",
            "exit": 0
        },
        {
            "match": "docker logs 'luminary-api'",
            "stdout": "real-line\n",
            "exit": 0
        }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), "arte");
    write_profile(sb.home(), "arte", &[("api", "luminary-api")]);

    sb.cmd()
        .args(["logs", "arte/api"])
        .assert()
        .success()
        .stdout(contains("real-line"))
        .stdout(predicates::str::contains("PHANTOM").not());
}

/// `inspect exec` must also target the real container.
#[test]
fn p2_exec_uses_real_container_name() {
    let mock = json!([
        {
            "match": "docker exec 'luminary-api'",
            "stdout": "uid=0(root)\n",
            "exit": 0
        }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), "arte");
    write_profile(sb.home(), "arte", &[("api", "luminary-api")]);

    sb.cmd()
        .args(["exec", "arte/api", "--apply", "--yes", "--", "id"])
        .assert()
        .success()
        .stdout(contains("uid=0"));
}

/// `inspect restart` must dispatch `docker restart <real container>`.
#[test]
fn p2_restart_uses_real_container_name() {
    let mock = json!([
        {
            "match": "docker restart 'luminary-api'",
            "stdout": "luminary-api\n",
            "exit": 0
        }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), "arte");
    write_profile(sb.home(), "arte", &[("api", "luminary-api")]);

    sb.cmd()
        .args(["restart", "arte/api", "--apply", "--yes"])
        .assert()
        .success();
}

// -----------------------------------------------------------------------------
// P1: --follow streaming
// -----------------------------------------------------------------------------

/// With `--follow`, the streaming path (default `RemoteRunner::run_streaming`
/// over the mock) must still surface every line and exit cleanly.
/// Live SSH streaming is exercised separately at integration time;
/// this test pins that the verb wiring (`stream_follow` in logs.rs)
/// renders lines via the trait method.
#[test]
fn p1_follow_streams_and_exits_cleanly() {
    let mock = json!([
        {
            "match": "docker logs",
            "stdout": "line-1\nline-2\nline-3\n",
            "exit": 0
        }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), "arte");
    write_profile(sb.home(), "arte", &[("api", "luminary-api")]);

    sb.cmd()
        .args(["logs", "arte/api", "--follow"])
        .assert()
        .success()
        .stdout(contains("line-1"))
        .stdout(contains("line-2"))
        .stdout(contains("line-3"));
}

/// `--follow` JSON mode must emit one envelope per line.
#[test]
fn p1_follow_json_emits_one_envelope_per_line() {
    let mock = json!([
        {
            "match": "docker logs",
            "stdout": "alpha\nbeta\n",
            "exit": 0
        }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), "arte");
    write_profile(sb.home(), "arte", &[("api", "luminary-api")]);

    let out = sb
        .cmd()
        .args(["logs", "arte/api", "--follow", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        lines.len(),
        2,
        "expected 2 JSON envelopes, got {}: {text}",
        lines.len()
    );
    for line in &lines {
        let v: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("not JSON: {line:?}: {e}"));
        assert_eq!(v["server"], "arte");
        assert_eq!(v["_source"], "logs");
        assert_eq!(v["_medium"], "logs");
    }
}
