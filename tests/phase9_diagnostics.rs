//! Phase 9 surface tests: `why`, `connectivity`, recipe engine.

use std::sync::{Mutex, MutexGuard, OnceLock};

use assert_cmd::Command;
use serde_json::{json, Value};

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
    fn new(mock_responses: serde_json::Value, services: &[ServiceSpec]) -> Self {
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
        sb.write_profile("arte", services);
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

    fn write_profile(&self, ns: &str, services: &[ServiceSpec]) {
        let dir = self.home.path().join("profiles");
        std::fs::create_dir_all(&dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut svc_yaml = String::new();
        for s in services {
            let ports_yaml = if s.ports.is_empty() {
                "[]".to_string()
            } else {
                let mut p = String::new();
                for &(host, container, proto) in &s.ports {
                    p.push_str(&format!(
                        "\n      - host: {host}\n        container: {container}\n        proto: {proto}"
                    ));
                }
                p
            };
            let deps_yaml = if s.deps.is_empty() {
                "[]".to_string()
            } else {
                let mut d = String::new();
                for dep in &s.deps {
                    d.push_str(&format!("\n      - {dep}"));
                }
                d
            };
            svc_yaml.push_str(&format!(
                "  - name: {n}\n    container_name: {n}\n    container_id: cid-{n}\n    image: ex/{n}:1\n    ports: {ports}\n    mounts: []\n    health_status: {hs}\n    log_readable_directly: false\n    kind: container\n    depends_on: {deps}\n",
                n = s.name,
                ports = ports_yaml,
                hs = s.health,
                deps = deps_yaml,
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

    fn write_recipe_file(&self, name: &str, body: &str) {
        let dir = self.home.path().join("recipes");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.yaml"));
        std::fs::write(&path, body).unwrap();
    }
}

struct ServiceSpec {
    name: &'static str,
    health: &'static str, // ok | unhealthy | starting | unknown
    deps: Vec<&'static str>,
    ports: Vec<(u16, u16, &'static str)>,
}

fn svc(name: &'static str, health: &'static str, deps: Vec<&'static str>) -> ServiceSpec {
    ServiceSpec {
        name,
        health,
        deps,
        ports: Vec::new(),
    }
}

fn svc_with_port(
    name: &'static str,
    health: &'static str,
    deps: Vec<&'static str>,
    ports: Vec<(u16, u16, &'static str)>,
) -> ServiceSpec {
    ServiceSpec {
        name,
        health,
        deps,
        ports,
    }
}

// -----------------------------------------------------------------------------
// `why`
// -----------------------------------------------------------------------------

#[test]
fn why_walks_dependency_chain_and_marks_root_cause() {
    // atlas -> pulse -> postgres (postgres unhealthy = root cause)
    let mock = json!([
        { "match": "docker ps", "stdout": "atlas\npulse\npostgres\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(
        mock,
        &[
            svc("atlas", "ok", vec!["pulse"]),
            svc("pulse", "ok", vec!["postgres"]),
            svc("postgres", "unhealthy", vec![]),
        ],
    );
    let out = sb
        .cmd()
        .args(["why", "arte/atlas", "--json"])
        .output()
        .unwrap();
    // failing-deps present => exit 2 (ExitKind::Error)
    assert_eq!(out.status.code(), Some(2));
    let line = String::from_utf8(out.stdout).unwrap();
    let v: Value = serde_json::from_str(line.lines().next().unwrap()).unwrap();
    let svc = &v["data"]["services"][0];
    assert_eq!(svc["root_cause"], "postgres");
    let nodes = svc["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 3);
    let postgres = nodes.iter().find(|n| n["name"] == "postgres").unwrap();
    assert_eq!(postgres["status"], "unhealthy");
    assert_eq!(postgres["depth"], 2);
}

#[test]
fn why_marks_missing_container_as_down() {
    // pulse missing from `docker ps` even though it's in the profile.
    let mock = json!([
        { "match": "docker ps", "stdout": "atlas\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(
        mock,
        &[
            svc("atlas", "ok", vec!["pulse"]),
            svc("pulse", "ok", vec![]),
        ],
    );
    let out = sb
        .cmd()
        .args(["why", "arte/atlas", "--json"])
        .output()
        .unwrap();
    let v: Value = serde_json::from_str(
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let svc = &v["data"]["services"][0];
    assert_eq!(svc["root_cause"], "pulse");
    let pulse = svc["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"] == "pulse")
        .unwrap();
    assert_eq!(pulse["status"], "down");
}

#[test]
fn why_human_output_renders_tree_and_summary() {
    let mock = json!([
        { "match": "docker ps", "stdout": "atlas\npulse\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(
        mock,
        &[
            svc("atlas", "ok", vec!["pulse"]),
            svc("pulse", "ok", vec![]),
        ],
    );
    let out = sb.cmd().args(["why", "arte/atlas"]).output().unwrap();
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("SUMMARY:"));
    assert!(s.contains("DATA:"));
    assert!(s.contains("arte/atlas:"));
    assert!(s.contains("pulse: ok"));
}

// -----------------------------------------------------------------------------
// `connectivity`
// -----------------------------------------------------------------------------

#[test]
fn connectivity_lists_edges_from_depends_on() {
    let mock = json!([{ "match": "docker ps", "stdout": "", "exit": 0 }]);
    let sb = Sandbox::new(
        mock,
        &[
            svc("atlas", "ok", vec!["pulse", "postgres"]),
            svc_with_port("pulse", "ok", vec![], vec![(8000, 8000, "tcp")]),
            svc_with_port("postgres", "ok", vec![], vec![(5432, 5432, "tcp")]),
        ],
    );
    let out = sb
        .cmd()
        .args(["connectivity", "arte/atlas", "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_str(
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let edges = v["data"]["services"][0]["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 2);
    let pulse_edge = edges.iter().find(|e| e["to"] == "pulse").unwrap();
    assert_eq!(pulse_edge["from"], "atlas");
    assert_eq!(pulse_edge["port"], 8000);
    assert_eq!(pulse_edge["probed"], "skipped");
}

#[test]
fn connectivity_probe_runs_dev_tcp() {
    let mock = json!([
        { "match": "/dev/tcp/pulse/8000", "stdout": "open\n", "exit": 0 },
        { "match": "/dev/tcp/postgres/5432", "stdout": "closed\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(
        mock,
        &[
            svc("atlas", "ok", vec!["pulse", "postgres"]),
            svc_with_port("pulse", "ok", vec![], vec![(8000, 8000, "tcp")]),
            svc_with_port("postgres", "ok", vec![], vec![(5432, 5432, "tcp")]),
        ],
    );
    let out = sb
        .cmd()
        .args(["connectivity", "arte/atlas", "--probe", "--json"])
        .output()
        .unwrap();
    let v: Value = serde_json::from_str(
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let edges = v["data"]["services"][0]["edges"].as_array().unwrap();
    let pulse = edges.iter().find(|e| e["to"] == "pulse").unwrap();
    let pg = edges.iter().find(|e| e["to"] == "postgres").unwrap();
    assert_eq!(pulse["probed"], "open");
    assert_eq!(pg["probed"], "closed");
    // closed edge => exit 2 (ExitKind::Error)
    assert_eq!(out.status.code(), Some(2));
}

// -----------------------------------------------------------------------------
// Recipes
// -----------------------------------------------------------------------------

#[test]
fn recipe_runs_user_yaml_with_sel_substitution() {
    let mock = json!([{ "match": "docker ps", "stdout": "pulse\n", "exit": 0 }]);
    let sb = Sandbox::new(mock, &[svc("pulse", "ok", vec![])]);
    sb.write_recipe_file("smoke", "name: smoke\nsteps:\n  - status $SEL --json\n");
    let out = sb
        .cmd()
        .args(["recipe", "smoke", "--sel", "arte", "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_str(
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(v["data"]["recipe"], "smoke");
    let steps = v["data"]["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(
        steps[0]["argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["status", "arte", "--json"]
    );
    assert_eq!(steps[0]["exit_code"], 0);
}

#[test]
fn recipe_resolves_builtin_health_everything() {
    let mock = json!([{ "match": "docker ps", "stdout": "pulse\n", "exit": 0 }]);
    let sb = Sandbox::new(mock, &[svc("pulse", "ok", vec![])]);
    let out = sb
        .cmd()
        .args(["recipe", "health-everything", "--sel", "arte", "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_str(
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(v["data"]["recipe"], "health-everything");
    assert_eq!(v["data"]["steps"].as_array().unwrap().len(), 2);
}

#[test]
fn mutating_recipe_dry_run_by_default_does_not_append_apply() {
    let mock = json!([{ "match": "docker ps", "stdout": "pulse\n", "exit": 0 }]);
    let sb = Sandbox::new(mock, &[svc("pulse", "ok", vec![])]);
    sb.write_recipe_file(
        "rolling",
        "name: rolling\nmutating: true\nsteps:\n  - restart $SEL/pulse\n",
    );
    let out = sb
        .cmd()
        .args(["recipe", "rolling", "--sel", "arte", "--json"])
        .output()
        .unwrap();
    let v: Value = serde_json::from_str(
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let argv: Vec<String> = v["data"]["steps"][0]["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect();
    assert!(!argv.iter().any(|a| a == "--apply"), "argv={:?}", argv);
    assert_eq!(v["data"]["mutating"], true);
    assert_eq!(v["data"]["apply"], false);
}

#[test]
fn mutating_recipe_with_apply_appends_apply_to_mutating_steps_only() {
    let mock = json!([{ "match": "docker ps", "stdout": "pulse\n", "exit": 0 }]);
    let sb = Sandbox::new(mock, &[svc("pulse", "ok", vec![])]);
    sb.write_recipe_file(
        "rolling",
        "name: rolling\nmutating: true\nsteps:\n  - status $SEL --json\n  - restart $SEL/pulse\n",
    );
    let out = sb
        .cmd()
        .args(["recipe", "rolling", "--sel", "arte", "--apply", "--json"])
        .output()
        .unwrap();
    let v: Value = serde_json::from_str(
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let step0_argv: Vec<String> = v["data"]["steps"][0]["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect();
    let step1_argv: Vec<String> = v["data"]["steps"][1]["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect();
    // status is non-mutating: must NOT receive --apply
    assert!(
        !step0_argv.iter().any(|a| a == "--apply"),
        "step0={:?}",
        step0_argv
    );
    // restart is mutating: must receive --apply
    assert!(
        step1_argv.iter().any(|a| a == "--apply"),
        "step1={:?}",
        step1_argv
    );
}

#[test]
fn unknown_recipe_errors_with_builtin_list() {
    let mock = json!([]);
    let sb = Sandbox::new(mock, &[svc("pulse", "ok", vec![])]);
    let out = sb
        .cmd()
        .args(["recipe", "does-not-exist"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no recipe named"));
    assert!(stderr.contains("deploy-check"));
}
