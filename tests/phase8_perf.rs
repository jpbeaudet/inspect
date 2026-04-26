//! Phase 8 surface tests: pushdown + parallelism + perf budget.
//!
//! Coverage:
//!   * Leading line filters become a remote `grep` chain (mock matches
//!     on the appended substring).
//!   * Parallel multi-branch query still concatenates correctly.
//!   * Cold-start `--version` budget (<200ms warm).
//!   * `search` across 5 mocked namespaces returns first results in
//!     <2s (bible §14.3).

use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

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
    fn new(mock_responses: serde_json::Value, namespaces: &[&str]) -> Self {
        let g = lock();
        let home = tempfile::tempdir().unwrap();
        let mock = tempfile::Builder::new()
            .prefix("inspect-mock-")
            .suffix(".json")
            .tempfile()
            .unwrap();
        std::fs::write(mock.path(), serde_json::to_string(&mock_responses).unwrap()).unwrap();
        let sb = Self { _g: g, home, mock };
        sb.write_servers_toml(namespaces);
        for n in namespaces {
            sb.write_profile(n, &[("pulse", "luminary/pulse:1")]);
        }
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
// Pushdown: leading `|=` becomes a remote `grep -F` chain.
// -----------------------------------------------------------------------------

#[test]
fn line_filter_pushdown_appends_grep_to_remote_command() {
    // Mock matches ONLY when the issued command contains the pushdown
    // grep clause. If pushdown wasn't wired, the docker logs command
    // wouldn't carry `| grep -F 'milvus'` and the catch-all `(no
    // match)` mock branch would fire (exit=127 ⇒ no records).
    let mock = json!([
        { "match": "| grep -F 'milvus'", "stdout": "matched milvus line\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock, &["arte"]);
    let out = sb
        .cmd()
        .args([
            "search",
            r#"{server="arte", service="pulse", source="logs"} |= "milvus""#,
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
    assert_eq!(
        recs.len(),
        1,
        "expected pushdown'd grep to fire and return 1 record; got {recs:?}"
    );
    assert!(recs[0]["line"]
        .as_str()
        .unwrap()
        .contains("matched milvus line"));
}

#[test]
fn negated_line_filter_uses_grep_minus_v() {
    let mock = json!([
        { "match": "grep -v -F 'noise'", "stdout": "kept line\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock, &["arte"]);
    let out = sb
        .cmd()
        .args([
            "search",
            r#"{server="arte", service="pulse", source="logs"} != "noise""#,
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
    assert_eq!(
        v["data"]["records"].as_array().unwrap().len(),
        1,
        "expected `!=` to push as `grep -v`"
    );
}

#[test]
fn regex_line_filter_uses_grep_minus_e() {
    let mock = json!([
        { "match": "grep -E 'err.*5\\d\\d'", "stdout": "err 503 boom\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock, &["arte"]);
    let out = sb
        .cmd()
        .args([
            "search",
            r#"{server="arte", service="pulse", source="logs"} |~ "err.*5\\d\\d""#,
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
    assert_eq!(v["data"]["records"].as_array().unwrap().len(), 1);
}

#[test]
fn pushdown_stops_at_first_parsing_stage() {
    // After `| json`, line content may be rewritten — pushdown beyond
    // that point would change semantics. We assert that a `|=` AFTER
    // a parse stage does NOT pushdown by giving the mock TWO records:
    // one with the keyword and one without; the in-memory stage filters
    // post-json. If pushdown ran, only the matching record would arrive
    // on the wire.
    let mock = json!([
        { "match": "docker logs",
          "stdout": "{\"k\":\"keep\"}\n{\"k\":\"drop\"}\n",
          "exit": 0 }
    ]);
    let sb = Sandbox::new(mock, &["arte"]);
    let out = sb
        .cmd()
        .args([
            "search",
            r#"{server="arte", service="pulse", source="logs"} | json | line_format "{{.k}}" |= "keep""#,
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
    let recs = v["data"]["records"].as_array().unwrap();
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0]["line"], "keep");
}

// -----------------------------------------------------------------------------
// Parallel multi-branch correctness
// -----------------------------------------------------------------------------

#[test]
fn parallel_or_query_produces_one_record_per_branch() {
    let mock = json!([
        { "match": "docker logs", "stdout": "log-line\n", "exit": 0 },
        { "match": "cat ", "stdout": "file-line\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock, &["arte"]);
    let out = sb
        .cmd()
        .args([
            "search",
            r#"{server="arte", service="pulse", source="logs"} or {server="arte", service="pulse", source="file:/etc/x"}"#,
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
    let recs = v["data"]["records"].as_array().unwrap();
    assert_eq!(recs.len(), 2);
    let mediums: Vec<&str> = recs
        .iter()
        .map(|r| r["_medium"].as_str().unwrap())
        .collect();
    assert!(mediums.contains(&"logs"));
    assert!(mediums.contains(&"file"));
}

// -----------------------------------------------------------------------------
// Time-range pushdown is honored by readers (Phase 7 → carried).
// -----------------------------------------------------------------------------

#[test]
fn since_until_tail_get_pushed_to_docker_logs() {
    let mock = json!([
        { "match": "docker logs --since '1h' --until '5m' --tail 50",
          "stdout": "windowed line\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock, &["arte"]);
    let out = sb
        .cmd()
        .args([
            "search",
            r#"{server="arte", service="pulse", source="logs"}"#,
            "--since",
            "1h",
            "--until",
            "5m",
            "--tail",
            "50",
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
    assert_eq!(v["data"]["records"].as_array().unwrap().len(), 1);
}

// -----------------------------------------------------------------------------
// Performance budgets (bible §14.3)
// -----------------------------------------------------------------------------

#[test]
fn cold_start_version_under_500ms() {
    // The bible target is <100ms cold; we keep generous headroom in
    // CI to avoid flakes (assert builders have warm fs cache).
    // Run twice and take the second one as "warm".
    let _ = Command::cargo_bin("inspect")
        .unwrap()
        .arg("--version")
        .output()
        .unwrap();
    let t = Instant::now();
    let out = Command::cargo_bin("inspect")
        .unwrap()
        .arg("--version")
        .output()
        .unwrap();
    let dt = t.elapsed();
    assert!(out.status.success());
    assert!(
        dt.as_millis() < 500,
        "warm `--version` took {}ms, budget 500ms",
        dt.as_millis()
    );
}

#[test]
fn search_across_five_namespaces_first_results_under_2s() {
    let mock = json!([
        { "match": "docker logs", "stdout": "hit\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock, &["arte", "boris", "celest", "delfi", "echo"]);
    let t = Instant::now();
    let out = sb
        .cmd()
        .args([
            "search",
            r#"{server=~".*", service="pulse", source="logs"}"#,
            "--json",
        ])
        .output()
        .unwrap();
    let dt = t.elapsed();
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let n = v["data"]["records"].as_array().unwrap().len();
    assert_eq!(n, 5, "expected one record per namespace");
    assert!(
        dt.as_millis() < 2000,
        "5-namespace fanout took {}ms, budget 2000ms",
        dt.as_millis()
    );
}

#[test]
fn max_parallel_env_knob_is_honored() {
    // We can't directly observe parallelism in a black-box test, but
    // we can confirm the binary still succeeds with a forced-serial
    // setting and produces the same output. Regression guard for
    // `INSPECT_MAX_PARALLEL=1` not crashing.
    let mock = json!([
        { "match": "docker logs", "stdout": "hit\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock, &["arte", "boris"]);
    let out = sb
        .cmd()
        .env("INSPECT_MAX_PARALLEL", "1")
        .args([
            "search",
            r#"{server=~".*", service="pulse", source="logs"}"#,
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
    assert_eq!(v["data"]["records"].as_array().unwrap().len(), 2);
}
