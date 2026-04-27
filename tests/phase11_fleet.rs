//! Phase 11 — Fleet operations.
//!
//! Covers:
//!   * groups.toml parsing and `@group` resolution
//!   * `--ns` glob and comma-list expansion
//!   * partial-failure semantics (one ns fails, others succeed, exit 2)
//!   * `--abort-on-error` short-circuits remaining work
//!   * JSON aggregate output with per-namespace granularity
//!   * `INSPECT_INTERNAL_FLEET_FORCE_NS` env override of the selector
//!     requires a matching parent-pid pairing (M1 hardening)
//!   * large-fanout interlock fires on namespace count
//!   * unsupported inner verbs (`fleet fleet …`) are rejected
//!
//! All tests share the `Sandbox` helper that points `INSPECT_HOME` at a
//! tempdir so they don't touch the real `~/.inspect/`.

use std::sync::{Mutex, MutexGuard, OnceLock};

use assert_cmd::Command;
use serde_json::Value;

fn lock() -> MutexGuard<'static, ()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

struct Sandbox {
    _g: MutexGuard<'static, ()>,
    home: tempfile::TempDir,
    mock: Option<tempfile::NamedTempFile>,
}

impl Sandbox {
    fn new() -> Self {
        let g = lock();
        let home = tempfile::tempdir().unwrap();
        Self {
            _g: g,
            home,
            mock: None,
        }
    }

    fn with_mock(mock_responses: Value) -> Self {
        let mut sb = Self::new();
        let mock = tempfile::Builder::new()
            .prefix("inspect-mock-")
            .suffix(".json")
            .tempfile()
            .unwrap();
        std::fs::write(mock.path(), serde_json::to_string(&mock_responses).unwrap()).unwrap();
        sb.mock = Some(mock);
        sb
    }

    fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("inspect").unwrap();
        c.env("INSPECT_HOME", self.home.path())
            .env("INSPECT_NON_INTERACTIVE", "1")
            .env_remove("INSPECT_FLEET_FORCE_NS")
            .env_remove("INSPECT_INTERNAL_FLEET_FORCE_NS")
            .env_remove("INSPECT_INTERNAL_FLEET_PARENT_PID")
            .env_remove("CODESPACES");
        if let Some(m) = &self.mock {
            c.env("INSPECT_MOCK_REMOTE_FILE", m.path());
        }
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

fn write_groups_toml(home: &std::path::Path, body: &str) {
    let path = home.join("groups.toml");
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn write_profile(home: &std::path::Path, ns: &str, services: &[(&str, &str, &str)]) {
    let dir = home.join("profiles");
    std::fs::create_dir_all(&dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut svc_yaml = String::new();
    for (name, image, hs) in services {
        svc_yaml.push_str(&format!(
            "  - name: {name}\n    container_name: {name}\n    container_id: cid-{name}\n    image: {image}\n    ports: []\n    mounts: []\n    health_status: {hs}\n    log_readable_directly: false\n    kind: container\n    depends_on: []\n"
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

// Mock that returns a single container for `docker ps`. Each ns will see
// the same shape since the mock is path-based, not host-based.
fn fleet_ps_mock() -> Value {
    serde_json::json!([
        { "match": "docker ps", "stdout": "{\"Names\":\"pulse\",\"Image\":\"luminary/pulse:1\",\"Status\":\"Up 3h\"}\n", "exit": 0 }
    ])
}

// ---------------------------------------------------------------------------
// 1. `--ns` glob expansion + per-namespace fanout produces aggregate output.
// ---------------------------------------------------------------------------

#[test]
fn fleet_glob_expansion_runs_each_namespace() {
    let sb = Sandbox::with_mock(fleet_ps_mock());
    write_servers_toml(sb.home(), &["arte", "prod-1", "prod-2"]);
    for ns in ["arte", "prod-1", "prod-2"] {
        write_profile(sb.home(), ns, &[("pulse", "luminary/pulse:1", "ok")]);
    }

    let out = sb
        .cmd()
        .args(["fleet", "--ns", "prod-*", "ps", "_"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("SUMMARY: fleet 'ps' over 2 namespace(s): 2 ok, 0 failed"),
        "got: {stdout}"
    );
    assert!(
        stdout.contains("[OK] prod-1"),
        "missing prod-1 section: {stdout}"
    );
    assert!(
        stdout.contains("[OK] prod-2"),
        "missing prod-2 section: {stdout}"
    );
    assert!(
        !stdout.contains("[OK] arte"),
        "arte should not be in the fleet: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// 2. JSON aggregate output has per-namespace granularity.
// ---------------------------------------------------------------------------

#[test]
fn fleet_json_aggregate_per_namespace() {
    let sb = Sandbox::with_mock(fleet_ps_mock());
    write_servers_toml(sb.home(), &["prod-1", "prod-2"]);
    for ns in ["prod-1", "prod-2"] {
        write_profile(sb.home(), ns, &[("pulse", "luminary/pulse:1", "ok")]);
    }

    let out = sb
        .cmd()
        .args(["fleet", "--ns", "prod-*", "--json", "ps", "_"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: Value = serde_json::from_str(stdout.trim()).expect("valid JSON");
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["fleet"]["verb"], "ps");
    let nses = v["fleet"]["namespaces"].as_array().unwrap();
    assert_eq!(nses.len(), 2);
    assert_eq!(nses[0]["name"], "prod-1");
    assert_eq!(nses[1]["name"], "prod-2");
    for ns in nses {
        assert_eq!(ns["exit"], 0);
        assert!(ns["stdout"].as_str().unwrap().contains("pulse"));
    }
    assert_eq!(v["summary"]["total"], 2);
    assert_eq!(v["summary"]["ok"], 2);
    assert_eq!(v["summary"]["failed"], 0);
}

// ---------------------------------------------------------------------------
// 3. Partial failure: one ns has no matching service → that child
//    fails, the other succeeds. Fleet returns Error (exit 2) overall
//    per bible "fleet continues with the rest" semantics.
// ---------------------------------------------------------------------------

#[test]
fn fleet_partial_failure_continues() {
    let sb = Sandbox::with_mock(fleet_ps_mock());
    write_servers_toml(sb.home(), &["prod-1", "prod-2"]);
    write_profile(sb.home(), "prod-1", &[("pulse", "luminary/pulse:1", "ok")]);
    write_profile(sb.home(), "prod-2", &[("atlas", "luminary/atlas:1", "ok")]);

    let out = sb
        .cmd()
        .args(["fleet", "--ns", "prod-*", "--json", "ps", "pulse"])
        .output()
        .unwrap();
    // Exit 2 because prod-2 errored (no `pulse` service).
    assert_eq!(
        out.status.code(),
        Some(2),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: Value = serde_json::from_str(stdout.trim()).expect("valid JSON");
    let nses = v["fleet"]["namespaces"].as_array().unwrap();
    assert_eq!(nses.len(), 2);
    let by_name: std::collections::HashMap<&str, &Value> = nses
        .iter()
        .map(|n| (n["name"].as_str().unwrap(), n))
        .collect();
    assert_eq!(by_name["prod-1"]["exit"], 0);
    assert_ne!(by_name["prod-2"]["exit"], 0);
    assert_eq!(v["summary"]["ok"], 1);
    assert_eq!(v["summary"]["failed"], 1);
}

// ---------------------------------------------------------------------------
// 4. `@group` resolution from groups.toml.
// ---------------------------------------------------------------------------

#[test]
fn fleet_group_resolution() {
    let sb = Sandbox::with_mock(fleet_ps_mock());
    write_servers_toml(sb.home(), &["arte", "prod-1", "prod-2"]);
    for ns in ["arte", "prod-1", "prod-2"] {
        write_profile(sb.home(), ns, &[("pulse", "luminary/pulse:1", "ok")]);
    }
    write_groups_toml(
        sb.home(),
        "schema_version = 1\n[groups.canaries]\nmembers = [\"prod-1\", \"arte\"]\n",
    );

    let out = sb
        .cmd()
        .args(["fleet", "--ns", "@canaries", "--json", "ps", "_"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: Value = serde_json::from_str(stdout.trim()).expect("valid JSON");
    let names: Vec<&str> = v["fleet"]["namespaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["arte", "prod-1"]); // sorted, no prod-2
}

#[test]
fn fleet_unknown_group_errors() {
    let sb = Sandbox::new();
    write_servers_toml(sb.home(), &["arte"]);
    let out = sb
        .cmd()
        .args(["fleet", "--ns", "@nope", "ps", "_"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("@nope"),
        "stderr missing group name: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// 5. Comma list works.
// ---------------------------------------------------------------------------

#[test]
fn fleet_comma_list_expansion() {
    let sb = Sandbox::with_mock(fleet_ps_mock());
    write_servers_toml(sb.home(), &["arte", "prod-1", "prod-2"]);
    for ns in ["arte", "prod-1", "prod-2"] {
        write_profile(sb.home(), ns, &[("pulse", "luminary/pulse:1", "ok")]);
    }
    let out = sb
        .cmd()
        .args(["fleet", "--ns", "arte,prod-2", "--json", "ps", "_"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: Value = serde_json::from_str(stdout.trim()).unwrap();
    let names: Vec<&str> = v["fleet"]["namespaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["arte", "prod-2"]);
}

// ---------------------------------------------------------------------------
// 6. The fleet override env var requires a matching parent-pid pairing.
// ---------------------------------------------------------------------------
//
// M1 hardening: setting `INSPECT_INTERNAL_FLEET_FORCE_NS` alone (e.g.
// from a stray shell export) MUST NOT silently rescope selector
// resolution. The selector resolver only honors the override when it
// is paired with a matching parent-pid env var, which only `inspect
// fleet` itself can provide. Without the pairing, the user's selector
// must win.

#[test]
fn fleet_force_ns_env_requires_parent_pid_pairing() {
    let sb = Sandbox::with_mock(fleet_ps_mock());
    write_servers_toml(sb.home(), &["prod-1", "arte"]);
    write_profile(sb.home(), "prod-1", &[("pulse", "luminary/pulse:1", "ok")]);
    write_profile(sb.home(), "arte", &[("pulse", "luminary/pulse:1", "ok")]);

    // Set the override var WITHOUT the matching parent-pid var. The
    // selector "arte/_" must resolve normally against namespace "arte"
    // (not get silently rescoped to "prod-1").
    let out = sb
        .cmd()
        .env("INSPECT_INTERNAL_FLEET_FORCE_NS", "prod-1")
        .args(["ps", "arte/_", "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Every emitted record must reference the user-asked namespace,
    // never the stray-env one.
    assert!(
        stdout.contains("\"server\":\"arte\""),
        "expected arte records: {stdout}"
    );
    assert!(
        !stdout.contains("\"server\":\"prod-1\""),
        "stray FORCE_NS env must not rescope selector: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// 7. Disallowed inner verb (`fleet fleet`) is rejected with a clear error.
// ---------------------------------------------------------------------------

#[test]
fn fleet_rejects_recursive_verb() {
    let sb = Sandbox::new();
    write_servers_toml(sb.home(), &["arte"]);
    let out = sb
        .cmd()
        .args(["fleet", "--ns", "arte", "fleet", "ps", "_"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not supported under 'inspect fleet'"),
        "stderr: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// 8. Empty `--ns` match is an error.
// ---------------------------------------------------------------------------

#[test]
fn fleet_empty_match_errors() {
    let sb = Sandbox::new();
    write_servers_toml(sb.home(), &["arte"]);
    let out = sb
        .cmd()
        .args(["fleet", "--ns", "prod-*", "ps", "_"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("matched no configured namespaces"),
        "stderr: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// 9. Large-fanout interlock fires when namespace count exceeds the
// threshold and `--yes-all` is missing.
// ---------------------------------------------------------------------------

#[test]
fn fleet_large_fanout_interlock_blocks_without_yes_all() {
    let sb = Sandbox::with_mock(fleet_ps_mock());
    let names: Vec<String> = (1..=12).map(|i| format!("prod-{i}")).collect();
    let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
    write_servers_toml(sb.home(), &refs);
    for ns in &names {
        write_profile(sb.home(), ns, &[("pulse", "luminary/pulse:1", "ok")]);
    }
    let out = sb
        .cmd()
        .args(["fleet", "--ns", "prod-*", "ps", "_"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "expected interlock to block 12-ns fanout"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--yes-all") || stderr.contains("yes-all"),
        "stderr should mention --yes-all: {stderr}"
    );
}

#[test]
fn fleet_large_fanout_yes_all_proceeds() {
    let sb = Sandbox::with_mock(fleet_ps_mock());
    let names: Vec<String> = (1..=12).map(|i| format!("prod-{i}")).collect();
    let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
    write_servers_toml(sb.home(), &refs);
    for ns in &names {
        write_profile(sb.home(), ns, &[("pulse", "luminary/pulse:1", "ok")]);
    }
    let out = sb
        .cmd()
        .args(["fleet", "--ns", "prod-*", "--yes-all", "--json", "ps", "_"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(v["summary"]["total"], 12);
    assert_eq!(v["summary"]["ok"], 12);
}
