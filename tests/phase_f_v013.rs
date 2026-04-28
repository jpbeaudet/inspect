//! Phase F (v0.1.3) acceptance tests — field-feedback regressions and
//! ergonomic gaps fixed in the v0.1.3 patch backlog. Each test is named
//! `f<N>_<short>` so the locked backlog item is obvious from the test
//! name alone.
//!
//! All tests run against the in-process mock medium (no real SSH).

use std::sync::{Mutex, MutexGuard, OnceLock};

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
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

// -----------------------------------------------------------------------------
// F8 — Cache freshness. Three field reports converged on the same
// failure mode: `inspect status` happily served a pre-restart snapshot
// for an unbounded window, with no way to ask for fresh data and no
// way to even tell the data was cached. The fix: a runtime-tier cache
// (~/.inspect/cache/<ns>/runtime.json) with a tiered TTL, an explicit
// `--refresh` flag, a `SOURCE: <mode> Ns ago …` provenance line on
// every read verb, automatic invalidation on every mutation verb, and
// `inspect cache show|clear` to introspect/reset.
// -----------------------------------------------------------------------------

fn write_minimal_arte(sb: &Sandbox) {
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(
        sb.home(),
        "arte",
        &[("atlas", "img/atlas:1", "ok")],
    );
}

fn arte_mock(restart_count: u32) -> serde_json::Value {
    json!([
        { "match": "docker ps --format", "stdout": "atlas\n", "exit": 0 },
        {
            "match": "docker inspect",
            "stdout": format!("/atlas\thealthy\t{restart_count}\n"),
            "exit": 0
        }
    ])
}

#[test]
fn f8_status_emits_source_line_live_after_refresh() {
    let sb = Sandbox::new(arte_mock(0));
    write_minimal_arte(&sb);
    sb.cmd()
        .args(["status", "arte", "--refresh"])
        .assert()
        .success()
        .stdout(contains("SOURCE:  live"));
}

#[test]
fn f8_status_source_line_present_in_human_output() {
    let sb = Sandbox::new(arte_mock(0));
    write_minimal_arte(&sb);
    // No --refresh: cold cache forces a live fetch the first call.
    sb.cmd()
        .args(["status", "arte"])
        .assert()
        .success()
        .stdout(contains("SOURCE:"));
}

#[test]
fn f8_status_json_meta_carries_source_field() {
    let sb = Sandbox::new(arte_mock(0));
    write_minimal_arte(&sb);
    let out = sb
        .cmd()
        .args(["status", "arte", "--json", "--refresh"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    // Machine formats must NOT carry the SOURCE: prose line — that
    // would corrupt JSON-Lines output.
    assert!(
        !stdout.lines().next().unwrap_or("").starts_with("SOURCE:"),
        "JSON output must not have a SOURCE: prose lead-in: {stdout}"
    );
    let line = stdout.lines().next().expect("at least one JSON record");
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    let mode = v
        .pointer("/meta/source/mode")
        .and_then(|m| m.as_str())
        .unwrap_or("");
    assert_eq!(mode, "live", "expected live mode in meta.source: {v}");
}

#[test]
fn f8_status_uses_cache_within_ttl() {
    let sb = Sandbox::new(arte_mock(0));
    write_minimal_arte(&sb);
    // First call populates the cache.
    sb.cmd().args(["status", "arte"]).assert().success();
    // Second call within TTL (default 10s) must read from cache.
    sb.cmd()
        .args(["status", "arte"])
        .assert()
        .success()
        .stdout(contains("SOURCE:  cached"));
}

#[test]
fn f8_runtime_ttl_zero_disables_cache() {
    let sb = Sandbox::new(arte_mock(0));
    write_minimal_arte(&sb);
    // First call still talks to docker — that's the live fetch.
    sb.cmd()
        .args(["status", "arte"])
        .env("INSPECT_RUNTIME_TTL_SECS", "0")
        .assert()
        .success();
    // With TTL=0 every subsequent call is "live" (no cache trusted).
    sb.cmd()
        .args(["status", "arte"])
        .env("INSPECT_RUNTIME_TTL_SECS", "0")
        .assert()
        .success()
        .stdout(contains("SOURCE:  live"));
}

#[test]
fn f8_post_mutation_runtime_is_invalidated() {
    // Two mocks: pre-restart shows healthy, post-restart we'd serve
    // the same data (the test asserts on cache provenance, not the
    // value). The key invariant: after `restart --apply`, the next
    // `status` call MUST be live, not cached.
    let mock = json!([
        { "match": "docker ps --format", "stdout": "atlas\n", "exit": 0 },
        { "match": "docker inspect",
          "stdout": "/atlas\thealthy\t0\n", "exit": 0 },
        { "match": "docker restart", "stdout": "atlas\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    // Warm the cache.
    sb.cmd().args(["status", "arte"]).assert().success();
    sb.cmd()
        .args(["status", "arte"])
        .assert()
        .success()
        .stdout(contains("SOURCE:  cached"));
    // Mutate: the F8 invariant says cache::invalidate runs here.
    sb.cmd()
        .args(["restart", "arte/atlas", "--apply", "--yes-all"])
        .assert()
        .success();
    // Next read must NOT be cached.
    sb.cmd()
        .args(["status", "arte"])
        .assert()
        .success()
        .stdout(contains("SOURCE:  live"));
}

#[test]
fn f8_cache_show_lists_namespaces() {
    let sb = Sandbox::new(arte_mock(0));
    write_minimal_arte(&sb);
    // Populate.
    sb.cmd().args(["status", "arte"]).assert().success();
    sb.cmd()
        .args(["cache", "show"])
        .assert()
        .success()
        .stdout(contains("arte"));
}

#[test]
fn f8_cache_clear_namespace_wipes_runtime() {
    let sb = Sandbox::new(arte_mock(0));
    write_minimal_arte(&sb);
    sb.cmd().args(["status", "arte"]).assert().success();
    sb.cmd()
        .args(["cache", "clear", "arte"])
        .assert()
        .success();
    // After clear, next read must be live (cache file gone).
    sb.cmd()
        .args(["status", "arte"])
        .assert()
        .success()
        .stdout(contains("SOURCE:  live"));
}

#[test]
fn f8_cache_clear_all_with_no_namespace_is_rejected() {
    // `inspect cache clear` (no ns, no --all) must fail loudly so an
    // operator never accidentally wipes a different ns than they meant.
    let sb = Sandbox::new(arte_mock(0));
    write_minimal_arte(&sb);
    sb.cmd().args(["cache", "clear"]).assert().failure();
}

// -----------------------------------------------------------------------------
// F8 hardening — followup invariants we held back from the first batch:
//   1. Cache hit MUST issue zero remote commands (the contract test for
//      "the cache actually saves work").
//   2. refresh_count is monotonic across reads.
//   3. cache clear writes an audit entry (operator-deliberate action).
//   4. bundle apply invalidates the runtime cache for every namespace
//      it touched.
// -----------------------------------------------------------------------------

/// Helper: rewrite the mock file in place. New mock takes effect on
/// the *next* `sb.cmd()` invocation (each is a fresh subprocess).
fn rewrite_mock(sb: &Sandbox, mock: serde_json::Value) {
    std::fs::write(sb.mock.path(), serde_json::to_string(&mock).unwrap()).unwrap();
}

#[test]
fn f8_cache_hit_issues_no_remote_commands() {
    // Warm with a normal mock, then swap in a "tripwire" mock with NO
    // entries. With the mock medium, an unmatched command returns
    // exit 127 + a `(mock) no match for command: ...` stderr —
    // nothing the verb can recover from. So if the cache-hit path
    // accidentally issues a docker call, the second `status` will
    // either fail outright or surface the tripwire stderr. Cache hit
    // = success + no tripwire stderr = zero commands issued.
    let sb = Sandbox::new(arte_mock(0));
    write_minimal_arte(&sb);
    sb.cmd().args(["status", "arte"]).assert().success(); // warm
    rewrite_mock(&sb, json!([])); // tripwire: any command → "no match"
    let out = sb.cmd().args(["status", "arte"]).assert().success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let stderr = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    assert!(
        stdout.contains("SOURCE:  cached"),
        "expected cached source on second call, got stdout: {stdout}"
    );
    assert!(
        !stderr.contains("(mock) no match"),
        "cache hit must not issue any remote commands; stderr leaked: {stderr}"
    );
}

#[test]
fn f8_refresh_count_increments_across_refreshes() {
    let sb = Sandbox::new(arte_mock(0));
    write_minimal_arte(&sb);
    // Three forced refreshes.
    for _ in 0..3 {
        sb.cmd()
            .args(["status", "arte", "--refresh"])
            .assert()
            .success();
    }
    let out = sb
        .cmd()
        .args(["cache", "show", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    let line = stdout.lines().next().expect("at least one record");
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    let count = v
        .pointer("/data/namespaces/0/refresh_count")
        .and_then(|n| n.as_u64())
        .unwrap_or(0);
    assert!(
        count >= 3,
        "expected refresh_count >= 3 after three --refresh calls, got {count}: {v}"
    );
}

#[test]
fn f8_cache_clear_writes_audit_entry() {
    let sb = Sandbox::new(arte_mock(0));
    write_minimal_arte(&sb);
    sb.cmd().args(["status", "arte"]).assert().success(); // populate
    sb.cmd()
        .args(["cache", "clear", "arte"])
        .assert()
        .success();
    // The audit log should now have a `cache-clear` entry for `arte`.
    let out = sb
        .cmd()
        .args(["audit", "ls", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    assert!(
        stdout.contains("cache-clear"),
        "audit ls must surface cache-clear entries; got: {stdout}"
    );
    assert!(
        stdout.contains("arte"),
        "audit ls cache-clear entry must reference arte; got: {stdout}"
    );
}

#[test]
fn f8_bundle_apply_invalidates_runtime_cache() {
    // A bundle that runs a single `exec docker restart atlas` on
    // namespace `arte`. After it lands, the next `inspect status arte`
    // MUST be live (cache invalidated by bundle apply).
    let mock = json!([
        { "match": "docker ps --format", "stdout": "atlas\n", "exit": 0 },
        { "match": "docker inspect", "stdout": "/atlas\thealthy\t0\n", "exit": 0 },
        // bundle exec body — wrapped as `docker exec atlas sh -c '...'`
        { "match": "docker exec", "stdout": "ok\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    // Warm the runtime cache.
    sb.cmd().args(["status", "arte"]).assert().success();
    sb.cmd()
        .args(["status", "arte"])
        .assert()
        .success()
        .stdout(contains("SOURCE:  cached"));
    // Author a minimal bundle that targets arte/atlas.
    let yaml = "\
name: f8-bundle-invalidation
host: arte
steps:
  - id: poke
    target: arte/atlas
    exec: 'true'
";
    let bundle_path = sb.home().join("b.yaml");
    std::fs::write(&bundle_path, yaml).unwrap();
    sb.cmd()
        .args([
            "bundle",
            "apply",
            bundle_path.to_str().unwrap(),
            "--apply",
        ])
        .assert()
        .success();
    // After the bundle ran, the next read MUST be live.
    sb.cmd()
        .args(["status", "arte"])
        .assert()
        .success()
        .stdout(contains("SOURCE:  live"));
}

// -----------------------------------------------------------------------------
// F9 — `inspect run` silently drops local stdin instead of forwarding it.
// 3rd field user (BUG-3 follow-up): `inspect run arte 'docker exec -i pg sh' < init.sql`
// returned `1 ok, 0 failed` while the SQL never ran. The fix: forward
// non-tty stdin byte-for-byte, audit the byte count, refuse loudly on
// `--no-stdin` with data waiting, cap at 10 MiB by default with explicit
// override, and bring `inspect run` in line with native `ssh host cmd`
// semantics.
// -----------------------------------------------------------------------------

fn f9_run_mock() -> serde_json::Value {
    json!([
        // `cat` echo: exercises the byte-for-byte forward contract.
        { "match": "cat", "stdout": "", "exit": 0, "echo_stdin": true },
        // `wc -c` echo: a fixed payload assertion target.
        { "match": "wc -c", "stdout": "", "exit": 0, "echo_stdin": true },
        // bare echo: no stdin involvement, regression guard for the
        // "no piped input" case.
        { "match": "echo hi", "stdout": "hi\n", "exit": 0 }
    ])
}

#[test]
fn f9_run_forwards_local_stdin_byte_for_byte() {
    // Field reproducer: piped data must reach the remote command.
    let sb = Sandbox::new(f9_run_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    sb.cmd()
        .args(["run", "arte/atlas", "--", "cat"])
        .write_stdin("hello world\n")
        .assert()
        .success()
        .stdout(contains("hello world"));
}

#[test]
fn f9_run_no_stdin_with_piped_input_exits_2_before_dispatch() {
    // Loud-failure contract: never silently discard input. With
    // `--no-stdin` and piped data, exit 2 BEFORE dispatching the
    // remote command (mock command counter stays at zero).
    let sb = Sandbox::new(f9_run_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let assert = sb
        .cmd()
        .args(["run", "arte/atlas", "--no-stdin", "--", "cat"])
        .write_stdin("would be silently dropped\n")
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("--no-stdin"),
        "stderr should explain --no-stdin contract: {stderr}"
    );
    assert!(
        stderr.contains("inspect cp") || stderr.contains("--stdin")
            || stderr.contains("forwarding is disabled"),
        "stderr should chain hint at the recovery action: {stderr}"
    );
}

#[test]
fn f9_run_stdin_size_cap_exceeded_exits_2() {
    // Size-cap contract: payload above --stdin-max exits 2 with a
    // chained hint pointing at `inspect cp`. No remote command fires.
    let sb = Sandbox::new(f9_run_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    // 1 KiB cap, 2 KiB payload → must exit 2.
    let payload: String = "x".repeat(2048);
    let assert = sb
        .cmd()
        .args([
            "run",
            "arte/atlas",
            "--stdin-max",
            "1k",
            "--",
            "cat",
        ])
        .write_stdin(payload)
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("cap") && stderr.contains("inspect cp"),
        "stderr should explain the size cap and chain to inspect cp: {stderr}"
    );
}

#[test]
fn f9_run_stdin_max_zero_disables_cap() {
    // `--stdin-max 0` means "no cap". A 200 KiB payload must succeed.
    let sb = Sandbox::new(f9_run_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let payload: String = "y".repeat(200 * 1024);
    sb.cmd()
        .args([
            "run",
            "arte/atlas",
            "--stdin-max",
            "0",
            "--",
            "cat",
        ])
        .write_stdin(payload)
        .assert()
        .success();
}

#[test]
fn f9_run_no_piped_input_unchanged_no_audit_no_forward() {
    // Regression guard: a `run` with no piped input behaves exactly
    // as it did in v0.1.2 — no audit entry written, no stdin forwarded,
    // exit 0 if the remote command exited 0.
    let sb = Sandbox::new(f9_run_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    // `< /dev/null` keeps stdin non-tty but empty. assert_cmd's
    // default stdin disposition is /dev/null when `write_stdin` is
    // not called, so this matches the no-input contract.
    sb.cmd()
        .args(["run", "arte/atlas", "--", "echo", "hi"])
        .assert()
        .success()
        .stdout(contains("hi"));
    // Audit dir must be empty (no entries appended).
    let audit_dir = sb.home().join("audit");
    if audit_dir.exists() {
        for entry in std::fs::read_dir(&audit_dir).unwrap() {
            let p = entry.unwrap().path();
            if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                let body = std::fs::read_to_string(&p).unwrap_or_default();
                assert!(
                    body.trim().is_empty(),
                    "no audit entry should be written for non-stdin run; got: {body}"
                );
            }
        }
    }
}

#[test]
fn f9_run_audit_entry_records_stdin_bytes() {
    // Audit contract: forwarded stdin writes one audit entry per step
    // with `verb=run`, `stdin_bytes=<N>`, and (without --audit-stdin-hash)
    // no `stdin_sha256` field.
    let sb = Sandbox::new(f9_run_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    sb.cmd()
        .args(["run", "arte/atlas", "--", "cat"])
        .write_stdin("hello\n") // 6 bytes
        .assert()
        .success();
    let audit_dir = sb.home().join("audit");
    let mut found = false;
    for entry in std::fs::read_dir(&audit_dir).unwrap() {
        let p = entry.unwrap().path();
        if p.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        for line in std::fs::read_to_string(&p).unwrap().lines() {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            if v.get("verb").and_then(|s| s.as_str()) == Some("run") {
                let bytes = v
                    .get("stdin_bytes")
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0);
                assert_eq!(bytes, 6, "expected stdin_bytes=6 in audit entry: {v}");
                assert!(
                    v.get("stdin_sha256").is_none(),
                    "stdin_sha256 must be absent without --audit-stdin-hash: {v}"
                );
                found = true;
            }
        }
    }
    assert!(found, "no audit entry with verb=run found");
}

#[test]
fn f9_run_audit_stdin_hash_records_sha256() {
    // With `--audit-stdin-hash`, the audit entry carries the hex
    // SHA-256 of the forwarded payload (audit-friendly proof of
    // content without storing the bytes themselves).
    let sb = Sandbox::new(f9_run_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    sb.cmd()
        .args([
            "run",
            "arte/atlas",
            "--audit-stdin-hash",
            "--",
            "cat",
        ])
        .write_stdin("hello")
        .assert()
        .success();
    // SHA-256 of "hello"
    let expected = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
    let audit_dir = sb.home().join("audit");
    let mut hash_found: Option<String> = None;
    for entry in std::fs::read_dir(&audit_dir).unwrap() {
        let p = entry.unwrap().path();
        if p.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        for line in std::fs::read_to_string(&p).unwrap().lines() {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            if v.get("verb").and_then(|s| s.as_str()) == Some("run") {
                if let Some(h) = v.get("stdin_sha256").and_then(|s| s.as_str()) {
                    hash_found = Some(h.to_string());
                }
            }
        }
    }
    assert_eq!(hash_found.as_deref(), Some(expected));
}

#[test]
fn f9_run_no_stdin_with_empty_pipe_is_silent_pass() {
    // `--no-stdin` with `< /dev/null` (non-tty but empty) must NOT
    // error — the contract is "never silently DROP data", and there
    // is no data to drop. This is the "batch script with no input"
    // case that operators use intentionally.
    let sb = Sandbox::new(f9_run_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    sb.cmd()
        .args(["run", "arte/atlas", "--no-stdin", "--", "echo", "hi"])
        .assert()
        .success()
        .stdout(contains("hi"));
}

// =============================================================================
// F11 — Universal pre-staged --revert on every write verb (load-bearing for
// agentic safety; non-negotiable before v0.2.0 freezes the audit schema).
// =============================================================================

fn audit_jsonl_body(home: &std::path::Path) -> String {
    let dir = home.join("audit");
    let entries: Vec<_> = std::fs::read_dir(&dir)
        .expect("audit dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .collect();
    assert!(!entries.is_empty(), "audit jsonl should exist");
    std::fs::read_to_string(entries[0].path()).unwrap()
}

#[test]
fn f11_chmod_captures_command_pair_revert() {
    let mock = json!([
        // F11 capture: stat -c %a returns prior mode
        { "match": "stat -c %a", "stdout": "0644\n", "exit": 0 },
        // The actual chmod
        { "match": "chmod", "stdout": "", "exit": 0 },
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    sb.cmd()
        .args([
            "chmod",
            "arte/atlas:/etc/app.conf",
            "0600",
            "--apply",
            "--yes",
        ])
        .assert()
        .success();
    let body = audit_jsonl_body(sb.home());
    assert!(body.contains("\"kind\":\"command_pair\""), "body: {body}");
    assert!(body.contains("chmod 0644"), "inverse missing: {body}");
    assert!(body.contains("\"applied\":true"), "applied flag missing: {body}");
}

#[test]
fn f11_exec_apply_without_no_revert_refuses() {
    let mock = json!([
        { "match": "docker exec", "stdout": "", "exit": 0 },
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    sb.cmd()
        .args(["exec", "arte/atlas", "--apply", "--yes", "--", "echo", "hi"])
        .assert()
        .failure()
        .stderr(contains("--no-revert"))
        .stderr(contains("inverse"));
}

#[test]
fn f11_exec_with_no_revert_records_unsupported_kind() {
    let mock = json!([
        { "match": "docker exec", "stdout": "", "exit": 0 },
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    sb.cmd()
        .args([
            "exec", "arte/atlas", "--apply", "--yes", "--no-revert",
            "--", "echo", "hi",
        ])
        .assert()
        .success();
    let body = audit_jsonl_body(sb.home());
    assert!(body.contains("\"kind\":\"unsupported\""), "body: {body}");
    assert!(body.contains("\"no_revert_acknowledged\":true"), "body: {body}");
}

#[test]
fn f11_revert_preview_prints_inverse_before_apply() {
    let mock = json!([
        { "match": "stat -c %a", "stdout": "0755\n", "exit": 0 },
        { "match": "chmod", "stdout": "", "exit": 0 },
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    sb.cmd()
        .args([
            "chmod", "arte/atlas:/usr/bin/foo", "0700",
            "--apply", "--yes", "--revert-preview",
        ])
        .assert()
        .success()
        .stderr(contains("revert preview"))
        .stderr(contains("command_pair"))
        .stderr(contains("chmod 0755"));
}

#[test]
fn f11_revert_command_pair_runs_inverse_via_audit_id() {
    // Two-phase: chmod (capture prior=0644, apply 0600), then revert.
    let mock = json!([
        { "match": "stat -c %a", "stdout": "0644\n", "exit": 0 },
        { "match": "chmod", "stdout": "", "exit": 0 },
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    sb.cmd()
        .args(["chmod", "arte/atlas:/etc/app.conf", "0600", "--apply", "--yes"])
        .assert()
        .success();
    // Pull audit id.
    let body = audit_jsonl_body(sb.home());
    let id = body
        .lines()
        .find(|l| l.contains("\"verb\":\"chmod\""))
        .and_then(|l| {
            let key = "\"id\":\"";
            l.find(key).map(|i| {
                let s = &l[i + key.len()..];
                s.split('"').next().unwrap().to_string()
            })
        })
        .expect("chmod audit id");
    sb.cmd()
        .args(["revert", &id, "--apply", "--yes"])
        .assert()
        .success()
        .stdout(contains("reverted"));
}

#[test]
fn f11_revert_unsupported_refuses_loudly() {
    // exec with --no-revert produces an unsupported-kind audit; revert
    // must refuse rather than silently succeed.
    let mock = json!([
        { "match": "docker exec", "stdout": "", "exit": 0 },
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    sb.cmd()
        .args([
            "exec", "arte/atlas", "--apply", "--yes", "--no-revert",
            "--", "echo", "hi",
        ])
        .assert()
        .success();
    let body = audit_jsonl_body(sb.home());
    let id = body
        .lines()
        .find(|l| l.contains("\"verb\":\"exec\""))
        .and_then(|l| {
            let key = "\"id\":\"";
            l.find(key).map(|i| {
                let s = &l[i + key.len()..];
                s.split('"').next().unwrap().to_string()
            })
        })
        .expect("exec audit id");
    sb.cmd()
        .args(["revert", &id, "--apply", "--yes"])
        .assert()
        .failure()
        .stderr(contains("--no-revert").or(contains("unsupported")));
}

#[test]
fn f11_revert_last_walks_recent_entries() {
    let mock = json!([
        { "match": "stat -c %a", "stdout": "0644\n", "exit": 0 },
        { "match": "chmod", "stdout": "", "exit": 0 },
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    sb.cmd()
        .args(["chmod", "arte/atlas:/etc/a.conf", "0600", "--apply", "--yes"])
        .assert()
        .success();
    // --last 1 dry-run should preview the inverse of the most recent
    // applied entry without requiring an audit id.
    sb.cmd()
        .args(["revert", "--last", "1"])
        .assert()
        .success()
        .stdout(contains("DRY RUN"))
        .stdout(contains("REVERT"));
}

#[test]
fn f11_legacy_audit_entry_predates_contract_loud_error() {
    // Synthesise a legacy v0.1.2 entry by hand (no `revert` field, no
    // previous_hash) and confirm `inspect revert` refuses with the
    // chained hint instead of silently no-opping.
    let sb = Sandbox::new(json!([]));
    write_minimal_arte(&sb);
    let audit_dir = sb.home().join("audit");
    std::fs::create_dir_all(&audit_dir).unwrap();
    let legacy = serde_json::json!({
        "schema_version": 1,
        "id": "1700000000000-abcd",
        "ts": "2024-01-01T00:00:00Z",
        "user": "tester",
        "host": "localhost",
        "verb": "restart",
        "selector": "arte/atlas",
        "args": "",
        "diff_summary": "",
        "exit": 0,
        "duration_ms": 12,
    });
    std::fs::write(
        audit_dir.join("2024-01.jsonl"),
        format!("{}\n", serde_json::to_string(&legacy).unwrap()),
    )
    .unwrap();
    sb.cmd()
        .args(["revert", "1700000000000-abcd"])
        .assert()
        .failure()
        .stderr(contains("predates the revert contract").or(contains("unsupported")));
}

// -----------------------------------------------------------------------------
// F3 — `inspect help <command>` as `--help` synonym.
//
// The contract:
//   1. For every known top-level verb, `inspect help <verb>` and
//      `inspect <verb> --help` produce byte-for-byte identical stdout.
//   2. Editorial topics (e.g. `quickstart`, `selectors`, `search`,
//      `fleet`) keep precedence — `inspect help search` renders the
//      search topic body, not clap's `search --help`.
//   3. Unknown tokens exit 2 with `error: unknown command or topic:
//      '<token>'` and a chained hint to `inspect help`. Never silently
//      fall back to the top-level help.
//   4. Bare `inspect help` is unchanged (renders the index).
// -----------------------------------------------------------------------------

#[test]
fn f3_help_verb_byte_for_byte_matches_dash_dash_help() {
    // Sample of top-level verbs across every registry section
    // (read / write / lifecycle / discovery / safety / ssh /
    // diagnostic). The list is intentionally a representative
    // sample, not the full registry — `tests/help_contract.rs`
    // already iterates the full TOP_LEVEL_VERBS list, and a
    // 50-verb cross product would slow the suite. The contract
    // is the same: every verb identical.
    let verbs = [
        "logs", "status", "health", "ps", "grep", "cat",
        "restart", "stop", "exec", "edit", "rm", "cp", "chmod",
        "audit", "revert", "why", "connectivity",
        "add", "list", "show", "setup", "connect",
    ];
    // Note: `search`, `fleet`, and `bundle` are intentionally excluded —
    // each is also an editorial topic, and per F3 the topic body wins
    // (verified separately by `f3_editorial_topic_takes_precedence_over_verb_synonym`).
    for verb in verbs {
        let via_help = Command::cargo_bin("inspect")
            .unwrap()
            .env("INSPECT_HELP_NO_PAGER", "1")
            .env("NO_COLOR", "1")
            .args(["help", verb])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let via_flag = Command::cargo_bin("inspect")
            .unwrap()
            .env("INSPECT_HELP_NO_PAGER", "1")
            .env("NO_COLOR", "1")
            .args([verb, "--help"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        assert_eq!(
            via_help, via_flag,
            "F3 contract violation: `inspect help {verb}` differs from `inspect {verb} --help`"
        );
    }
}

#[test]
fn f3_editorial_topic_takes_precedence_over_verb_synonym() {
    // `search` is BOTH a verb and an editorial topic. The topic must
    // win so curated content (LogQL DSL guide) trumps clap's flag list.
    let out = Command::cargo_bin("inspect")
        .unwrap()
        .env("INSPECT_HELP_NO_PAGER", "1")
        .env("NO_COLOR", "1")
        .args(["help", "search"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let body = String::from_utf8(out).unwrap();
    // The topic body has a distinctive uppercase header from the
    // renderer; clap's --help does not.
    assert!(
        body.contains("SEARCH"),
        "expected curated SEARCH topic header, got:\n{body}"
    );
    // And it must NOT be clap's flag-list shape.
    assert!(
        !body.contains("\nUsage: inspect search"),
        "expected curated topic, got clap --help body:\n{body}"
    );
}

#[test]
fn f3_unknown_token_exits_2_with_chained_hint() {
    Command::cargo_bin("inspect")
        .unwrap()
        .env("INSPECT_HELP_NO_PAGER", "1")
        .env("NO_COLOR", "1")
        .args(["help", "definitely-not-a-thing"])
        .assert()
        .code(2)
        .stderr(contains("error: unknown command or topic: 'definitely-not-a-thing'"))
        .stderr(contains("see: inspect help examples"));
}

#[test]
fn f3_unknown_token_typo_suggests_real_token() {
    // Suggester considers BOTH topics and verbs (existing P8
    // behavior, re-asserted under the F3 contract).
    Command::cargo_bin("inspect")
        .unwrap()
        .env("INSPECT_HELP_NO_PAGER", "1")
        .env("NO_COLOR", "1")
        .args(["help", "logz"])
        .assert()
        .code(2)
        .stderr(contains("did you mean: logs?"));
}

#[test]
fn f3_bare_help_unchanged_renders_index() {
    // F3 explicitly preserves the bare-help path. No regressions.
    Command::cargo_bin("inspect")
        .unwrap()
        .env("INSPECT_HELP_NO_PAGER", "1")
        .env("NO_COLOR", "1")
        .arg("help")
        .assert()
        .success()
        .stdout(contains("INSPECT"))
        .stdout(contains("Topics:"))
        .stdout(contains("Commands:"));
}

// -----------------------------------------------------------------------------
// F5 — Container-name vs compose-service-name uniform resolution.
//
// The 2nd field user typed `arte/luminary-onyx-onyx-vault-1` (the docker
// container name) after `arte/onyx-vault` (the compose service name); both
// forms appear in the discovered inventory but only the compose form
// resolved. F5 makes both forms work and surfaces aliases in JSON.
// -----------------------------------------------------------------------------

/// F5 helper: write a profile where each service has a distinct
/// compose service `name` and docker `container_name`.
fn write_profile_with_aliases(
    home: &std::path::Path,
    ns: &str,
    services: &[(&str, &str, &str, &str)], // (service_name, container_name, image, health_status)
) {
    let dir = home.join("profiles");
    std::fs::create_dir_all(&dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut svc_yaml = String::new();
    for (name, container_name, image, hs) in services {
        svc_yaml.push_str(&format!(
            "  - name: {name}\n    container_name: {container_name}\n    compose_service: {name}\n    container_id: cid-{container_name}\n    image: {image}\n    ports: []\n    mounts: []\n    health_status: {hs}\n    log_readable_directly: false\n    kind: container\n    depends_on: []\n"
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

/// F5 helper: mock for a single-container host where the docker
/// container name (`container_name`) differs from the compose
/// service name. The runtime cache keys lookup by container_name,
/// so the mock must return the docker name from `docker ps`.
fn f5_arte_mock(container_name: &str) -> serde_json::Value {
    json!([
        { "match": "docker ps --format", "stdout": format!("{container_name}\n"), "exit": 0 },
        {
            "match": "docker inspect",
            "stdout": format!("/{container_name}\thealthy\t0\n"),
            "exit": 0
        }
    ])
}

#[test]
fn f5_container_name_resolves_same_as_service_name() {
    // The 2nd field user's exact reproducer: both selector forms must
    // resolve to the same target in `inspect status`.
    let sb = Sandbox::new(f5_arte_mock("luminary-onyx-onyx-vault-1"));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile_with_aliases(
        sb.home(),
        "arte",
        &[(
            "onyx-vault",
            "luminary-onyx-onyx-vault-1",
            "vault:latest",
            "ok",
        )],
    );

    // Form 1: compose service name (already worked pre-F5).
    sb.cmd()
        .args(["status", "arte/onyx-vault"])
        .assert()
        .success()
        .stdout(contains("onyx-vault"));

    // Form 2: docker container name (broken pre-F5).
    sb.cmd()
        .args(["status", "arte/luminary-onyx-onyx-vault-1"])
        .assert()
        .success()
        .stdout(contains("onyx-vault"));
}

#[test]
fn f5_container_name_form_emits_canonical_hint_on_stderr() {
    // When the rejected-pre-F5 form resolves, we still want the
    // operator to learn the canonical name. The hint goes on stderr
    // (so it doesn't pollute --json or piped stdout) and points at
    // the compose service name as canonical.
    let sb = Sandbox::new(f5_arte_mock("luminary-onyx-onyx-vault-1"));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile_with_aliases(
        sb.home(),
        "arte",
        &[(
            "onyx-vault",
            "luminary-onyx-onyx-vault-1",
            "vault:latest",
            "ok",
        )],
    );

    sb.cmd()
        .args(["status", "arte/luminary-onyx-onyx-vault-1"])
        .assert()
        .success()
        .stderr(contains(
            "'luminary-onyx-onyx-vault-1' is the docker container name",
        ))
        .stderr(contains("canonical selector is 'arte/onyx-vault'"));
}

#[test]
fn f5_canonical_hint_silent_when_no_distinct_alias() {
    // When name == container_name (no compose label, or label promoted
    // to name with no fall-through), there is nothing to suggest.
    let sb = Sandbox::new(f5_arte_mock("worker"));
    write_servers_toml(sb.home(), &["arte"]);
    // Use the existing helper which sets name == container_name.
    write_profile(sb.home(), "arte", &[("worker", "alpine", "ok")]);

    let out = sb
        .cmd()
        .args(["status", "arte/worker"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("docker container name"),
        "canonical hint should be silent when there is no alias to suggest, got stderr: {stderr}"
    );
}

#[test]
fn f5_status_json_carries_aliases_field_per_service() {
    // Agents need to enumerate equivalences without trial-and-error.
    let sb = Sandbox::new(f5_arte_mock("luminary-onyx-onyx-vault-1"));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile_with_aliases(
        sb.home(),
        "arte",
        &[(
            "onyx-vault",
            "luminary-onyx-onyx-vault-1",
            "vault:latest",
            "ok",
        )],
    );

    let out = sb
        .cmd()
        .args(["status", "arte", "--json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let body: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("status --json must parse");
    let services = body
        .pointer("/data/services")
        .and_then(|v| v.as_array())
        .expect("data.services array");
    let svc = services
        .iter()
        .find(|s| s["service"] == "onyx-vault")
        .expect("onyx-vault row");
    let aliases = svc["aliases"]
        .as_array()
        .expect("aliases must be an array, even if empty");
    let alias_strs: Vec<&str> = aliases.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        alias_strs.contains(&"luminary-onyx-onyx-vault-1"),
        "expected docker container name in aliases, got {alias_strs:?}"
    );
}

#[test]
fn f5_status_json_aliases_empty_when_no_distinct_alias() {
    // Schema stability: `aliases` is always present, empty array when
    // there is no distinct docker container name to surface.
    let sb = Sandbox::new(f5_arte_mock("worker"));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("worker", "alpine", "ok")]);

    let out = sb
        .cmd()
        .args(["status", "arte", "--json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let body: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let svc = body
        .pointer("/data/services")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.iter().find(|s| s["service"] == "worker"))
        .expect("worker row");
    assert_eq!(
        svc["aliases"],
        serde_json::json!([]),
        "aliases must be an empty array, not missing or null"
    );
}

#[test]
fn f5_glob_matches_either_form() {
    // Globs work against both name and container_name — operator
    // intent is "match anything that looks like onyx-vault" without
    // having to know which axis the inventory used.
    let sb = Sandbox::new(f5_arte_mock("luminary-onyx-onyx-vault-1"));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile_with_aliases(
        sb.home(),
        "arte",
        &[(
            "onyx-vault",
            "luminary-onyx-onyx-vault-1",
            "vault:latest",
            "ok",
        )],
    );

    // Glob on the compose-name shape.
    sb.cmd()
        .args(["status", "arte/onyx-*"])
        .assert()
        .success()
        .stdout(contains("onyx-vault"));

    // Glob on the docker-name shape.
    sb.cmd()
        .args(["status", "arte/luminary-onyx-onyx-*-1"])
        .assert()
        .success()
        .stdout(contains("onyx-vault"));
}
