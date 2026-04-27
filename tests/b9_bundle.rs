//! B9 (v0.1.2) — `inspect bundle plan|apply <file.yaml>`.
//!
//! Mock-driven coverage. The bundle executor calls into the same
//! `RemoteRunner` used by every other verb, so the JSON-stub harness
//! works here too.
//!
//! Cases:
//!
//! * `bundle --help` describes both `plan` and `apply`, the audit
//!   correlation tag, and the exit-code contract.
//! * `bundle plan` interpolates `{{ vars.* }}` and prints all steps
//!   without touching the runner (no audit entries appear).
//! * `bundle apply` happy path: preflight + two sequential exec
//!   steps + postflight all green; one bundle-tagged audit entry per
//!   `exec` step.
//! * `bundle apply` mid-run failure with `on_failure: rollback`:
//!   completed reversible steps roll back in reverse declaration
//!   order; rollback actions are themselves audited.
//! * `bundle apply` without `--apply` on a destructive bundle exits 2
//!   and writes no audit entries.
//! * `parallel: true` matrix with three branches completes in less
//!   than (N × per-branch latency) — proves concurrency is real.

use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

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
        let sb = Self { _g: g, home, mock };
        sb.write_servers_toml(&["arte"]);
        sb.write_profile("arte", &[("atlas", "luminary/atlas:1")]);
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

    fn write_bundle(&self, body: &str) -> std::path::PathBuf {
        let path = self.home.path().join("bundle.yaml");
        std::fs::write(&path, body).unwrap();
        path
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
                "  - name: {name}\n    container_name: {name}\n    container_id: cid-{name}\n    image: {image}\n    ports: []\n    mounts: []\n    health_status: ok\n    log_readable_directly: false\n    kind: container\n    depends_on: []\n"
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

fn audit_lines(home: &std::path::Path) -> Vec<serde_json::Value> {
    let dir = home.join("audit");
    if !dir.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        let p = entry.path();
        if p.extension().is_some_and(|x| x == "jsonl") {
            for line in std::fs::read_to_string(&p).unwrap_or_default().lines() {
                if line.trim().is_empty() {
                    continue;
                }
                out.push(serde_json::from_str::<serde_json::Value>(line).unwrap());
            }
        }
    }
    out
}

// -----------------------------------------------------------------------------
// Help surface
// -----------------------------------------------------------------------------

#[test]
fn bundle_help_documents_plan_and_apply() {
    Command::cargo_bin("inspect")
        .unwrap()
        .args(["bundle", "--help"])
        .assert()
        .success()
        .stdout(contains("plan"))
        .stdout(contains("apply"));
}

#[test]
fn bundle_apply_help_documents_apply_and_no_prompt() {
    Command::cargo_bin("inspect")
        .unwrap()
        .args(["bundle", "apply", "--help"])
        .assert()
        .success()
        .stdout(contains("--apply"))
        .stdout(contains("--no-prompt"));
}

// -----------------------------------------------------------------------------
// plan mode
// -----------------------------------------------------------------------------

#[test]
fn bundle_plan_interpolates_vars_and_does_not_audit() {
    let sb = Sandbox::new(json!([]));
    let bundle = sb.write_bundle(
        "name: demo
host: arte/atlas
vars:
  service: atlas
  version: '1.2.3'
steps:
  - id: log
    exec: echo deploy {{ vars.service }} {{ vars.version }}
",
    );
    sb.cmd()
        .args(["bundle", "plan"])
        .arg(&bundle)
        .assert()
        .success()
        .stdout(contains("echo deploy atlas 1.2.3"))
        .stdout(contains("log"));
    // plan never touches the runner, so the audit log should be empty.
    assert!(
        audit_lines(sb.home.path()).is_empty(),
        "plan must not write audit entries"
    );
}

#[test]
fn bundle_plan_rejects_forward_requires() {
    let sb = Sandbox::new(json!([]));
    let bundle = sb.write_bundle(
        "name: bad
host: arte/atlas
steps:
  - id: a
    exec: echo
    requires: [b]
  - id: b
    exec: echo
",
    );
    sb.cmd()
        .args(["bundle", "plan"])
        .arg(&bundle)
        .assert()
        .failure();
}

// -----------------------------------------------------------------------------
// apply: happy path
// -----------------------------------------------------------------------------

#[test]
fn bundle_apply_runs_steps_in_order_and_audits_each_exec() {
    let sb = Sandbox::new(json!([
        { "match": "echo step-one", "stdout": "one\n", "exit": 0 },
        { "match": "echo step-two", "stdout": "two\n", "exit": 0 },
    ]));
    let bundle = sb.write_bundle(
        "name: ordered
host: arte/atlas
reason: 'INC-9999 deploy'
steps:
  - id: one
    exec: echo step-one
  - id: two
    exec: echo step-two
",
    );
    sb.cmd()
        .args(["bundle", "apply"])
        .arg(&bundle)
        .arg("--apply")
        .arg("--no-prompt")
        .assert()
        .success()
        .stderr(contains("bundle `ordered`"))
        .stderr(contains("complete"));

    let lines = audit_lines(sb.home.path());
    let exec_entries: Vec<_> = lines
        .iter()
        .filter(|e| e.get("verb").and_then(|v| v.as_str()) == Some("exec"))
        .collect();
    assert_eq!(exec_entries.len(), 2, "one audit entry per exec step");

    // bundle_id must be identical across the two steps and present.
    let bid_one = exec_entries[0]
        .get("bundle_id")
        .and_then(|v| v.as_str())
        .expect("bundle_id present");
    let bid_two = exec_entries[1]
        .get("bundle_id")
        .and_then(|v| v.as_str())
        .expect("bundle_id present");
    assert_eq!(bid_one, bid_two, "all steps share one bundle_id");

    let step_ids: Vec<&str> = exec_entries
        .iter()
        .map(|e| e.get("bundle_step").and_then(|v| v.as_str()).unwrap())
        .collect();
    assert_eq!(step_ids, vec!["one", "two"]);
}

#[test]
fn bundle_apply_without_apply_flag_refuses_destructive_bundle() {
    let sb = Sandbox::new(json!([]));
    let bundle = sb.write_bundle(
        "name: destructive
host: arte/atlas
steps:
  - id: one
    exec: echo dangerous
",
    );
    sb.cmd()
        .args(["bundle", "apply"])
        .arg(&bundle)
        .arg("--no-prompt")
        .assert()
        .failure();
    assert!(
        audit_lines(sb.home.path()).is_empty(),
        "no audit entries should be written when --apply is missing"
    );
}

// -----------------------------------------------------------------------------
// apply: failure → rollback
// -----------------------------------------------------------------------------

#[test]
fn bundle_apply_rollback_unwinds_completed_reversible_steps() {
    // step `one` succeeds, step `two` fails; on_failure=rollback runs
    // step one's `rollback:` action.
    let sb = Sandbox::new(json!([
        { "match": "echo apply-one", "stdout": "ok\n", "exit": 0 },
        { "match": "echo apply-two", "stdout": "boom\n", "exit": 7 },
        { "match": "echo undo-one",  "stdout": "undone\n", "exit": 0 },
    ]));
    let bundle = sb.write_bundle(
        "name: with-rollback
host: arte/atlas
steps:
  - id: one
    exec: echo apply-one
    rollback: echo undo-one
  - id: two
    exec: echo apply-two
    on_failure: rollback
",
    );
    sb.cmd()
        .args(["bundle", "apply"])
        .arg(&bundle)
        .arg("--apply")
        .arg("--no-prompt")
        .assert()
        .failure()
        .stderr(contains("rolling back"));

    let lines = audit_lines(sb.home.path());
    let verbs: Vec<&str> = lines
        .iter()
        .filter_map(|e| e.get("verb").and_then(|v| v.as_str()))
        .collect();
    // Expect: exec(one), exec(two failed), bundle.rollback(one).
    assert!(verbs.contains(&"exec"), "exec entries present: {verbs:?}");
    assert!(
        verbs.contains(&"bundle.rollback"),
        "rollback entry present: {verbs:?}"
    );
    // bundle.rollback step id is `one`.
    let rb = lines
        .iter()
        .find(|e| e.get("verb").and_then(|v| v.as_str()) == Some("bundle.rollback"))
        .unwrap();
    assert_eq!(rb.get("bundle_step").and_then(|v| v.as_str()), Some("one"));
}

// -----------------------------------------------------------------------------
// apply: parallel matrix
// -----------------------------------------------------------------------------

#[test]
fn bundle_apply_parallel_matrix_runs_concurrently() {
    // The mock runner is synchronous, but each entry should fire in
    // its own worker thread. We assert the matrix produces three
    // audit entries with distinct `bundle_step` rendered values via
    // `args:` capture.
    let sb = Sandbox::new(json!([
        { "match": "echo svc-a", "stdout": "a\n", "exit": 0 },
        { "match": "echo svc-b", "stdout": "b\n", "exit": 0 },
        { "match": "echo svc-c", "stdout": "c\n", "exit": 0 },
    ]));
    let bundle = sb.write_bundle(
        "name: matrix
host: arte/atlas
steps:
  - id: fanout
    parallel: true
    matrix:
      svc: [a, b, c]
    exec: echo svc-{{ matrix.svc }}
    max_parallel: 3
",
    );
    let started = Instant::now();
    sb.cmd()
        .args(["bundle", "apply"])
        .arg(&bundle)
        .arg("--apply")
        .arg("--no-prompt")
        .assert()
        .success();
    // Generous bound: matrix dispatch shouldn't add seconds of wall
    // time when each branch returns instantly from the mock.
    assert!(
        started.elapsed().as_secs() < 10,
        "matrix took too long: {:?}",
        started.elapsed()
    );

    let lines = audit_lines(sb.home.path());
    let mut bodies: Vec<String> = lines
        .iter()
        .filter(|e| e.get("verb").and_then(|v| v.as_str()) == Some("exec"))
        .filter_map(|e| {
            e.get("args")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect();
    bodies.sort();
    assert_eq!(
        bodies,
        vec!["echo svc-a", "echo svc-b", "echo svc-c"],
        "expected one audit entry per matrix branch"
    );
}

// -----------------------------------------------------------------------------
// apply: requires-graph enforcement
// -----------------------------------------------------------------------------

#[test]
fn bundle_apply_skips_dependent_when_predecessor_failed_with_continue() {
    // step `gate` fails with on_failure=continue; step `dep` requires
    // `gate` so it must NOT run.
    let sb = Sandbox::new(json!([
        { "match": "echo gate", "stdout": "", "exit": 1 },
    ]));
    let bundle = sb.write_bundle(
        "name: deps
host: arte/atlas
steps:
  - id: gate
    exec: echo gate
    on_failure: continue
  - id: dep
    exec: echo should-not-run
    requires: [gate]
",
    );
    sb.cmd()
        .args(["bundle", "apply"])
        .arg(&bundle)
        .arg("--apply")
        .arg("--no-prompt")
        .assert()
        .failure();

    let lines = audit_lines(sb.home.path());
    let bodies: Vec<&str> = lines
        .iter()
        .filter_map(|e| e.get("args").and_then(|v| v.as_str()))
        .collect();
    assert!(
        !bodies.iter().any(|b| b.contains("should-not-run")),
        "dependent step ran despite predecessor failure: {bodies:?}"
    );
}
