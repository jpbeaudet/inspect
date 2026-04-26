//! Phase 7 surface tests: end-to-end execution across mediums.
//!
//! Drives the binary through `INSPECT_MOCK_REMOTE_FILE` (no SSH).
//! Mock entries match on a substring of the issued shell command, so
//! we can return distinct payloads for `docker logs` vs `cat`.
//!
//! Exit criteria covered:
//!   * Multi-source `or` queries work across mixed mediums.
//!   * `map` stage works on unique-label fanout and returns merged
//!     outputs.
//!   * Stable JSON envelope shape for log + metric output.

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
        sb.write_profile(
            "arte",
            &[("pulse", "luminary/pulse:1"), ("atlas", "luminary/atlas:1")],
        );
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
// Multi-source `or` across mixed mediums
// -----------------------------------------------------------------------------

#[test]
fn multi_source_or_mixes_logs_and_file() {
    // Two selector branches union into one result set: one reads
    // container logs, the other reads /etc/atlas.conf.
    let mock = json!([
        { "match": "docker logs", "stdout": "milvus error in pulse\n", "exit": 0 },
        { "match": "cat ", "stdout": "atlas.conf line one milvus\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    let out = sb
        .cmd()
        .args([
            "search",
            r#"{server="arte", service="pulse", source="logs"} or {server="arte", service="atlas", source="file:/etc/atlas.conf"} |= "milvus""#,
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
    let recs = v["data"]["records"].as_array().expect("records");
    assert_eq!(recs.len(), 2, "expected one record per branch: {recs:?}");
    let mediums: Vec<&str> = recs
        .iter()
        .map(|r| r["_medium"].as_str().unwrap())
        .collect();
    assert!(mediums.contains(&"logs"));
    assert!(mediums.contains(&"file"));
}

// -----------------------------------------------------------------------------
// `map` stage cross-medium chaining with `$field$` interpolation
// -----------------------------------------------------------------------------

#[test]
fn map_stage_runs_subquery_per_unique_field() {
    // Parent emits two records with `service` field set to "pulse"
    // and "atlas". Sub-query reads file:/svc.conf — we feed it a
    // single line back so we can prove it ran twice (once per
    // unique service tuple).
    let mock = json!([
        { "match": "docker logs",
          "stdout": "{\"service\":\"pulse\",\"msg\":\"x\"}\n{\"service\":\"atlas\",\"msg\":\"x\"}\n",
          "exit": 0 },
        { "match": "cat ", "stdout": "matched-line\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    let out = sb
        .cmd()
        .args([
            "search",
            r#"{server="arte", service="pulse", source="logs"} | json | map { {server="arte", service="$service$", source="file:/svc.conf"} }"#,
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
    let recs = v["data"]["records"].as_array().expect("records");
    // Sub-query ran once per unique service value (pulse, atlas).
    assert_eq!(recs.len(), 2, "expected 2 records, got {recs:?}");
    for r in recs {
        assert_eq!(r["_medium"], "file");
        assert_eq!(r["line"], "matched-line");
    }
    let services: Vec<&str> = recs
        .iter()
        .map(|r| r["labels"]["service"].as_str().unwrap())
        .collect();
    assert!(services.contains(&"pulse"));
    assert!(services.contains(&"atlas"));
}

// -----------------------------------------------------------------------------
// Parsed-field filter
// -----------------------------------------------------------------------------

#[test]
fn json_stage_then_field_filter_drops_below_threshold() {
    let mock = json!([
        { "match": "docker logs",
          "stdout": "{\"status\":200,\"path\":\"/a\"}\n{\"status\":500,\"path\":\"/b\"}\n{\"status\":503,\"path\":\"/c\"}\n",
          "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    let out = sb
        .cmd()
        .args([
            "search",
            r#"{server="arte", service="pulse", source="logs"} | json | status >= 500"#,
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
    let recs = v["data"]["records"].as_array().expect("records");
    assert_eq!(recs.len(), 2, "expected 500+503; got {recs:?}");
    for r in recs {
        let s = r["fields"]["status"].as_i64().unwrap();
        assert!(s >= 500);
    }
}

// -----------------------------------------------------------------------------
// `count_over_time` metric (range aggregation)
// -----------------------------------------------------------------------------

#[test]
fn count_over_time_returns_metric_samples() {
    let mock = json!([
        { "match": "docker logs", "stdout": "a\nb\nc\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    let out = sb
        .cmd()
        .args([
            "search",
            r#"count_over_time({server="arte", service="pulse", source="logs"}[5m])"#,
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
    assert_eq!(v["data"]["kind"], "metric");
    let samples = v["data"]["samples"].as_array().expect("samples");
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0]["value"], 3.0);
}

// -----------------------------------------------------------------------------
// `topk` ranking + `by`
// -----------------------------------------------------------------------------

#[test]
fn topk_truncates_to_param() {
    let mock = json!([
        { "match": "docker logs",
          "stdout": "{\"path\":\"/a\"}\n{\"path\":\"/a\"}\n{\"path\":\"/b\"}\n{\"path\":\"/b\"}\n{\"path\":\"/b\"}\n{\"path\":\"/c\"}\n",
          "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    let out = sb
        .cmd()
        .args([
            "search",
            r#"topk(2, sum by (path) (count_over_time({server="arte", service="pulse", source="logs"} | json [1h])))"#,
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
    let samples = v["data"]["samples"].as_array().expect("samples");
    assert_eq!(
        samples.len(),
        2,
        "topk(2) must return 2 series; got {samples:?}"
    );
}

// -----------------------------------------------------------------------------
// Empty result still returns valid envelope (exit code = NoMatches)
// -----------------------------------------------------------------------------

#[test]
fn empty_result_returns_no_matches_exit_code() {
    let mock = json!([
        { "match": "docker logs", "stdout": "", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    let out = sb
        .cmd()
        .args([
            "search",
            r#"{server="arte", service="pulse", source="logs"} |= "nope""#,
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "no-matches must exit 1");
}

// -----------------------------------------------------------------------------
// Discovery medium does NOT shell out
// -----------------------------------------------------------------------------

#[test]
fn discovery_source_returns_profile_services_without_remote_call() {
    // No mock entries: any remote call would yield exit=127 (mock has
    // a "no match" fallback). If discovery were to shell out, the
    // command would still succeed but produce "(mock) no match" lines.
    // We assert the records reflect the profile's services.
    let mock = json!([]);
    let sb = Sandbox::new(mock);
    let out = sb
        .cmd()
        .args(["search", r#"{server="arte", source="discovery"}"#, "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let recs = v["data"]["records"].as_array().expect("records");
    let names: Vec<&str> = recs
        .iter()
        .filter_map(|r| {
            r["fields"]["name"]
                .as_str()
                .or_else(|| r["labels"]["service"].as_str())
        })
        .collect();
    assert!(names.contains(&"pulse"), "missing pulse: {recs:?}");
    assert!(names.contains(&"atlas"), "missing atlas: {recs:?}");
}

// -----------------------------------------------------------------------------
// Human output exit-code contract
// -----------------------------------------------------------------------------

#[test]
fn human_output_includes_summary_data_next() {
    let mock = json!([
        { "match": "docker logs", "stdout": "hello\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    sb.cmd()
        .args([
            "search",
            r#"{server="arte", service="pulse", source="logs"}"#,
        ])
        .assert()
        .success()
        .stdout(contains("SUMMARY:"))
        .stdout(contains("DATA:"))
        .stdout(contains("NEXT:"));
}
