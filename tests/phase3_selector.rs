//! Phase 3 surface tests: selector parsing, alias CRUD, and end-to-end
//! resolution against a synthetic profile.
//!
//! Real SSH/discovery is NOT exercised here; we plant a hand-crafted
//! profile YAML on disk and verify that the resolve verb walks it
//! correctly. SSH lifecycle and discovery have their own phase tests.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

use assert_cmd::Command;
use predicates::prelude::*;
use predicates::str::contains;

fn lock() -> MutexGuard<'static, ()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

struct Sandbox {
    _g: MutexGuard<'static, ()>,
    home: tempfile::TempDir,
}
impl Sandbox {
    fn new() -> Self {
        let g = lock();
        let home = tempfile::tempdir().unwrap();
        // Pre-create the directory with mode 0700 so child processes that
        // re-read INSPECT_HOME find it consistent.
        Self { _g: g, home }
    }
    fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("inspect").unwrap();
        c.env("INSPECT_HOME", self.home.path());
        c.env_remove("CODESPACES");
        c
    }
    fn home(&self) -> &std::path::Path {
        self.home.path()
    }
}

fn write_servers_toml(home: &std::path::Path, names: &[&str]) {
    let mut body = String::new();
    body.push_str("schema_version = 1\n\n");
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
    services: &[&str],
    groups: BTreeMap<&str, Vec<&str>>,
) {
    let dir = home.join("profiles");
    std::fs::create_dir_all(&dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut svc_yaml = String::new();
    for s in services {
        svc_yaml.push_str(&format!(
            "  - name: {s}\n    container_name: {s}\n    container_id: cid-{s}\n    image: example/{s}:latest\n    ports: []\n    mounts: []\n    log_readable_directly: false\n    kind: container\n    depends_on: []\n"
        ));
    }
    let mut groups_yaml = String::new();
    if !groups.is_empty() {
        groups_yaml.push_str("groups:\n");
        for (k, v) in &groups {
            groups_yaml.push_str(&format!("  {k}:\n"));
            for m in v {
                groups_yaml.push_str(&format!("    - {m}\n"));
            }
        }
    }
    let body = format!(
        "schema_version: 1\nnamespace: {ns}\nhost: {ns}.example.invalid\ndiscovered_at: 2099-01-01T00:00:00+00:00\nremote_tooling:\n  rg: false\n  jq: false\n  journalctl: false\n  sed: false\n  grep: true\n  netstat: false\n  ss: false\n  systemctl: false\n  docker: true\nservices:\n{svc_yaml}volumes: []\nimages: []\nnetworks: []\n{groups_yaml}"
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
// alias CRUD
// -----------------------------------------------------------------------------

#[test]
fn alias_add_list_show_remove_round_trip() {
    let sb = Sandbox::new();

    sb.cmd()
        .args([
            "alias",
            "add",
            "plogs",
            "arte/pulse",
            "--description",
            "pulse on arte",
        ])
        .assert()
        .success()
        .stdout(contains("@plogs"))
        .stdout(contains("verb-style"));

    sb.cmd()
        .args(["alias", "list"])
        .assert()
        .success()
        .stdout(contains("@plogs"))
        .stdout(contains("arte/pulse"));

    sb.cmd()
        .args(["alias", "show", "plogs", "--json"])
        .assert()
        .success()
        .stdout(contains("\"selector\""))
        .stdout(contains("\"plogs\""));

    sb.cmd()
        .args(["alias", "remove", "plogs"])
        .assert()
        .success()
        .stdout(contains("removed"));

    sb.cmd()
        .args(["alias", "list"])
        .assert()
        .success()
        .stdout(contains("no aliases configured"));
}

#[test]
fn alias_refuses_overwrite_without_force() {
    let sb = Sandbox::new();
    sb.cmd()
        .args(["alias", "add", "a", "arte/pulse"])
        .assert()
        .success();
    sb.cmd()
        .args(["alias", "add", "a", "arte/atlas"])
        .assert()
        .failure()
        .stderr(contains("already exists"));
    sb.cmd()
        .args(["alias", "add", "a", "arte/atlas", "--force"])
        .assert()
        .success();
}

#[test]
fn alias_classifies_logql() {
    let sb = Sandbox::new();
    sb.cmd()
        .args(["alias", "add", "q", "{server=\"arte\"}"])
        .assert()
        .success()
        .stdout(contains("logql-style"));
}

#[test]
fn alias_rejects_definitional_cycle() {
    // L3 (v0.1.3): chaining is supported up to depth 5, but a
    // definitional cycle in the alias graph is rejected at
    // `alias add` time with the cycle printed back.
    let sb = Sandbox::new();
    sb.cmd()
        .args(["alias", "add", "a", "@b(x=1)"])
        .assert()
        .success();
    sb.cmd()
        .args(["alias", "add", "b", "@a(y=2)"])
        .assert()
        .failure()
        .stderr(contains("circular alias reference"));
}

#[test]
fn alias_rejects_unparseable_verb_body() {
    let sb = Sandbox::new();
    sb.cmd()
        .args(["alias", "add", "bad", "arte/"])
        .assert()
        .failure()
        .stderr(contains("cannot be parsed"));
}

// -----------------------------------------------------------------------------
// resolve verb
// -----------------------------------------------------------------------------

#[test]
fn resolve_exact_service() {
    let sb = Sandbox::new();
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(
        sb.home(),
        "arte",
        &["pulse", "atlas", "milvus-1"],
        BTreeMap::new(),
    );
    sb.cmd()
        .args(["resolve", "arte/pulse"])
        .assert()
        .success()
        .stdout(contains("arte -> service=pulse"));
}

#[test]
fn resolve_glob_matches_multiple() {
    let sb = Sandbox::new();
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(
        sb.home(),
        "arte",
        &["prod-1", "prod-2", "staging"],
        BTreeMap::new(),
    );
    sb.cmd()
        .args(["resolve", "arte/prod-*", "--json"])
        .assert()
        .success()
        .stdout(contains("\"prod-1\""))
        .stdout(contains("\"prod-2\""));
}

#[test]
fn resolve_regex_service() {
    let sb = Sandbox::new();
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(
        sb.home(),
        "arte",
        &["milvus-1", "milvus-2", "pulse"],
        BTreeMap::new(),
    );
    sb.cmd()
        .args(["resolve", "arte//milvus-\\d+/", "--json"])
        .assert()
        .success()
        .stdout(contains("\"milvus-1\""))
        .stdout(contains("\"milvus-2\""))
        .stdout(predicates::str::contains("\"pulse\"").not());
}

#[test]
fn resolve_host_with_path() {
    let sb = Sandbox::new();
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &["pulse"], BTreeMap::new());
    sb.cmd()
        .args(["resolve", "arte/_:/var/log/syslog"])
        .assert()
        .success()
        .stdout(contains("host"))
        .stdout(contains("/var/log/syslog"));
}

#[test]
fn resolve_groups_expand() {
    let sb = Sandbox::new();
    write_servers_toml(sb.home(), &["arte"]);
    let mut groups = BTreeMap::new();
    groups.insert("storage", vec!["pulse", "atlas"]);
    write_profile(sb.home(), "arte", &["pulse", "atlas", "weaver"], groups);
    sb.cmd()
        .args(["resolve", "arte/storage", "--json"])
        .assert()
        .success()
        .stdout(contains("\"pulse\""))
        .stdout(contains("\"atlas\""))
        .stdout(predicates::str::contains("\"weaver\"").not());
}

#[test]
fn resolve_subtractive_excludes() {
    let sb = Sandbox::new();
    write_servers_toml(sb.home(), &["arte", "prod", "staging"]);
    for ns in &["arte", "prod", "staging"] {
        write_profile(sb.home(), ns, &["pulse"], BTreeMap::new());
    }
    sb.cmd()
        .args(["resolve", "~staging/pulse", "--json"])
        .assert()
        .success()
        .stdout(contains("\"arte\""))
        .stdout(contains("\"prod\""))
        .stdout(predicates::str::contains("\"staging\"").not());
}

#[test]
fn resolve_unknown_alias_diagnoses() {
    let sb = Sandbox::new();
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &["pulse"], BTreeMap::new());
    sb.cmd()
        .args(["resolve", "@ghost"])
        .assert()
        .failure()
        .stderr(contains("not defined"))
        .stderr(contains("inspect alias list"));
}

#[test]
fn resolve_no_match_lists_available() {
    let sb = Sandbox::new();
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &["pulse", "atlas"], BTreeMap::new());
    sb.cmd()
        .args(["resolve", "arte/nonsuch"])
        .assert()
        .failure()
        .stderr(contains("matched no targets"))
        .stderr(contains("services available"))
        .stderr(contains("pulse"))
        .stderr(contains("atlas"));
}

#[test]
fn resolve_logql_form_in_verb_rejected() {
    let sb = Sandbox::new();
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &["pulse"], BTreeMap::new());
    sb.cmd()
        .args(["resolve", "{server=\"arte\"}"])
        .assert()
        .failure()
        .stderr(contains("LogQL"));
}

#[test]
fn resolve_alias_expansion_works_end_to_end() {
    let sb = Sandbox::new();
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &["pulse", "atlas"], BTreeMap::new());
    sb.cmd()
        .args(["alias", "add", "plogs", "arte/pulse"])
        .assert()
        .success();
    sb.cmd()
        .args(["resolve", "@plogs"])
        .assert()
        .success()
        .stdout(contains("arte -> service=pulse"));
}

#[test]
fn resolve_alias_logql_in_verb_context_errors() {
    let sb = Sandbox::new();
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &["pulse"], BTreeMap::new());
    sb.cmd()
        .args(["alias", "add", "q", "{server=\"arte\"}"])
        .assert()
        .success();
    sb.cmd()
        .args(["resolve", "@q"])
        .assert()
        .failure()
        .stderr(contains("LogQL selector"));
}
