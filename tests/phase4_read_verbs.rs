//! Phase 4 surface tests: read verbs against a mocked remote.
//!
//! No real SSH is required. The binary honors `INSPECT_MOCK_REMOTE_FILE`
//! to short-circuit `run_remote` with deterministic responses.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

use assert_cmd::Command;
use predicates::prelude::*;
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
    rg: bool,
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
        "schema_version: 1\nnamespace: {ns}\nhost: {ns}.example.invalid\ndiscovered_at: 2099-01-01T00:00:00+00:00\nremote_tooling:\n  rg: {rg}\n  jq: false\n  journalctl: false\n  sed: false\n  grep: true\n  netstat: false\n  ss: true\n  systemctl: false\n  docker: true\nservices:\n{svc_yaml}volumes: []\nimages: []\nnetworks: []\n"
    );
    let path = dir.join(format!("{ns}.yaml"));
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let _ = BTreeMap::<&str, &str>::new(); // silence
}

// -----------------------------------------------------------------------------
// Verbs
// -----------------------------------------------------------------------------

#[test]
fn ps_renders_running_containers() {
    let mock = json!([
        { "match": "docker ps", "stdout": "{\"Names\":\"pulse\",\"Image\":\"luminary/pulse:1\",\"Status\":\"Up 3h\"}\n{\"Names\":\"atlas\",\"Image\":\"luminary/atlas:1\",\"Status\":\"Up 1h\"}\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(
        sb.home(),
        "arte",
        &[
            ("pulse", "luminary/pulse:1", "ok"),
            ("atlas", "luminary/atlas:1", "ok"),
        ],
        false,
    );
    sb.cmd()
        .args(["ps", "arte"])
        .assert()
        .success()
        .stdout(contains("pulse"))
        .stdout(contains("atlas"))
        .stdout(contains("2 container(s)"));
}

#[test]
fn ps_json_emits_envelopes() {
    let mock = json!([
        { "match": "docker ps", "stdout": "{\"Names\":\"pulse\",\"Image\":\"luminary/pulse:1\",\"Status\":\"Up 3h\"}\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(
        sb.home(),
        "arte",
        &[("pulse", "luminary/pulse:1", "ok")],
        false,
    );
    sb.cmd()
        .args(["ps", "arte", "--json"])
        .assert()
        .success()
        .stdout(contains("\"schema_version\":1"))
        .stdout(contains("\"_medium\":\"state\""))
        .stdout(contains("\"server\":\"arte\""));
}

#[test]
fn status_rolls_up_health() {
    let mock = json!([
        { "match": "docker ps", "stdout": "pulse\natlas\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(
        sb.home(),
        "arte",
        &[("pulse", "p:1", "ok"), ("atlas", "a:1", "unhealthy")],
        false,
    );
    sb.cmd()
        .args(["status", "arte/*"])
        .assert()
        .success()
        .stdout(contains("2 service(s)"))
        .stdout(contains("1 healthy"))
        .stdout(contains("1 unhealthy"));
}

#[test]
fn status_marks_missing_containers_down() {
    // docker ps lists only 'pulse'; profile has 'pulse' + 'atlas'.
    let mock = json!([
        { "match": "docker ps", "stdout": "pulse\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(
        sb.home(),
        "arte",
        &[("pulse", "p:1", "ok"), ("atlas", "a:1", "ok")],
        false,
    );
    sb.cmd()
        .args(["status", "arte/*"])
        .assert()
        .success()
        .stdout(contains("down"))
        .stdout(contains("1 unhealthy"));
}

#[test]
fn cat_reads_remote_file() {
    let mock = json!([
        { "match": "cat -- ", "stdout": "hello\nworld\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "a:1", "ok")], false);
    sb.cmd()
        .args(["cat", "arte/atlas:/etc/atlas.conf"])
        .assert()
        .success()
        .stdout(contains("hello"))
        .stdout(contains("world"));
}

#[test]
fn cat_requires_path() {
    let mock = json!([]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "a:1", "ok")], false);
    sb.cmd()
        .args(["cat", "arte/atlas"])
        .assert()
        .stderr(contains("requires a file path"));
}

#[test]
fn ls_lists_directory() {
    let mock = json!([
        { "match": "ls", "stdout": "a.txt\nb.txt\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "a:1", "ok")], false);
    sb.cmd()
        .args(["ls", "arte/atlas:/etc"])
        .assert()
        .success()
        .stdout(contains("a.txt"))
        .stdout(contains("b.txt"));
}

#[test]
fn find_matches_files_and_returns_zero_on_hit() {
    let mock = json!([
        { "match": "find", "stdout": "/etc/foo.conf\n/etc/bar.conf\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "a:1", "ok")], false);
    sb.cmd()
        .args(["find", "arte/atlas:/etc", "*.conf"])
        .assert()
        .success()
        .stdout(contains("foo.conf"))
        .stdout(contains("bar.conf"));
}

#[test]
fn find_no_match_returns_one() {
    let mock = json!([
        { "match": "find", "stdout": "", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "a:1", "ok")], false);
    sb.cmd()
        .args(["find", "arte/atlas:/etc", "*.nope"])
        .assert()
        .code(1);
}

#[test]
fn logs_tail_emits_lines() {
    let mock = json!([
        { "match": "docker logs", "stdout": "first\nsecond\nthird\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("pulse", "p:1", "ok")], false);
    sb.cmd()
        .args(["logs", "arte/pulse", "--tail", "10"])
        .assert()
        .success()
        .stdout(contains("first"))
        .stdout(contains("third"));
}

#[test]
fn logs_invalid_since_errors() {
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("pulse", "p:1", "ok")], false);
    sb.cmd()
        .args(["logs", "arte/pulse", "--since", "1y"])
        .assert()
        .failure()
        .stderr(contains("unknown unit"));
}

#[test]
fn grep_returns_one_on_no_match() {
    // Mock executes the full server-side pipeline; we return empty stdout
    // (so `grep` produced no matches) and the trailing `|| true` zeroes
    // the exit code. Inspect itself must still emit ExitKind::NoMatches.
    let mock = json!([
        { "match": "docker logs", "stdout": "", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("pulse", "p:1", "ok")], false);
    sb.cmd()
        .args(["grep", "BOOM", "arte/pulse"])
        .assert()
        .code(1);
}

#[test]
fn grep_finds_matches() {
    let mock = json!([
        { "match": "docker logs", "stdout": "boom-error-1\nboom-error-2\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("pulse", "p:1", "ok")], false);
    sb.cmd()
        .args(["grep", "boom", "arte/pulse", "--json"])
        .assert()
        .success()
        .stdout(contains("\"line\":\"boom-error-1\""))
        .stdout(contains("\"line\":\"boom-error-2\""));
}

#[test]
fn grep_smart_case_matches_lowercase_pattern() {
    // Mock is permissive: any "docker logs" command returns the same output.
    let mock = json!([
        { "match": "docker logs", "stdout": "ERROR boom\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("pulse", "p:1", "ok")], false);
    // We can't assert -i was passed without inspecting the cmd; run + ensure
    // no crash and exit code reflects success when mock returns matches.
    sb.cmd()
        .args(["grep", "error", "arte/pulse"])
        .assert()
        .success();
}

#[test]
fn volumes_renders_list() {
    let mock = json!([
        { "match": "docker volume ls", "stdout": "{\"Name\":\"milvus-data\",\"Driver\":\"local\"}\n{\"Name\":\"redis\",\"Driver\":\"local\"}\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("pulse", "p:1", "ok")], false);
    sb.cmd()
        .args(["volumes", "arte"])
        .assert()
        .success()
        .stdout(contains("milvus-data"))
        .stdout(contains("redis"))
        .stdout(contains("2 volume(s)"));
}

#[test]
fn images_renders_list_json() {
    let mock = json!([
        { "match": "docker images", "stdout": "{\"Repository\":\"nginx\",\"Tag\":\"1.27\",\"Size\":\"42MB\"}\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("pulse", "p:1", "ok")], false);
    sb.cmd()
        .args(["images", "arte", "--json"])
        .assert()
        .success()
        .stdout(contains("\"repo_tag\":\"nginx:1.27\""));
}

#[test]
fn network_renders_list() {
    let mock = json!([
        { "match": "docker network ls", "stdout": "{\"Name\":\"bridge\",\"Driver\":\"bridge\",\"Scope\":\"local\"}\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("pulse", "p:1", "ok")], false);
    sb.cmd()
        .args(["network", "arte"])
        .assert()
        .success()
        .stdout(contains("bridge"))
        .stdout(contains("1 network(s)"));
}

#[test]
fn ports_host_uses_ss() {
    let mock = json!([
        { "match": "ss -tlnp", "stdout": "LISTEN 0 511 *:8080 *:* users:((\"node\"))\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("pulse", "p:1", "ok")], false);
    sb.cmd()
        .args(["ports", "arte/_"])
        .assert()
        .success()
        .stdout(contains("8080"));
}

#[test]
fn health_uses_curl_when_url_present() {
    let mock = json!([
        { "match": "curl", "stdout": "200", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    // Need a service with a `health` URL — write a custom profile.
    let dir = sb.home().join("profiles");
    std::fs::create_dir_all(&dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let body = "schema_version: 1\nnamespace: arte\nhost: arte.example.invalid\ndiscovered_at: 2099-01-01T00:00:00+00:00\nremote_tooling:\n  rg: false\n  jq: false\n  journalctl: false\n  sed: false\n  grep: true\n  netstat: false\n  ss: true\n  systemctl: false\n  docker: true\nservices:\n  - name: pulse\n    container_name: pulse\n    container_id: cid-pulse\n    image: p:1\n    ports: []\n    mounts: []\n    health: http://localhost:8000/health\n    health_status: ok\n    log_readable_directly: false\n    kind: container\n    depends_on: []\nvolumes: []\nimages: []\nnetworks: []\n";
    let path = dir.join("arte.yaml");
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    sb.cmd()
        .args(["health", "arte/pulse"])
        .assert()
        .success()
        .stdout(contains("HTTP 200"))
        .stdout(contains("1 ok"));
}

#[test]
fn read_verb_no_namespace_errors() {
    let sb = Sandbox::new(json!([]));
    sb.cmd().args(["status", "arte"]).assert().failure().stderr(
        predicates::str::contains("no namespaces are configured")
            .or(contains("matched no targets")),
    );
}
