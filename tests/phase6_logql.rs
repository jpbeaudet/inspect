//! Phase 6 + Phase 7 surface tests for `inspect search '<query>'`.
//!
//! Phase 6 provided parser + diagnostics; Phase 7 wires the parsed AST
//! into source readers and emits records via the SUMMARY/DATA/NEXT
//! contract (or the stable JSON envelope under `--json`). These tests
//! exercise the full end-to-end CLI surface against a mocked remote
//! (`INSPECT_MOCK_REMOTE_FILE`) so no real SSH is required.

use std::sync::{Mutex, MutexGuard, OnceLock};

use assert_cmd::Command;
use predicates::str::contains;
use serde_json::{json, Value};

fn lock() -> MutexGuard<'static, ()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// Mock used for every test in this file. Catch-all entry returns a
/// log line containing both `error` and `milvus` so that line-filter
/// pushdown still produces records on the `|=` / `|~` path. Every
/// command boils down to a `runner.run(...)` call regardless of whether
/// it's `docker logs`, `cat /etc/foo`, `journalctl ...`, etc.
fn default_mock() -> serde_json::Value {
    json!([
        { "match": "", "stdout": "2026-04-26T00:00:00 milvus error happened in pulse\n", "exit": 0 }
    ])
}

struct Sandbox {
    _g: MutexGuard<'static, ()>,
    home: tempfile::TempDir,
    mock: tempfile::NamedTempFile,
}
impl Sandbox {
    fn new() -> Self {
        let g = lock();
        let home = tempfile::tempdir().unwrap();
        let mock = tempfile::Builder::new()
            .prefix("inspect-mock-")
            .suffix(".json")
            .tempfile()
            .unwrap();
        std::fs::write(mock.path(), serde_json::to_string(&default_mock()).unwrap()).unwrap();
        let sb = Self { _g: g, home, mock };
        sb.write_servers_toml(&["arte"]);
        sb.write_profile("arte", &[("pulse", "luminary/pulse:1")]);
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
                "  - name: {name}\n    container_id: cid-{name}\n    image: {image}\n    ports: []\n    mounts: []\n    health_status: ok\n    log_readable_directly: false\n    kind: container\n    depends_on: []\n"
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
// End-to-end log query (Phase 7)
// -----------------------------------------------------------------------------

#[test]
fn parses_simple_log_query() {
    let sb = Sandbox::new();
    sb.cmd()
        .args(["search", r#"{server="arte", source="logs"} |= "error""#])
        .assert()
        .success()
        .stdout(contains("SUMMARY"))
        .stdout(contains("record(s)"))
        .stdout(contains("DATA"))
        .stdout(contains("arte/pulse"))
        .stdout(contains("[logs]"))
        .stdout(contains("NEXT"));
}

#[test]
fn parses_metric_query() {
    let sb = Sandbox::new();
    sb.cmd()
        .args([
            "search",
            r#"count_over_time({server="arte", source="logs"} |= "error" [5m])"#,
        ])
        .assert()
        .success()
        .stdout(contains("SUMMARY"))
        .stdout(contains("series"));
}

#[test]
fn json_output_has_stable_envelope() {
    let sb = Sandbox::new();
    let out = sb
        .cmd()
        .args([
            "search",
            r#"{server="arte", source="logs"} |= "error""#,
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["data"]["kind"], "log");
    let recs = v["data"]["records"].as_array().expect("records array");
    assert!(!recs.is_empty(), "expected at least one record");
    let r0 = &recs[0];
    assert_eq!(r0["_medium"], "logs");
    assert_eq!(r0["labels"]["server"], "arte");
    assert_eq!(r0["labels"]["service"], "pulse");
    assert_eq!(r0["labels"]["source"], "logs");
    assert!(r0["line"].is_string());
}

// -----------------------------------------------------------------------------
// Parser diagnostics (Phase 6, still in force)
// -----------------------------------------------------------------------------

#[test]
fn missing_source_rejected_with_diagnostic() {
    let sb = Sandbox::new();
    sb.cmd()
        .args(["search", r#"{server="arte"} |= "x""#])
        .assert()
        .failure()
        .stderr(contains("source"));
}

#[test]
fn parse_error_renders_carat() {
    let sb = Sandbox::new();
    sb.cmd()
        .args(["search", r#"{server=}"#])
        .assert()
        .failure()
        .stderr(contains("error:"))
        .stderr(contains("^"))
        .stderr(contains("hint:"));
}

#[test]
fn parse_error_human_message_uses_friendly_token_names() {
    // Regression: error messages must NEVER leak Debug-form token
    // names like `RBrace`, `Ident("foo")`, `PipeEq`.
    let sb = Sandbox::new();
    let out = sb.cmd().args(["search", r#"{server=}"#]).output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(!err.contains("RBrace"), "leaked Debug repr in:\n{err}");
    assert!(!err.contains("PipeEq"), "leaked Debug repr in:\n{err}");
    // Should contain the friendly form `}` and a hint.
    assert!(err.contains("`}`"), "missing friendly token in:\n{err}");
    assert!(err.contains("hint:"), "missing hint in:\n{err}");
}

#[test]
fn empty_query_rejected() {
    let sb = Sandbox::new();
    sb.cmd()
        .args(["search", "  "])
        .assert()
        .failure()
        .stderr(contains("empty"));
}

// -----------------------------------------------------------------------------
// Metric aggregation surface
// -----------------------------------------------------------------------------

#[test]
fn topk_with_grouping_parses() {
    let sb = Sandbox::new();
    sb.cmd()
        .args([
            "search",
            r#"topk(5, sum by (service) (rate({server="arte", source="logs"} |= "error" [1h])))"#,
        ])
        .assert()
        .success()
        .stdout(contains("series"));
}

// -----------------------------------------------------------------------------
// `map` stage (Phase 7 cross-medium chaining)
// -----------------------------------------------------------------------------

#[test]
fn map_stage_parses() {
    let sb = Sandbox::new();
    sb.cmd()
        .args([
            "search",
            r#"{server="arte", source="logs"} |= "milvus" | json | map { {server="arte", service="$service$", source=~"file:.*"} |~ "milvus" }"#,
        ])
        .assert()
        .success()
        .stdout(contains("SUMMARY"));
}

// -----------------------------------------------------------------------------
// JSON error envelope
// -----------------------------------------------------------------------------

#[test]
fn json_error_envelope_has_error_field() {
    let sb = Sandbox::new();
    let out = sb
        .cmd()
        .args(["search", r#"{server=}"#, "--json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["data"]["error"]["message"].is_string());
}

// -----------------------------------------------------------------------------
// Alias substitution
// -----------------------------------------------------------------------------

#[test]
fn alias_substitution_via_inspect_alias() {
    let sb = Sandbox::new();
    sb.cmd()
        .args([
            "alias",
            "add",
            "plogs",
            r#"{server="arte", source="logs"}"#,
        ])
        .assert()
        .success();
    sb.cmd()
        .args(["search", r#"@plogs |= "error""#])
        .assert()
        .success()
        .stdout(contains("SUMMARY"))
        .stdout(contains("arte/pulse"));
}

#[test]
fn unknown_alias_diagnostic() {
    let sb = Sandbox::new();
    sb.cmd()
        .args(["search", r#"@nope"#])
        .assert()
        .failure()
        .stderr(contains("unknown alias"));
}
