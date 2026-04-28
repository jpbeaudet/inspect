//! Phase F (v0.1.3) acceptance tests — field-feedback regressions and
//! ergonomic gaps fixed in the v0.1.3 patch backlog. Each test is named
//! `f<N>_<short>` so the locked backlog item is obvious from the test
//! name alone.
//!
//! All tests run against the in-process mock medium (no real SSH).

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

fn write_profile(
    home: &std::path::Path,
    ns: &str,
    services: &[(&str, &str, &str)], // (name, image, health_status)
) {
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

// -----------------------------------------------------------------------------
// F1 — `inspect status <ns>` (bare namespace, no `/service`) must list every
// service in the namespace, not silently return 0. Two field users hit this
// in the first minute of v0.1.2 use; the existing phase4 status tests all
// used the explicit `arte/*` form so the regression slipped through.
// -----------------------------------------------------------------------------

#[test]
fn f1_status_bare_namespace_lists_all_services() {
    // 10-container mock host — matches the "N≥10 containers" contract in
    // the F1 backlog entry.
    let mock = json!([
        { "match": "docker ps", "stdout":
            "svc01\nsvc02\nsvc03\nsvc04\nsvc05\nsvc06\nsvc07\nsvc08\nsvc09\nsvc10\n",
            "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(
        sb.home(),
        "arte",
        &[
            ("svc01", "img/svc01:1", "ok"),
            ("svc02", "img/svc02:1", "ok"),
            ("svc03", "img/svc03:1", "ok"),
            ("svc04", "img/svc04:1", "ok"),
            ("svc05", "img/svc05:1", "ok"),
            ("svc06", "img/svc06:1", "ok"),
            ("svc07", "img/svc07:1", "ok"),
            ("svc08", "img/svc08:1", "ok"),
            ("svc09", "img/svc09:1", "ok"),
            ("svc10", "img/svc10:1", "ok"),
        ],
    );

    // Bare namespace selector — the form every field user types first.
    sb.cmd()
        .args(["status", "arte"])
        .assert()
        .success()
        .stdout(contains("10 service(s)"))
        .stdout(contains("10 healthy"));
}

#[test]
fn f1_status_bare_namespace_one_container() {
    let mock = json!([
        { "match": "docker ps", "stdout": "lonely\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("lonely", "img/lonely:1", "ok")]);

    sb.cmd()
        .args(["status", "arte"])
        .assert()
        .success()
        .stdout(contains("1 service(s)"))
        .stdout(contains("1 healthy"));
}

#[test]
fn f1_status_bare_namespace_matches_explicit_glob() {
    // Bare `arte` and explicit `arte/*` must produce equivalent service
    // counts. Guards against the bare form drifting away again.
    let mock = json!([
        { "match": "docker ps", "stdout": "alpha\nbeta\ngamma\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(
        sb.home(),
        "arte",
        &[
            ("alpha", "img/alpha:1", "ok"),
            ("beta", "img/beta:1", "unhealthy"),
            ("gamma", "img/gamma:1", "ok"),
        ],
    );

    sb.cmd()
        .args(["status", "arte"])
        .assert()
        .success()
        .stdout(contains("3 service(s)"))
        .stdout(contains("2 healthy"))
        .stdout(contains("1 unhealthy"));

    sb.cmd()
        .args(["status", "arte/*"])
        .assert()
        .success()
        .stdout(contains("3 service(s)"))
        .stdout(contains("2 healthy"))
        .stdout(contains("1 unhealthy"));
}

#[test]
fn f1_status_namespace_glob_lists_all_services_across_matches() {
    // `prod-*` (server glob, no `/`) must fan out across every matching
    // namespace's services, not collapse to host-level steps.
    let mock = json!([
        { "match": "docker ps", "stdout": "api\nworker\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["prod-eu", "prod-us"]);
    write_profile(
        sb.home(),
        "prod-eu",
        &[("api", "img/api:1", "ok"), ("worker", "img/worker:1", "ok")],
    );
    write_profile(
        sb.home(),
        "prod-us",
        &[("api", "img/api:1", "ok"), ("worker", "img/worker:1", "ok")],
    );

    sb.cmd()
        .args(["status", "prod-*"])
        .assert()
        .success()
        .stdout(contains("4 service(s)"));
}
