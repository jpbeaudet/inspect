//! Phase 6 surface tests: `inspect search '<query>'` parses correctly,
//! emits SUMMARY/DATA/NEXT human output and stable JSON envelopes, and
//! produces actionable diagnostics for malformed queries.

use std::sync::{Mutex, MutexGuard, OnceLock};

use assert_cmd::Command;
use predicates::str::contains;
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
}
impl Sandbox {
    fn new() -> Self {
        let g = lock();
        let home = tempfile::tempdir().unwrap();
        Self { _g: g, home }
    }
    fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("inspect").unwrap();
        c.env("INSPECT_HOME", self.home.path())
            .env("INSPECT_NON_INTERACTIVE", "1")
            .env_remove("CODESPACES");
        c
    }
}

#[test]
fn parses_simple_log_query() {
    let sb = Sandbox::new();
    sb.cmd()
        .args(["search", r#"{server="arte", source="logs"} |= "error""#])
        .assert()
        .success()
        .stdout(contains("parsed log query OK"))
        .stdout(contains("SUMMARY"));
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
        .stdout(contains("parsed metric query OK"));
}

#[test]
fn json_output_has_stable_envelope() {
    let sb = Sandbox::new();
    let out = sb
        .cmd()
        .args(["search", r#"{source="logs"} |= "x""#, "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["data"]["kind"], "log");
    assert_eq!(v["data"]["branches"], 1);
}

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
        .stderr(contains("^"));
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
        .stdout(contains("metric"));
}

#[test]
fn map_stage_parses() {
    let sb = Sandbox::new();
    sb.cmd()
        .args([
            "search",
            r#"{server="arte", source="logs"} |= "milvus" | json | map { {server="arte", service="$service", source=~"file:.*"} |~ "milvus" }"#,
        ])
        .assert()
        .success();
}

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

#[test]
fn alias_substitution_via_inspect_alias() {
    let sb = Sandbox::new();
    sb.cmd()
        .args(["alias", "add", "plogs", r#"{server="arte", source="logs"}"#])
        .assert()
        .success();
    sb.cmd()
        .args(["search", r#"@plogs |= "x""#])
        .assert()
        .success()
        .stdout(contains("parsed log query OK"));
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
