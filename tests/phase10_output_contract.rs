//! Phase 10 surface tests: unified output contract (bible §10/§11) +
//! correlation rules.
//!
//! Every aggregate command (`status`, `health`, `why`, `connectivity`,
//! `recipe`, `search`) must emit a single command-level JSON envelope
//! shaped:
//!     { schema_version: u32, summary: str, data: object,
//!       next: [{cmd, rationale}], meta: object }
//! Streaming commands (`logs`, `grep`, `ps`, `volumes`, ...) use the
//! per-record §10 envelope shape carrying `schema_version`, `_source`,
//! `_medium`, `server`. Phase 10 also enforces:
//!   * `schema_version` == 1 everywhere
//!   * SUMMARY/DATA/NEXT human prefixes always present
//!   * Correlation `next[]` populated when failure conditions are met
//!     (status with unhealthy services, why with a root cause, etc.)

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
    fn new(mock_responses: Value, services: &[Svc]) -> Self {
        let g = lock();
        let home = tempfile::tempdir().unwrap();
        let mock = tempfile::Builder::new()
            .prefix("inspect-mock-")
            .suffix(".json")
            .tempfile()
            .unwrap();
        std::fs::write(mock.path(), serde_json::to_string(&mock_responses).unwrap()).unwrap();
        let sb = Self { _g: g, home, mock };
        sb.write_servers(&["arte"]);
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

    fn write_servers(&self, names: &[&str]) {
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

    fn write_profile(&self, ns: &str, services: &[Svc]) {
        let dir = self.home.path().join("profiles");
        std::fs::create_dir_all(&dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut svc_yaml = String::new();
        for s in services {
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
                "  - name: {n}\n    container_name: {n}\n    container_id: cid-{n}\n    image: ex/{n}:1\n    ports: []\n    mounts: []\n    health_status: {hs}\n    log_readable_directly: false\n    kind: container\n    depends_on: {deps}\n",
                n = s.name,
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
}

struct Svc {
    name: &'static str,
    health: &'static str,
    deps: Vec<&'static str>,
}

fn svc(name: &'static str, health: &'static str, deps: Vec<&'static str>) -> Svc {
    Svc { name, health, deps }
}

fn first_json(stdout: &[u8]) -> Value {
    let s = String::from_utf8_lossy(stdout);
    let line = s
        .lines()
        .find(|l| l.trim_start().starts_with('{'))
        .expect("no JSON line");
    serde_json::from_str(line).expect("invalid JSON")
}

/// Assert the §11 command-level envelope shape.
fn assert_envelope_shape(v: &Value) {
    assert_eq!(v["schema_version"], 1, "schema_version must be 1");
    assert!(v.get("summary").is_some(), "envelope missing `summary`");
    assert!(v["summary"].is_string(), "`summary` must be string");
    assert!(v.get("data").is_some(), "envelope missing `data`");
    assert!(v["data"].is_object(), "`data` must be object");
    if let Some(next) = v.get("next") {
        assert!(next.is_array(), "`next` must be array if present");
        for n in next.as_array().unwrap() {
            assert!(n["cmd"].is_string(), "next.cmd must be string");
            assert!(n["rationale"].is_string(), "next.rationale must be string");
        }
    }
    if let Some(meta) = v.get("meta") {
        assert!(meta.is_object(), "`meta` must be object if present");
    }
}

// -----------------------------------------------------------------------------
// Envelope shape across all aggregate commands
// -----------------------------------------------------------------------------

#[test]
fn status_emits_envelope_and_correlation_when_unhealthy() {
    let mock = json!([
        { "match": "docker ps", "stdout": "pulse\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock, &[svc("pulse", "unhealthy", vec![])]);
    let out = sb
        .cmd()
        .args(["status", "arte/*", "--json"])
        .output()
        .unwrap();
    let v = first_json(&out.stdout);
    assert_envelope_shape(&v);
    assert!(v["data"]["services"].is_array());
    assert_eq!(v["data"]["totals"]["unhealthy"], 1);
    // Correlation: an unhealthy service should produce at least one
    // `next` suggestion (e.g. `inspect health` or `inspect why`).
    let next = v["next"]
        .as_array()
        .expect("next must be present when unhealthy");
    assert!(!next.is_empty(), "expected correlation rules to fire");
    assert!(next
        .iter()
        .any(|n| n["cmd"].as_str().unwrap().contains("inspect")));
}

#[test]
fn status_emits_envelope_with_no_correlation_when_healthy() {
    let mock = json!([
        { "match": "docker ps", "stdout": "pulse\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock, &[svc("pulse", "ok", vec![])]);
    let out = sb
        .cmd()
        .args(["status", "arte/*", "--json"])
        .output()
        .unwrap();
    let v = first_json(&out.stdout);
    assert_envelope_shape(&v);
    // When everything is healthy, `next` is omitted entirely (or empty).
    let next_len = v
        .get("next")
        .and_then(|n| n.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    assert_eq!(next_len, 0, "no correlation expected when healthy: {v}");
}

#[test]
fn health_emits_envelope() {
    let mock = json!([{ "match": "docker ps", "stdout": "pulse\n", "exit": 0 }]);
    let sb = Sandbox::new(mock, &[svc("pulse", "ok", vec![])]);
    let out = sb
        .cmd()
        .args(["health", "arte/*", "--json"])
        .output()
        .unwrap();
    let v = first_json(&out.stdout);
    assert_envelope_shape(&v);
    assert!(v["data"]["probes"].is_array());
}

#[test]
fn why_emits_envelope_with_root_cause_correlation() {
    let mock = json!([{ "match": "docker ps", "stdout": "atlas\n", "exit": 0 }]);
    let sb = Sandbox::new(
        mock,
        &[
            svc("atlas", "ok", vec!["pulse"]),
            svc("pulse", "ok", vec![]), // not in `docker ps` => down
        ],
    );
    let out = sb
        .cmd()
        .args(["why", "arte/atlas", "--json"])
        .output()
        .unwrap();
    let v = first_json(&out.stdout);
    assert_envelope_shape(&v);
    assert_eq!(v["data"]["services"][0]["root_cause"], "pulse");
    let next = v["next"].as_array().unwrap();
    assert!(!next.is_empty());
    assert!(
        next.iter()
            .any(|n| n["cmd"].as_str().unwrap().contains("pulse")),
        "expected next suggestion to mention root cause: {next:?}"
    );
}

#[test]
fn connectivity_emits_envelope_with_probe_suggestion_when_not_probed() {
    let mock = json!([{ "match": "docker ps", "stdout": "atlas\n", "exit": 0 }]);
    let sb = Sandbox::new(
        mock,
        &[
            svc("atlas", "ok", vec!["pulse"]),
            svc("pulse", "ok", vec![]),
        ],
    );
    let out = sb
        .cmd()
        .args(["connectivity", "arte/atlas", "--json"])
        .output()
        .unwrap();
    let v = first_json(&out.stdout);
    assert_envelope_shape(&v);
    let next = v["next"].as_array().unwrap();
    assert!(
        next.iter()
            .any(|n| n["cmd"].as_str().unwrap().contains("--probe")),
        "expected --probe suggestion: {next:?}"
    );
}

#[test]
fn recipe_emits_envelope() {
    let mock = json!([{ "match": "docker ps", "stdout": "pulse\n", "exit": 0 }]);
    let sb = Sandbox::new(mock, &[svc("pulse", "ok", vec![])]);
    let out = sb
        .cmd()
        .args(["recipe", "health-everything", "--sel", "arte", "--json"])
        .output()
        .unwrap();
    let v = first_json(&out.stdout);
    assert_envelope_shape(&v);
    assert_eq!(v["data"]["recipe"], "health-everything");
}

#[test]
fn search_emits_envelope() {
    let mock = json!([{ "match": "docker logs", "stdout": "hit\n", "exit": 0 }]);
    let sb = Sandbox::new(mock, &[svc("pulse", "ok", vec![])]);
    let out = sb
        .cmd()
        .args([
            "search",
            r#"{server="arte", service="pulse", source="logs"}"#,
            "--json",
        ])
        .output()
        .unwrap();
    let v = first_json(&out.stdout);
    assert_envelope_shape(&v);
    assert_eq!(v["data"]["kind"], "log");
}

// -----------------------------------------------------------------------------
// Human contract: SUMMARY / DATA / NEXT must always be present
// -----------------------------------------------------------------------------

#[test]
fn human_status_has_summary_data_next() {
    let mock = json!([{ "match": "docker ps", "stdout": "pulse\n", "exit": 0 }]);
    let sb = Sandbox::new(mock, &[svc("pulse", "unhealthy", vec![])]);
    let out = sb.cmd().args(["status", "arte/*"]).output().unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("SUMMARY:"), "missing SUMMARY:\n{s}");
    assert!(s.contains("DATA:"), "missing DATA:\n{s}");
    assert!(s.contains("NEXT:"), "missing NEXT:\n{s}");
}

#[test]
fn human_health_has_summary_data_next() {
    let mock = json!([{ "match": "docker ps", "stdout": "pulse\n", "exit": 0 }]);
    let sb = Sandbox::new(mock, &[svc("pulse", "ok", vec![])]);
    let out = sb.cmd().args(["health", "arte/*"]).output().unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("SUMMARY:"));
    assert!(s.contains("DATA:"));
    assert!(s.contains("NEXT:"));
}

#[test]
fn human_why_has_summary_data_next() {
    let mock = json!([{ "match": "docker ps", "stdout": "pulse\n", "exit": 0 }]);
    let sb = Sandbox::new(mock, &[svc("pulse", "ok", vec![])]);
    let out = sb.cmd().args(["why", "arte/pulse"]).output().unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("SUMMARY:"));
    assert!(s.contains("DATA:"));
    assert!(s.contains("NEXT:"));
}

#[test]
fn human_recipe_has_summary_data_next() {
    let mock = json!([{ "match": "docker ps", "stdout": "pulse\n", "exit": 0 }]);
    let sb = Sandbox::new(mock, &[svc("pulse", "ok", vec![])]);
    let out = sb
        .cmd()
        .args(["recipe", "health-everything", "--sel", "arte"])
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("SUMMARY:"));
    assert!(s.contains("DATA:"));
    assert!(s.contains("NEXT:"));
}

// -----------------------------------------------------------------------------
// Backward compat: streaming verbs still use the §10 per-record shape.
// -----------------------------------------------------------------------------

#[test]
fn streaming_verbs_keep_per_record_envelope() {
    let mock = json!([
        { "match": "docker ps", "stdout": "pulse\tcid-pulse\tex/pulse:1\trunning\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock, &[svc("pulse", "ok", vec![])]);
    let out = sb.cmd().args(["ps", "arte", "--json"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Each line must be a valid JSON object with `schema_version`,
    // `_source`, `_medium`, `server`.
    let mut lines = stdout.lines().filter(|l| l.trim_start().starts_with('{'));
    let line = lines.next().expect("expected a JSON line from `ps`");
    let v: Value = serde_json::from_str(line).expect("invalid JSON");
    assert_eq!(v["schema_version"], 1);
    assert!(
        v.get("_source").is_some(),
        "per-record envelope missing _source: {v}"
    );
    assert!(
        v.get("_medium").is_some(),
        "per-record envelope missing _medium: {v}"
    );
    assert!(
        v.get("server").is_some(),
        "per-record envelope missing server: {v}"
    );
}
