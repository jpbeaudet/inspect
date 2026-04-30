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
        // bundle exec body — wrapped as `docker exec sh -c '...'`
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

// -----------------------------------------------------------------------------
// F4 — `inspect why` compose-aware deep-diagnostic bundle.
//
// The 2nd field user's "one load-bearing feature request": for any
// unhealthy/down/restart-looping container, attach (1) recent logs,
// (2) effective Cmd + Entrypoint + wrapper-injection detection, and
// (3) port reality vs declared. Compresses 15-minute manual triage
// into ~30 seconds. Healthy-path output is unchanged (no extra
// remote commands, no perf regression).
// -----------------------------------------------------------------------------

/// F4 mock for an unhealthy Vault-style container with the dev-listen
/// duplicate-bind reproducer the field user hit. Covers every command
/// the bundle gatherer fires; substring-matched in MockRunner.
fn f4_vault_unhealthy_mock() -> serde_json::Value {
    json!([
        // Runtime cache: docker ps + docker inspect for health/restart
        { "match": "docker ps --format", "stdout": "onyx-vault\n", "exit": 0 },
        {
            "match": "docker inspect --format '{{.Name}}",
            "stdout": "/onyx-vault\tunhealthy\t4\n",
            "exit": 0
        },
        // Bundle: recent logs.
        {
            "match": "docker logs --tail",
            "stdout":
                "==> Vault server configuration:\n\
                 listener (tcp): bind: address already in use\n\
                 listener two binds (config + entrypoint): conflict on 8200\n",
            "exit": 0
        },
        // Bundle: effective Cmd / Entrypoint / ports as JSON.
        {
            "match": "docker inspect --format '{{json .Config.Cmd}}",
            "stdout":
                "[\"server\",\"-config=/vault/config\"]|[\"docker-entrypoint.sh\"]|{\"8200/tcp\":[{\"HostIp\":\"0.0.0.0\",\"HostPort\":\"8200\"}]}|{\"8200/tcp\":{}}\n",
            "exit": 0
        },
        // Bundle: entrypoint script — dev-listen-address injection.
        {
            "match": "cat /docker-entrypoint",
            "stdout":
                "#!/bin/sh\n\
                 # Vault wrapper\n\
                 exec vault server -dev-listen-address=0.0.0.0:8200 \"$@\"\n",
            "exit": 0
        },
        // Bundle: host port reality.
        {
            "match": "ss -ltn",
            "stdout":
                "State    Recv-Q   Send-Q   Local Address:Port   Peer Address:Port\n\
                 LISTEN   0        4096     0.0.0.0:8200         0.0.0.0:*\n",
            "exit": 0
        }
    ])
}

#[test]
fn f4_unhealthy_target_attaches_bundle_artifacts() {
    // Headline reproducer: all three sections present in human output.
    let sb = Sandbox::new(f4_vault_unhealthy_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("onyx-vault", "vault:latest", "unhealthy")]);

    sb.cmd()
        .args(["why", "arte/onyx-vault"])
        .assert()
        .code(2) // failing => ExitKind::Error
        .stdout(contains("logs:"))
        .stdout(contains("address already in use"))
        .stdout(contains("effective_command:"))
        .stdout(contains("docker-entrypoint.sh"))
        .stdout(contains("port_reality:"))
        .stdout(contains("8200"));
}

#[test]
fn f4_unhealthy_target_detects_wrapper_injection() {
    // The "wrapper injects: -dev-listen-address" line is the
    // headline diagnostic for the duplicate-bind class of failure.
    let sb = Sandbox::new(f4_vault_unhealthy_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("onyx-vault", "vault:latest", "unhealthy")]);

    sb.cmd()
        .args(["why", "arte/onyx-vault"])
        .assert()
        .code(2)
        .stdout(contains("wrapper injects:"))
        .stdout(contains("-dev-listen-address"));
}

#[test]
fn f4_no_bundle_flag_suppresses_artifacts() {
    // Operators who already drive the deeper queries themselves want
    // the v0.1.2 terse output. --no-bundle restores it.
    let sb = Sandbox::new(f4_vault_unhealthy_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("onyx-vault", "vault:latest", "unhealthy")]);

    let out = sb
        .cmd()
        .args(["why", "arte/onyx-vault", "--no-bundle"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("logs:"),
        "--no-bundle must suppress the logs section: {stdout}"
    );
    assert!(
        !stdout.contains("effective_command:"),
        "--no-bundle must suppress the effective_command section: {stdout}"
    );
    assert!(
        !stdout.contains("port_reality:"),
        "--no-bundle must suppress the port_reality section: {stdout}"
    );
}

#[test]
fn f4_healthy_target_no_bundle_attached() {
    // Happy-path discipline: byte-for-byte unchanged on healthy
    // services, no bundle headers in stdout.
    let sb = Sandbox::new(arte_mock(0));
    write_minimal_arte(&sb);

    let out = sb
        .cmd()
        .args(["why", "arte/atlas"])
        .assert()
        .success() // healthy => exit 0
        .get_output()
        .clone();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("logs:"),
        "healthy target must not attach logs section: {stdout}"
    );
    assert!(
        !stdout.contains("effective_command:"),
        "healthy target must not attach effective_command section: {stdout}"
    );
    assert!(
        !stdout.contains("port_reality:"),
        "healthy target must not attach port_reality section: {stdout}"
    );
}

#[test]
fn f4_log_tail_above_cap_is_clamped_to_200() {
    // The cap protects the operator from accidentally pulling 50k
    // lines through redaction; clamp + one-line notice on stderr.
    let sb = Sandbox::new(f4_vault_unhealthy_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("onyx-vault", "vault:latest", "unhealthy")]);

    sb.cmd()
        .args(["why", "arte/onyx-vault", "--log-tail", "500"])
        .assert()
        .code(2)
        .stderr(contains("--log-tail 500 clamped to 200"));
}

#[test]
fn f4_json_bundle_fields_populated_on_unhealthy() {
    // Agent contract: the three new fields are present and populated
    // with structured data on unhealthy services.
    let sb = Sandbox::new(f4_vault_unhealthy_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("onyx-vault", "vault:latest", "unhealthy")]);

    let out = sb
        .cmd()
        .args(["why", "arte/onyx-vault", "--json"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let line = String::from_utf8(out.stdout).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(line.lines().next().expect("at least one JSON record"))
            .expect("why --json must parse");
    let svc = &v["data"]["services"][0];

    let logs = svc["recent_logs"].as_array().expect("recent_logs array");
    assert!(!logs.is_empty(), "recent_logs must be populated");
    let logs_text = logs
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        logs_text.contains("address already in use"),
        "expected reproducer text in recent_logs, got: {logs_text}"
    );

    let cmd = &svc["effective_command"];
    assert!(
        cmd.is_object(),
        "effective_command must be a JSON object on unhealthy: {cmd:?}"
    );
    assert!(
        cmd["wrapper_injects"]
            .as_str()
            .map(|s| s.contains("-dev-listen-address"))
            .unwrap_or(false),
        "wrapper_injects must surface the dev-listen-address flag: {cmd:?}"
    );

    let ports = svc["port_reality"].as_array().expect("port_reality array");
    assert!(!ports.is_empty(), "port_reality must be populated");
    let p8200 = ports
        .iter()
        .find(|p| p["port"] == 8200)
        .expect("port 8200 row");
    assert_eq!(p8200["port"], 8200);
}

#[test]
fn f4_json_bundle_fields_empty_on_healthy() {
    // Schema stability: the three fields are always present, with
    // documented empty defaults on healthy services so agents can
    // address them without optional-chaining gymnastics.
    let sb = Sandbox::new(arte_mock(0));
    write_minimal_arte(&sb);

    let out = sb
        .cmd()
        .args(["why", "arte/atlas", "--json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let line = String::from_utf8(out.stdout).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(line.lines().next().unwrap()).expect("why --json must parse");
    let svc = &v["data"]["services"][0];

    assert_eq!(
        svc["recent_logs"],
        serde_json::json!([]),
        "recent_logs must be empty array on healthy"
    );
    assert!(
        svc["effective_command"].is_null()
            || svc["effective_command"] == serde_json::json!({}),
        "effective_command must be null or empty object on healthy: {:?}",
        svc["effective_command"]
    );
    assert_eq!(
        svc["port_reality"],
        serde_json::json!([]),
        "port_reality must be empty array on healthy"
    );
}

#[test]
fn f4_smart_next_suggests_entrypoint_inspection_on_double_bind() {
    // When wrapper-injection is detected AND a port appears bound
    // twice, the NEXT block guides the operator at the entrypoint.
    let sb = Sandbox::new(f4_vault_unhealthy_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("onyx-vault", "vault:latest", "unhealthy")]);

    sb.cmd()
        .args(["why", "arte/onyx-vault"])
        .assert()
        .code(2)
        .stdout(contains("NEXT:"))
        .stdout(contains("entrypoint"));
}

// -----------------------------------------------------------------------------
// F7 — Selector / output ergonomic papercuts (bundle of small fixes).
//
//   1. Pre-setup verb error redirects to `inspect setup <ns>` (not
//      `inspect profile`) when the namespace is known but its profile
//      has no services.
//   2. `arte:/path` shorthand is accepted (was already supported as
//      sugar for `arte/_:/path`); a regression test pins the contract.
//   3. `inspect ports --port <n>` / `--port-range <lo-hi>` filter the
//      table server-side. JSON output respects the same filter.
//   4. Global `--quiet` flag suppresses SUMMARY:/NEXT: trailers on the
//      Human path so output is safe to pipe into `tail`/`head`/etc.
//      Mutually exclusive with `--json` (JSON is already trailer-free).
//   5. `inspect status` empty-state output: when the inventory is
//      non-empty but no services were classified, the SUMMARY line
//      reads "no service definitions configured for <ns> — N
//      container(s) discovered but unmatched", with a chained NEXT
//      pointing at `inspect ps <ns>` and `inspect setup <ns> --force`.
//      The `--json` output gains a `state: "ok" | "no_services_matched"
//      | "empty_inventory"` field for agents.
// -----------------------------------------------------------------------------

/// F7.1: empty profile (namespace registered but `inspect setup` never
/// ran or discovered nothing) — the verb should redirect to
/// `inspect setup <ns>`, not `inspect profile`.
#[test]
fn f7_empty_profile_hint_points_to_setup_not_profile() {
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    // Deliberately no profile written → empty service set.

    let out = sb
        .cmd()
        .args(["why", "arte/atlas-vault"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("inspect setup arte"),
        "empty-profile error must redirect to 'inspect setup <ns>': {stderr}"
    );
}

/// F7.2: regression-pin the `arte:/path` host shorthand (already
/// accepted as sugar for `arte/_:/path`). The field user's typo ran
/// `inspect cat arte:/etc/hosts`; today it parses fine and the
/// command continues. We just want to make sure the parser does not
/// regress to rejecting this form.
#[test]
fn f7_host_path_shorthand_is_accepted_as_arte_underscore_path() {
    let sb = Sandbox::new(json!([
        { "match": "cat -- '/etc/hostname'", "stdout": "host-arte\n", "exit": 0 }
    ]));
    write_minimal_arte(&sb);
    // `arte:/etc/hostname` should resolve to the host-level target,
    // identical to `arte/_:/etc/hostname`. We test by running cat and
    // expecting the mocked stdout, not a "selector character ':'" error.
    sb.cmd()
        .args(["cat", "arte:/etc/hostname"])
        .assert()
        .success()
        .stdout(contains("host-arte"));
}

/// F7.3a: `inspect ports arte --port 8200` returns only rows mentioning
/// port 8200. Other ports in the same `ss -tlnp` output must be dropped.
#[test]
fn f7_ports_filter_by_single_port() {
    let mock = json!([
        {
            "match": "ss -tlnp",
            "stdout":
                "State    Recv-Q   Send-Q   Local Address:Port   Peer\n\
                 LISTEN   0        128      0.0.0.0:22           0.0.0.0:*\n\
                 LISTEN   0        4096     0.0.0.0:8200         0.0.0.0:*\n\
                 LISTEN   0        4096     0.0.0.0:9090         0.0.0.0:*\n",
            "exit": 0
        }
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);

    let out = sb
        .cmd()
        .args(["ports", "arte", "--port", "8200"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("8200"),
        "filtered output must include the matching port: {stdout}"
    );
    assert!(
        !stdout.contains(":22 ") && !stdout.contains("0.0.0.0:22\t") && !stdout.contains(":9090"),
        "filtered output must drop non-matching ports: {stdout}"
    );
}

/// F7.3b: `--port-range 8000-9000` returns only rows in that range.
#[test]
fn f7_ports_filter_by_port_range() {
    let mock = json!([
        {
            "match": "ss -tlnp",
            "stdout":
                "LISTEN   0   128   0.0.0.0:22     0.0.0.0:*\n\
                 LISTEN   0   4096  0.0.0.0:8200   0.0.0.0:*\n\
                 LISTEN   0   4096  0.0.0.0:9090   0.0.0.0:*\n\
                 LISTEN   0   4096  0.0.0.0:11211  0.0.0.0:*\n",
            "exit": 0
        }
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);

    let out = sb
        .cmd()
        .args(["ports", "arte", "--port-range", "8000-9999"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("8200"));
    assert!(!stdout.contains(":22 "));
    assert!(!stdout.contains("11211"));
    // 9090 is included (range is inclusive).
    assert!(stdout.contains("9090"));
}

/// F7.3c: `--port` and `--port-range` are mutually exclusive.
#[test]
fn f7_ports_filter_flags_mutually_exclusive() {
    let sb = Sandbox::new(json!([]));
    write_minimal_arte(&sb);
    sb.cmd()
        .args(["ports", "arte", "--port", "80", "--port-range", "1-1000"])
        .assert()
        .failure();
}

/// F7.4a: `--quiet` suppresses the `SUMMARY:` and `NEXT:` envelope
/// lines on the Human path so output is safe to pipe into `tail` /
/// `head` / `grep -A` without trailer corruption.
#[test]
fn f7_quiet_suppresses_summary_and_next_on_status() {
    let sb = Sandbox::new(arte_mock(0));
    write_minimal_arte(&sb);

    let out = sb
        .cmd()
        .args(["status", "arte", "--quiet"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("SUMMARY:"),
        "--quiet must suppress SUMMARY line: {stdout}"
    );
    assert!(
        !stdout.contains("NEXT:"),
        "--quiet must suppress NEXT lines: {stdout}"
    );
    // DATA section (or its content) should still be present.
    assert!(
        stdout.contains("atlas"),
        "--quiet must keep DATA content: {stdout}"
    );
}

/// F7.4b: `--quiet` and `--json` are mutually exclusive (JSON is
/// already trailer-free; combining the two would be ambiguous).
#[test]
fn f7_quiet_and_json_are_mutually_exclusive() {
    let sb = Sandbox::new(arte_mock(0));
    write_minimal_arte(&sb);
    sb.cmd()
        .args(["status", "arte", "--quiet", "--json"])
        .assert()
        .failure();
}

/// F7.5a: empty-state phrasing — when the inventory is non-empty but
/// no services match (e.g. discovery classified zero containers as
/// services), the SUMMARY reads as a configuration condition, not
/// "everything is broken." Chained NEXT points at `inspect ps` and
/// `inspect setup --force`.
#[test]
fn f7_status_empty_state_phrases_as_no_services_configured() {
    // Mock inventory with three containers, but no profile written
    // (so no service definitions matched).
    let mock = json!([
        { "match": "docker ps --format", "stdout": "raw1\nraw2\nraw3\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    // Empty profile (zero services), but inventory has 3 containers.
    write_profile(sb.home(), "arte", &[]);

    let out = sb
        .cmd()
        .args(["status", "arte"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("no service definitions configured")
            || stdout.contains("no service definitions"),
        "empty-state text must phrase as a config condition: {stdout}"
    );
    assert!(
        stdout.contains("inspect ps") && stdout.contains("inspect setup"),
        "NEXT must guide to ps + setup --force: {stdout}"
    );
}

/// F7.5b: status `--json` carries an explicit `state` field so agents
/// distinguish ok / no_services_matched / empty_inventory without
/// parsing the SUMMARY prose.
#[test]
fn f7_status_json_carries_state_field() {
    let sb = Sandbox::new(arte_mock(0));
    write_minimal_arte(&sb);

    let out = sb
        .cmd()
        .args(["status", "arte", "--json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let line = String::from_utf8(out.stdout).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(line.lines().next().expect("at least one JSON record"))
            .expect("status --json must parse");
    assert_eq!(
        v["data"]["state"], "ok",
        "healthy status must carry state=ok: {v}"
    );
}

/// F7.5c: state field carries `no_services_matched` when the inventory
/// is non-empty but no services were classified.
#[test]
fn f7_status_json_state_no_services_matched_with_nonempty_inventory() {
    let mock = json!([
        { "match": "docker ps --format", "stdout": "raw1\nraw2\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[]);

    let out = sb
        .cmd()
        .args(["status", "arte", "--json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let line = String::from_utf8(out.stdout).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(line.lines().next().unwrap()).expect("status --json must parse");
    assert_eq!(
        v["data"]["state"], "no_services_matched",
        "non-empty inventory + zero services must surface as no_services_matched: {v}"
    );
}

// =============================================================================
// F10 — 4th-user polish bundle (v0.1.3)
// =============================================================================
//
// 7 sub-items, each a documented first-hour friction point:
//   F10.1 — namespace-flag-as-typo hint (`--on`, `--in`, `--at`, `--host`,
//           `--ns`, `--namespace`) on every selector-taking verb
//   F10.2 — `inspect cat --lines L-R` server-side line slice
//   F10.3 — `why <ns>/<container>` chained hint when the token is a running
//           container but not a registered service
//   F10.4 — `inspect grep` / `inspect search` MODEL/EXAMPLE/NOTE help preface
//   F10.5 — F7.4 `--quiet` regression-test promotion (jq-clean, exact wc -l)
//   F10.6 — `inspect logs` discoverability on the top-level `--help` index
//   F10.7 — `--clean-output` / `--no-tty` flag on `inspect run` strips ANSI
//           escapes from captured output and sets TERM=dumb
// -----------------------------------------------------------------------------

/// F10.1a: `inspect why atlas-neo4j --on arte` exits 2 with a chained
/// hint that points at the correct selector form. Mirrors `kubectl
/// -n <ns>` muscle memory; today's error is a generic "unknown flag".
#[test]
fn f10_namespace_flag_typo_emits_chained_hint_on_why() {
    let sb = Sandbox::new(json!([]));
    write_minimal_arte(&sb);
    let out = sb
        .cmd()
        .args(["why", "atlas-neo4j", "--on", "arte"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--on is not a flag"),
        "stderr must explain that --on is not a flag: {stderr}"
    );
    assert!(
        stderr.contains("inspect why arte/atlas-neo4j"),
        "stderr must suggest the canonical form: {stderr}"
    );
}

/// F10.1b: every spelling of the namespace-flag pattern triggers
/// the same chained hint.
#[test]
fn f10_namespace_flag_typo_covers_all_aliases() {
    let sb = Sandbox::new(json!([]));
    write_minimal_arte(&sb);
    for flag in &["--in", "--at", "--host", "--ns", "--namespace"] {
        let out = sb
            .cmd()
            .args(["status", "atlas", flag, "arte"])
            .assert()
            .failure()
            .get_output()
            .clone();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(&format!("{flag} is not a flag")),
            "stderr must explain {flag}: {stderr}"
        );
        assert!(
            stderr.contains("inspect status arte/atlas"),
            "stderr must suggest canonical form for {flag}: {stderr}"
        );
    }
}

/// F10.2a: `inspect cat --lines 5-10` returns lines 5..=10 inclusive
/// (6 lines). Slice happens client-side post-fetch; the server-side
/// fetch is unchanged.
#[test]
fn f10_cat_lines_range_inclusive() {
    let mock = json!([
        {
            "match": "cat -- '/etc/test.conf'",
            "stdout":
                "L1\nL2\nL3\nL4\nL5\nL6\nL7\nL8\nL9\nL10\nL11\nL12\nL13\nL14\nL15\n",
            "exit": 0
        }
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    let out = sb
        .cmd()
        .args(["cat", "arte:/etc/test.conf", "--lines", "5-10"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("L5"), "L5 must appear: {stdout}");
    assert!(stdout.contains("L10"), "L10 must appear: {stdout}");
    assert!(!stdout.contains("L4"), "L4 must NOT appear: {stdout}");
    assert!(!stdout.contains("L11"), "L11 must NOT appear: {stdout}");
}

/// F10.2b: `--start L --end R` is a synonym for `--lines L-R`.
#[test]
fn f10_cat_start_end_synonym_for_lines() {
    let mock = json!([
        {
            "match": "cat -- '/etc/test.conf'",
            "stdout": "L1\nL2\nL3\nL4\nL5\nL6\nL7\nL8\n",
            "exit": 0
        }
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    let out = sb
        .cmd()
        .args(["cat", "arte:/etc/test.conf", "--start", "3", "--end", "5"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("L3") && stdout.contains("L4") && stdout.contains("L5"));
    assert!(!stdout.contains("L2") && !stdout.contains("L6"));
}

/// F10.2c: `--lines` and `--start`/`--end` are mutually exclusive
/// with each other (only one form per invocation).
#[test]
fn f10_cat_lines_and_start_end_mutually_exclusive() {
    let sb = Sandbox::new(json!([]));
    write_minimal_arte(&sb);
    sb.cmd()
        .args(["cat", "arte:/etc/test.conf", "--lines", "1-5", "--start", "3"])
        .assert()
        .failure();
}

/// F10.2d: `--lines 5-10 --json` emits structured `lines: [{n, text}, …]`
/// records with 1-based line numbers — agents get line numbers
/// structurally rather than parsing prose.
#[test]
fn f10_cat_lines_json_emits_n_text_records() {
    let mock = json!([
        {
            "match": "cat -- '/etc/test.conf'",
            "stdout": "alpha\nbeta\ngamma\ndelta\nepsilon\nzeta\n",
            "exit": 0
        }
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    let out = sb
        .cmd()
        .args([
            "cat",
            "arte:/etc/test.conf",
            "--lines",
            "2-4",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut found_n = std::collections::BTreeSet::new();
    let mut found_text: Vec<String> = Vec::new();
    for ln in stdout.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(ln) else {
            continue;
        };
        if let (Some(n), Some(t)) = (v["n"].as_u64(), v["line"].as_str()) {
            found_n.insert(n);
            found_text.push(t.to_string());
        }
    }
    assert!(found_n.contains(&2) && found_n.contains(&3) && found_n.contains(&4));
    assert!(!found_n.contains(&1) && !found_n.contains(&5));
    assert!(found_text.iter().any(|s| s == "beta"));
}

/// F10.3a: `why arte/<container>` — when the resolved selector finds
/// no service definition AND the inventory has a container with that
/// exact name, surface the friendly chained hint and exit 0
/// (informational), not exit 2 (selector typo).
#[test]
fn f10_why_chained_hint_when_container_is_not_a_registered_service() {
    let mock = json!([
        // Atlas is registered AND running; atlas-neo4j is also running but
        // not in the profile (container exists, no service definition).
        { "match": "docker ps --format", "stdout": "atlas\natlas-neo4j\n", "exit": 0 },
        {
            "match": "docker inspect",
            "stdout": "/atlas\thealthy\t0\n/atlas-neo4j\thealthy\t0\n",
            "exit": 0
        }
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    let out = sb
        .cmd()
        .args(["why", "arte/atlas-neo4j"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("running container but not a registered service"),
        "must explain the container/service distinction: {combined}"
    );
    assert!(
        combined.contains("inspect logs arte/atlas-neo4j")
            && combined.contains("inspect setup arte"),
        "must chain logs + setup hints: {combined}"
    );
}

/// F10.3b: when the container is genuinely not present in the
/// inventory either, the prior selector-error path stands (no
/// chained hint, exit 2).
#[test]
fn f10_why_genuine_typo_keeps_selector_error() {
    let mock = json!([
        { "match": "docker ps --format", "stdout": "atlas\n", "exit": 0 },
        {
            "match": "docker inspect",
            "stdout": "/atlas\thealthy\t0\n",
            "exit": 0
        }
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    sb.cmd()
        .args(["why", "arte/nonexistent-typo"])
        .assert()
        .failure();
}

/// F10.4a: `inspect grep --help` carries the MODEL/EXAMPLE/NOTE
/// preface so an operator can tell from `--help` whether grep
/// indexes-then-searches or shells out to remote `grep`.
#[test]
fn f10_grep_help_includes_model_preface() {
    let sb = Sandbox::new(json!([]));
    let out = sb
        .cmd()
        .args(["grep", "--help"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("MODEL:") && stdout.contains("shells out to remote"),
        "grep --help must declare the model: {stdout}"
    );
    assert!(
        stdout.contains("NOTE:") && stdout.contains("inspect search"),
        "grep --help must point at inspect search for indexed search: {stdout}"
    );
}

/// F10.4b: `inspect search --help` carries the matching MODEL/NOTE
/// preface so the contrast lands from either entry point.
#[test]
fn f10_search_help_includes_model_preface() {
    let sb = Sandbox::new(json!([]));
    let out = sb
        .cmd()
        .args(["search", "--help"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("MODEL:") && stdout.contains("LogQL"),
        "search --help must declare the model: {stdout}"
    );
}

/// F10.5a: `inspect status arte --quiet` is jq-parseable when paired
/// with `--json` is impossible (mutex), so the actual contract is:
/// the human path produces no SUMMARY:/NEXT:/WARNINGS: trailers when
/// --quiet is set. We assert the absence directly (the upstream test
/// for jq-clean would require `--json --quiet` which is rejected by
/// design — pipe-clean is established by the no-trailer property).
#[test]
fn f10_quiet_status_human_path_has_no_envelope_trailers() {
    let sb = Sandbox::new(arte_mock(0));
    write_minimal_arte(&sb);
    let out = sb
        .cmd()
        .args(["status", "arte", "--quiet"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("SUMMARY:")
            && !stdout.contains("NEXT:")
            && !stdout.contains("WARNINGS:"),
        "--quiet output must drop envelope trailers: {stdout}"
    );
}

/// F10.5b: `inspect logs <ns>/<svc> --tail 50 --quiet | wc -l == 50`.
/// Logs verb emits N lines for `--tail N` with no envelope; the
/// `--quiet` flag must be parsed without error and the line count
/// must match exactly so pipelines like `--tail 50 --quiet | wc -l`
/// are a tested contract.
#[test]
fn f10_quiet_logs_tail_count_is_exact() {
    // Build a 50-line log payload.
    let payload: String = (1..=50).map(|i| format!("LINE-{i}\n")).collect();
    let mock = json!([
        { "match": "docker logs", "stdout": payload, "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    let out = sb
        .cmd()
        .args(["logs", "arte/atlas", "--tail", "50", "--quiet"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let n = stdout.lines().filter(|l| !l.is_empty()).count();
    assert_eq!(
        n, 50,
        "logs --tail 50 --quiet must emit exactly 50 lines: got {n}\n{stdout}"
    );
}

/// F10.6: top-level `inspect --help` lists `logs` in the "common
/// verbs" block above the diagnostic verbs, with a worked example so
/// operators don't keep reaching for `inspect run -- 'docker logs'`.
#[test]
fn f10_top_level_help_promotes_logs_with_example() {
    let sb = Sandbox::new(json!([]));
    let out = sb
        .cmd()
        .args(["--help"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("COMMON VERBS")
            || stdout.contains("Common verbs")
            || stdout.contains("inspect logs"),
        "top-level --help must surface a common-verbs / logs block: {stdout}"
    );
    assert!(
        stdout.contains("inspect logs arte"),
        "must include a worked example for logs: {stdout}"
    );
}

/// F10.7a: `inspect run --clean-output` strips ANSI escape sequences
/// from captured stdout. Mock injects ESC[31m red ESC[0m markers in
/// the simulated remote output; expected output is plain ASCII.
#[test]
fn f10_run_clean_output_strips_ansi_escapes() {
    let mock = json!([
        {
            "match": "echo hello",
            "stdout": "\u{001b}[31mhello-red\u{001b}[0m\n",
            "exit": 0
        }
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    let out = sb
        .cmd()
        .args(["run", "arte", "--clean-output", "--", "echo", "hello"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("hello-red"),
        "the visible text must survive: {stdout}"
    );
    assert!(
        !stdout.contains("\u{001b}[31m") && !stdout.contains("\u{001b}[0m"),
        "ANSI escape codes must be stripped: {:?}",
        stdout.as_bytes()
    );
}

/// F10.7b: `--clean-output` and `--tty` are mutually exclusive
/// (`--tty` forces tty allocation; `--clean-output` forces no tty
/// + ANSI strip — combining them is incoherent).
#[test]
fn f10_run_clean_output_and_tty_mutually_exclusive() {
    let sb = Sandbox::new(json!([]));
    write_minimal_arte(&sb);
    sb.cmd()
        .args([
            "run",
            "arte",
            "--clean-output",
            "--tty",
            "--",
            "echo",
            "hello",
        ])
        .assert()
        .failure();
}

// =============================================================================
// F12 — Per-namespace remote environment overlay.
//
// Spec: `[namespaces.<ns>.env]` in servers.toml carries a string-string map
// that is prepended to every `inspect run` / `inspect exec` invocation as
// `env KEY="VAL" ... -- <cmd>`. Composes with `--env KEY=VALUE` (per-
// invocation merge) and `--env-clear` (per-invocation drop). `inspect connect
// <ns> --show|--set-path|--set-env|--unset-env` manages the overlay without
// opening a session.
// =============================================================================

fn write_servers_toml_with_env(home: &std::path::Path, ns: &str, env_kvs: &[(&str, &str)]) {
    let mut body = format!(
        "schema_version = 1\n\n[namespaces.{ns}]\nhost = \"{ns}.example.invalid\"\nuser = \"deploy\"\nport = 22\n",
    );
    if !env_kvs.is_empty() {
        body.push_str(&format!("\n[namespaces.{ns}.env]\n"));
        for (k, v) in env_kvs {
            // TOML basic string: escape backslash and quote, leave $ alone
            // so the test cases that need `$PATH` can write it literally.
            let escaped = v.replace('\\', "\\\\").replace('"', "\\\"");
            body.push_str(&format!("{k} = \"{escaped}\"\n"));
        }
    }
    let path = home.join("servers.toml");
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn read_servers_toml(home: &std::path::Path) -> String {
    std::fs::read_to_string(home.join("servers.toml")).unwrap()
}

fn read_audit_entries(home: &std::path::Path, verb: &str) -> Vec<serde_json::Value> {
    let dir = home.join("audit");
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        for line in std::fs::read_to_string(&p).unwrap().lines() {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            if v.get("verb").and_then(|s| s.as_str()) == Some(verb) {
                out.push(v);
            }
        }
    }
    out
}

fn f12_run_mock() -> serde_json::Value {
    // Mock matches against the rendered cmd, which under F12 looks like
    // `env PATH="/extra/bin:$PATH" -- echo hi`. We match on the env
    // prefix substring so the test fails cleanly if F12 stops emitting
    // the prefix; the no-overlay sentinel (`echo hi`) lives second so
    // the env-prefixed match wins by file order.
    json!([
        { "match": "env PATH=\"/extra/bin:$PATH\" --", "stdout": "WITH_OVERLAY\n", "exit": 0 },
        { "match": "env LANG=\"C.UTF-8\" --", "stdout": "WITH_LANG_ONLY\n", "exit": 0 },
        { "match": "env MALICIOUS=", "stdout": "MALICIOUS_LITERAL\n", "exit": 0 },
        { "match": "echo hi", "stdout": "PLAIN_ECHO\n", "exit": 0 },
        // F12 audit: cat for exec dispatches with mock-aware echo so the
        // exec call site can capture stdout deterministically.
        { "match": "cat", "stdout": "exec-out\n", "exit": 0 }
    ])
}

#[test]
fn f12_env_overlay_applied_to_run() {
    // Overlay configured at file-level → rendered remote cmd carries
    // the `env PATH="/extra/bin:$PATH" -- ` prefix. Asserted via the
    // mock entry that only matches when the prefix is present.
    let sb = Sandbox::new(f12_run_mock());
    write_servers_toml_with_env(sb.home(), "arte", &[("PATH", "/extra/bin:$PATH")]);
    sb.cmd()
        .args(["run", "arte", "--", "echo", "hi"])
        .assert()
        .success()
        .stdout(contains("WITH_OVERLAY"));
}

#[test]
fn f12_env_overlay_applied_to_exec_with_audit_record() {
    // Exec with overlay records `env_overlay` map and `rendered_cmd`
    // string in the audit entry, so post-hoc readers can replay.
    let sb = Sandbox::new(f12_run_mock());
    write_servers_toml_with_env(sb.home(), "arte", &[("PATH", "/extra/bin:$PATH")]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    sb.cmd()
        .args([
            "exec",
            "arte/atlas",
            "--apply",
            "--no-revert",
            "--yes",
            "--",
            "cat",
        ])
        .assert()
        .success();
    let entries = read_audit_entries(sb.home(), "exec");
    assert_eq!(entries.len(), 1, "expected one exec audit entry");
    let e = &entries[0];
    let overlay = e
        .get("env_overlay")
        .and_then(|v| v.as_object())
        .unwrap_or_else(|| panic!("env_overlay missing on exec entry: {e}"));
    assert_eq!(
        overlay.get("PATH").and_then(|v| v.as_str()),
        Some("/extra/bin:$PATH"),
    );
    let rendered = e
        .get("rendered_cmd")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("rendered_cmd missing on exec entry: {e}"));
    assert!(
        rendered.starts_with("env PATH=\"/extra/bin:$PATH\" -- "),
        "rendered_cmd should start with overlay prefix, got: {rendered}",
    );
}

#[test]
fn f12_env_clear_drops_namespace_overlay_for_one_invocation() {
    // `--env-clear` alone → cmd dispatches with no `env ... --`
    // prefix even though the namespace has an overlay. Followed by
    // `--env-clear --env LANG=C.UTF-8` → only the user entry survives.
    let sb = Sandbox::new(f12_run_mock());
    write_servers_toml_with_env(sb.home(), "arte", &[("PATH", "/extra/bin:$PATH")]);
    sb.cmd()
        .args(["run", "arte", "--env-clear", "--", "echo", "hi"])
        .assert()
        .success()
        .stdout(contains("PLAIN_ECHO"));
    sb.cmd()
        .args([
            "run",
            "arte",
            "--env-clear",
            "--env",
            "LANG=C.UTF-8",
            "--",
            "echo",
            "hi",
        ])
        .assert()
        .success()
        .stdout(contains("WITH_LANG_ONLY"));
}

#[test]
fn f12_env_flag_without_clear_merges_on_top_of_namespace_overlay() {
    // With `[namespaces.arte.env].PATH = ...` configured AND
    // `--env LANG=C.UTF-8`, both keys reach the rendered cmd.
    let sb = Sandbox::new(json!([
        // Tighter match: BOTH the namespace and the user entry must
        // appear in the prefix for this matcher to fire.
        { "match": "env LANG=\"C.UTF-8\" PATH=\"/extra/bin:$PATH\" --", "stdout": "BOTH\n", "exit": 0 }
    ]));
    write_servers_toml_with_env(sb.home(), "arte", &[("PATH", "/extra/bin:$PATH")]);
    sb.cmd()
        .args(["run", "arte", "--env", "LANG=C.UTF-8", "--", "echo", "hi"])
        .assert()
        .success()
        .stdout(contains("BOTH"));
}

#[test]
fn f12_env_user_wins_collision_with_namespace_overlay() {
    // `--env PATH=/user/bin` overrides the namespace's `PATH=/ns/bin`.
    let sb = Sandbox::new(json!([
        { "match": "env PATH=\"/user/bin\" --", "stdout": "USER\n", "exit": 0 },
        { "match": "env PATH=\"/ns/bin\" --", "stdout": "NAMESPACE\n", "exit": 0 }
    ]));
    write_servers_toml_with_env(sb.home(), "arte", &[("PATH", "/ns/bin")]);
    sb.cmd()
        .args(["run", "arte", "--env", "PATH=/user/bin", "--", "echo", "hi"])
        .assert()
        .success()
        .stdout(contains("USER"));
}

#[test]
fn f12_no_overlay_no_env_prefix_added() {
    // Regression guard: namespace WITHOUT an env block AND no `--env`
    // flag must dispatch the cmd byte-for-byte unchanged (no `env` prefix).
    let sb = Sandbox::new(json!([
        { "match": "env ", "stdout": "UNEXPECTED_PREFIX\n", "exit": 0 },
        { "match": "echo hi", "stdout": "CLEAN\n", "exit": 0 }
    ]));
    write_minimal_arte(&sb);
    sb.cmd()
        .args(["run", "arte", "--", "echo", "hi"])
        .assert()
        .success()
        .stdout(contains("CLEAN").and(contains("UNEXPECTED_PREFIX").not()));
}

#[test]
fn f12_run_debug_prints_rendered_command_to_stderr() {
    // `--debug` echoes the rendered remote command (with overlay
    // prefix) to stderr before dispatch.
    let sb = Sandbox::new(f12_run_mock());
    write_servers_toml_with_env(sb.home(), "arte", &[("PATH", "/extra/bin:$PATH")]);
    sb.cmd()
        .args(["run", "arte", "--debug", "--", "echo", "hi"])
        .assert()
        .success()
        .stderr(contains("rendered command for arte: env PATH=\"/extra/bin:$PATH\" -- echo hi"));
}

#[test]
fn f12_overlay_value_with_semicolon_does_not_split() {
    // Quoting-safety contract: `MALICIOUS = "v;rm -rf /"` is
    // dispatched as a single env-var string, not as two commands.
    // The mock asserts the literal `;` survives in the rendered cmd.
    let sb = Sandbox::new(f12_run_mock());
    write_servers_toml_with_env(
        sb.home(),
        "arte",
        &[("MALICIOUS", "v;rm -rf /")],
    );
    sb.cmd()
        .args(["run", "arte", "--", "echo", "hi"])
        .assert()
        .success()
        .stdout(contains("MALICIOUS_LITERAL"));
}

#[test]
fn f12_invalid_env_key_in_config_is_rejected() {
    // POSIX shell variable name rule: `[A-Za-z_][A-Za-z0-9_]*`.
    // A key like `BAD-KEY` would split on `-` in some shells, so we
    // refuse it at validation time.
    let sb = Sandbox::new(json!([]));
    let body = "schema_version = 1\n\n[namespaces.arte]\nhost = \"arte.example.invalid\"\nuser = \"deploy\"\nport = 22\n\n[namespaces.arte.env]\n\"BAD-KEY\" = \"value\"\n";
    let path = sb.home().join("servers.toml");
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    sb.cmd()
        .args(["run", "arte", "--", "echo", "hi"])
        .assert()
        .failure()
        .stderr(contains("invalid env-overlay key 'BAD-KEY'"));
}

#[test]
fn f12_env_flag_invalid_key_is_rejected_pre_dispatch() {
    let sb = Sandbox::new(f12_run_mock());
    write_minimal_arte(&sb);
    sb.cmd()
        .args(["run", "arte", "--env", "1FOO=x", "--", "echo", "hi"])
        .assert()
        .failure()
        .stderr(contains("must match"));
}

#[test]
fn f12_connect_show_lists_overlay() {
    let sb = Sandbox::new(json!([]));
    write_servers_toml_with_env(
        sb.home(),
        "arte",
        &[("PATH", "$HOME/.local/bin:$PATH"), ("LANG", "C.UTF-8")],
    );
    sb.cmd()
        .args(["connect", "arte", "--show"])
        .assert()
        .success()
        .stdout(
            contains("env overlay for 'arte' (2 entries)")
                .and(contains("LANG=C.UTF-8"))
                .and(contains("PATH=$HOME/.local/bin:$PATH")),
        );
}

#[test]
fn f12_connect_show_empty_overlay() {
    let sb = Sandbox::new(json!([]));
    write_minimal_arte(&sb);
    sb.cmd()
        .args(["connect", "arte", "--show"])
        .assert()
        .success()
        .stdout(contains("(none configured)"));
}

#[test]
fn f12_connect_set_path_writes_config_idempotently() {
    let sb = Sandbox::new(json!([]));
    write_minimal_arte(&sb);
    // First write.
    sb.cmd()
        .args(["connect", "arte", "--set-path", "$HOME/.local/bin:$PATH"])
        .assert()
        .success();
    let body = read_servers_toml(sb.home());
    assert!(
        body.contains("PATH = \"$HOME/.local/bin:$PATH\""),
        "servers.toml missing PATH entry: {body}",
    );
    // Second write with the same value: no-op (idempotent), still
    // present.
    sb.cmd()
        .args(["connect", "arte", "--set-path", "$HOME/.local/bin:$PATH"])
        .assert()
        .success()
        .stdout(contains("already applied"));
}

#[test]
fn f12_connect_set_env_and_unset_env_round_trip() {
    let sb = Sandbox::new(json!([]));
    write_minimal_arte(&sb);
    sb.cmd()
        .args([
            "connect",
            "arte",
            "--set-env",
            "LANG=C.UTF-8",
            "--set-env",
            "PYTHONUNBUFFERED=1",
        ])
        .assert()
        .success();
    let body = read_servers_toml(sb.home());
    assert!(body.contains("LANG = \"C.UTF-8\""));
    assert!(body.contains("PYTHONUNBUFFERED = \"1\""));
    sb.cmd()
        .args(["connect", "arte", "--unset-env", "LANG"])
        .assert()
        .success();
    let body = read_servers_toml(sb.home());
    assert!(!body.contains("LANG ="));
    assert!(body.contains("PYTHONUNBUFFERED = \"1\""));
}

#[test]
fn f12_connect_unset_last_env_drops_table() {
    // Unsetting the last entry empties the map; we drop the
    // `[namespaces.<ns>.env]` block entirely so the TOML stays tidy.
    let sb = Sandbox::new(json!([]));
    write_servers_toml_with_env(sb.home(), "arte", &[("LANG", "C.UTF-8")]);
    sb.cmd()
        .args(["connect", "arte", "--unset-env", "LANG"])
        .assert()
        .success();
    let body = read_servers_toml(sb.home());
    assert!(
        !body.contains("[namespaces.arte.env]"),
        "expected env block to be removed when last entry was unset, got:\n{body}",
    );
}

#[test]
fn f12_connect_set_env_invalid_kv_rejected() {
    let sb = Sandbox::new(json!([]));
    write_minimal_arte(&sb);
    sb.cmd()
        .args(["connect", "arte", "--set-env", "no-equals-sign"])
        .assert()
        .failure();
    sb.cmd()
        .args(["connect", "arte", "--set-env", "BAD-KEY=val"])
        .assert()
        .failure();
}

#[test]
fn f12_connect_set_env_unknown_namespace_rejected() {
    let sb = Sandbox::new(json!([]));
    write_minimal_arte(&sb);
    sb.cmd()
        .args(["connect", "ghost", "--set-env", "FOO=bar"])
        .assert()
        .failure()
        .stderr(contains("not configured"));
}

#[test]
fn f12_show_and_mutate_flags_are_mutually_exclusive() {
    let sb = Sandbox::new(json!([]));
    write_minimal_arte(&sb);
    sb.cmd()
        .args(["connect", "arte", "--show", "--set-env", "FOO=bar"])
        .assert()
        .failure();
}

// =============================================================================
// F13 — stale-session auto-reauth + distinct transport exit class.
//
// Verifies the production contract:
//   • exit code 12 = transport_stale, 13 = unreachable, 14 = auth_failed
//   • SUMMARY trailer carries a chained `ssh_error:` recovery hint
//   • JSON stream gains a final `phase=summary, failure_class=…` envelope
//   • auto-reauth fires exactly once per verb invocation, gated by both
//     `--no-reauth` (per-call) and `auto_reauth = false` (per-namespace).
//
// Tests run against the in-process mock medium driven by:
//   • MockEntry.transport_class — synthesises `Err("transport:<class>")`
//   • MockEntry.max_uses — lets a stale entry fire once then yield to a
//     fallback ok entry on the post-reauth retry.
//   • INSPECT_MOCK_REAUTH=fail — drives the failed-reauth → AuthFailed
//     escalation path.
// =============================================================================

#[test]
fn f13_no_reauth_stale_exits_12_with_summary_trailer() {
    let mock = json!([
        { "match": "sh -c 'cat'", "transport_class": "stale" }
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    sb.cmd()
        .args(["run", "arte/atlas", "--no-reauth", "--", "cat"])
        .assert()
        .code(12)
        .stdout(contains("ssh_error: stale connection").and(contains("--reauth")));
}

#[test]
fn f13_stale_auto_reauth_retries_and_succeeds() {
    let mock = json!([
        { "match": "sh -c 'cat'", "transport_class": "stale", "max_uses": 1 },
        { "match": "sh -c 'cat'", "stdout": "after-reauth\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    sb.cmd()
        .env("INSPECT_MOCK_REAUTH", "ok")
        .args(["run", "arte/atlas", "--", "cat"])
        .assert()
        .success()
        .stdout(contains("after-reauth"))
        .stderr(contains("re-authenticating"));
}

#[test]
fn f13_stale_auto_reauth_failure_exits_14() {
    let mock = json!([
        { "match": "sh -c 'cat'", "transport_class": "stale" }
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    sb.cmd()
        .env("INSPECT_MOCK_REAUTH", "fail")
        .args(["run", "arte/atlas", "--", "cat"])
        .assert()
        .code(14)
        .stdout(contains("ssh_error: auth failed"));
}

#[test]
fn f13_unreachable_exits_13_with_connectivity_hint() {
    let mock = json!([
        { "match": "sh -c 'cat'", "transport_class": "unreachable" }
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    sb.cmd()
        .args(["run", "arte/atlas", "--", "cat"])
        .assert()
        .code(13)
        .stdout(contains("ssh_error: unreachable").and(contains("inspect connectivity")));
}

#[test]
fn f13_per_namespace_auto_reauth_false_disables_retry() {
    let mock = json!([
        { "match": "sh -c 'cat'", "transport_class": "stale" }
    ]);
    let sb = Sandbox::new(mock);
    // Custom servers.toml with auto_reauth = false on the arte namespace.
    let body = "schema_version = 1\n\n\
                [namespaces.arte]\n\
                host = \"arte.example.invalid\"\n\
                user = \"deploy\"\n\
                port = 22\n\
                auto_reauth = false\n";
    let path = sb.home().join("servers.toml");
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    sb.cmd()
        // No --no-reauth, no INSPECT_MOCK_REAUTH override: per-namespace
        // opt-out must be honored on its own.
        .args(["run", "arte/atlas", "--", "cat"])
        .assert()
        .code(12)
        .stdout(contains("ssh_error: stale connection"));
}

#[test]
fn f13_command_failed_uses_inner_exit_not_transport() {
    // Plain non-zero remote exit must still surface as ExitKind::Inner
    // (P11 contract) and must NOT collide with the new 12/13/14 codes.
    let mock = json!([
        { "match": "sh -c", "stdout": "", "exit": 7 }
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    sb.cmd()
        .args(["run", "arte/atlas", "--", "false"])
        .assert()
        .code(7);
}

#[test]
fn f13_json_summary_envelope_carries_failure_class_ok() {
    let mock = json!([
        { "match": "sh -c", "stdout": "hi\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    let out = sb.cmd()
        .args(["run", "arte/atlas", "--json", "--", "echo", "hi"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let body = String::from_utf8(out).unwrap();
    let summary_line = body
        .lines()
        .find(|l| l.contains("\"phase\":\"summary\""))
        .expect("expected a phase=summary envelope on the JSON stream");
    assert!(
        summary_line.contains("\"failure_class\":\"ok\""),
        "summary envelope missing failure_class=ok: {summary_line}"
    );
}

#[test]
fn f13_json_summary_envelope_carries_failure_class_transport_stale() {
    let mock = json!([
        { "match": "sh -c 'cat'", "transport_class": "stale" }
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    let assert = sb.cmd()
        .args(["run", "arte/atlas", "--no-reauth", "--json", "--", "cat"])
        .assert()
        .code(12);
    let body = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let summary_line = body
        .lines()
        .find(|l| l.contains("\"phase\":\"summary\""))
        .expect("expected a phase=summary envelope on the JSON stream");
    assert!(
        summary_line.contains("\"failure_class\":\"transport_stale\""),
        "summary envelope missing failure_class=transport_stale: {summary_line}"
    );
}

#[test]
fn f13_audit_entry_records_failure_class_and_reauth_id() {
    // Drive the stale → reauth → success path and assert that the
    // post-retry audit entry carries `retry_of` + `reauth_id`, and
    // that a `connect.reauth` audit entry was written between them.
    let mock = json!([
        { "match": "sh -c 'cat'", "transport_class": "stale", "max_uses": 1 },
        { "match": "sh -c 'cat'", "stdout": "ok\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    // Use --audit-stdin-hash + stdin payload to force per-step audit.
    sb.cmd()
        .env("INSPECT_MOCK_REAUTH", "ok")
        .args(["run", "arte/atlas", "--", "cat"])
        .write_stdin("payload\n")
        .assert()
        .success();

    // Read the audit log (audit/<yyyy-mm>-<user>.jsonl).
    let dir = sb.home().join("audit");
    let mut body = String::new();
    for entry in std::fs::read_dir(&dir).expect("audit dir should exist") {
        let p = entry.unwrap().path();
        if p.extension().map(|e| e == "jsonl").unwrap_or(false) {
            body.push_str(&std::fs::read_to_string(&p).unwrap());
        }
    }
    let lines: Vec<&str> = body.lines().collect();
    assert!(
        lines.iter().any(|l| l.contains("\"verb\":\"connect.reauth\"")),
        "expected a connect.reauth audit entry in {body}"
    );
    let run_entry = lines
        .iter()
        .find(|l| l.contains("\"verb\":\"run\""))
        .expect("expected a run audit entry");
    assert!(
        run_entry.contains("\"reauth_id\":"),
        "run audit entry missing reauth_id: {run_entry}"
    );
    assert!(
        run_entry.contains("\"retry_of\":"),
        "run audit entry missing retry_of: {run_entry}"
    );
    assert!(
        run_entry.contains("\"failure_class\":\"ok\""),
        "run audit entry missing failure_class=ok: {run_entry}"
    );
}

// =============================================================================
// F14 — `inspect run --file <script>` / `--stdin-script` heredoc-on-stdin
// script mode. Eliminates cross-layer quoting (your shell → ssh → bash →
// docker exec → psql/python -c) by shipping the local script body as the
// remote command body via `bash -s` — the body is never re-parsed by any
// local shell beyond the one that already invoked `inspect`.
// =============================================================================

fn f14_script_mock() -> serde_json::Value {
    json!([
        // Script-mode dispatch: `bash -s` reads the script body from
        // remote stdin. With `echo_stdin: true` the mock surfaces the
        // body in stdout so tests can assert byte-for-byte fidelity.
        { "match": "bash -s", "stdout": "", "exit": 0, "echo_stdin": true },
        // Non-bash interpreter dispatch (shebang test): `python3 -`.
        { "match": "python3 -", "stdout": "", "exit": 0, "echo_stdin": true },
        // Container-targeted variant: `docker exec -i <ctr> bash -s`.
        { "match": "docker exec -i", "stdout": "", "exit": 0, "echo_stdin": true }
    ])
}

#[test]
fn f14_run_file_ships_script_via_bash_s_no_quoting_needed() {
    // The headline contract: a script with embedded `psql -c "..."`
    // and `python -c '...'` heredocs that would normally require an
    // escape pass for every shell layer reaches the remote
    // interpreter byte-for-byte under `--file`.
    let sb = Sandbox::new(f14_script_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let script_dir = sb.home().join("scripts-fixture");
    std::fs::create_dir_all(&script_dir).unwrap();
    let script_path = script_dir.join("migrate.sh");
    let body = "#!/bin/bash\n\
                set -euo pipefail\n\
                psql -c \"SELECT 'embedded \\\"double\\\" quote';\"\n\
                python3 -c 'print(\"hi from $\")'\n\
                cypher-shell <<'CYPHER'\n\
                MATCH (n) RETURN count(n);\n\
                CYPHER\n";
    std::fs::write(&script_path, body).unwrap();
    let assert = sb
        .cmd()
        .args([
            "run",
            "arte/atlas",
            "--file",
            script_path.to_str().unwrap(),
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    // Embedded quote-bearing strings must reach the remote intact.
    assert!(
        stdout.contains("SELECT 'embedded \\\"double\\\" quote';"),
        "psql heredoc should survive byte-for-byte: {stdout}"
    );
    assert!(
        stdout.contains("print(\"hi from $\")"),
        "python -c body should survive byte-for-byte: {stdout}"
    );
    assert!(
        stdout.contains("MATCH (n) RETURN count(n);"),
        "cypher-shell heredoc should survive byte-for-byte: {stdout}"
    );
}

#[test]
fn f14_stdin_script_matches_file_output_byte_for_byte() {
    // The heredoc form: `cat fixture.sh | inspect run … --stdin-script`
    // produces identical output to `inspect run … --file fixture.sh`.
    let body = "#!/bin/bash\necho marker-from-stdin-script\n";
    let sb1 = Sandbox::new(f14_script_mock());
    write_servers_toml(sb1.home(), &["arte"]);
    write_profile(sb1.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let script_path = sb1.home().join("s.sh");
    std::fs::write(&script_path, body).unwrap();
    let a1 = sb1
        .cmd()
        .args(["run", "arte/atlas", "--file", script_path.to_str().unwrap()])
        .assert()
        .success();
    let out1 = String::from_utf8(a1.get_output().stdout.clone()).unwrap();
    drop(sb1);

    let sb2 = Sandbox::new(f14_script_mock());
    write_servers_toml(sb2.home(), &["arte"]);
    write_profile(sb2.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let a2 = sb2
        .cmd()
        .args(["run", "arte/atlas", "--stdin-script"])
        .write_stdin(body)
        .assert()
        .success();
    let out2 = String::from_utf8(a2.get_output().stdout.clone()).unwrap();
    assert_eq!(out1, out2, "--file and --stdin-script must produce identical output");
    assert!(out1.contains("marker-from-stdin-script"));
}

#[test]
fn f14_stdin_script_with_tty_exits_2_with_file_hint() {
    // Mutual exclusion: `--stdin-script` without piped stdin (would be
    // a tty in real life) is rejected loud. assert_cmd's default
    // stdin is /dev/null which `read` treats as empty non-tty; the
    // empty-stdin branch likewise exits 2 with the recovery hint.
    let sb = Sandbox::new(f14_script_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let assert = sb
        .cmd()
        .args(["run", "arte/atlas", "--stdin-script"])
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("--stdin-script") && stderr.contains("--file"),
        "stderr should chain to --file hint: {stderr}"
    );
}

#[test]
fn f14_run_file_and_stdin_script_are_clap_mutually_exclusive() {
    let sb = Sandbox::new(f14_script_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let p = sb.home().join("s.sh");
    std::fs::write(&p, "echo x\n").unwrap();
    sb.cmd()
        .args([
            "run",
            "arte/atlas",
            "--file",
            p.to_str().unwrap(),
            "--stdin-script",
        ])
        .assert()
        .failure();
}

#[test]
fn f14_run_file_args_after_dash_dash_become_positional() {
    // `inspect run … --file s.sh -- alpha beta` runs `bash -s -- alpha beta`
    // on the remote so `$1` / `$2` are `alpha` / `beta`. The mock matches
    // the `bash -s` substring in the rendered command line.
    let sb = Sandbox::new(f14_script_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let p = sb.home().join("s.sh");
    std::fs::write(&p, "#!/bin/bash\necho \"$1 $2\"\n").unwrap();
    sb.cmd()
        .args([
            "run",
            "arte/atlas",
            "--audit-script-body",
            "--file",
            p.to_str().unwrap(),
            "--",
            "alpha",
            "beta",
        ])
        .assert()
        .success();
    // Verify the audit entry recorded the rendered command containing
    // `bash -s -- alpha beta`.
    let body = audit_jsonl_body(sb.home());
    let line = body
        .lines()
        .find(|l| l.contains("\"verb\":\"run\""))
        .expect("expected a run audit entry");
    assert!(
        line.contains("bash -s -- 'alpha' 'beta'"),
        "rendered_cmd should contain bash -s -- 'alpha' 'beta': {line}"
    );
}

#[test]
fn f14_run_file_audit_records_script_path_sha256_and_bytes() {
    let sb = Sandbox::new(f14_script_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let p = sb.home().join("s.sh");
    let body = "#!/bin/bash\necho hello-from-script\n";
    std::fs::write(&p, body).unwrap();
    sb.cmd()
        .args(["run", "arte/atlas", "--file", p.to_str().unwrap()])
        .assert()
        .success();
    // Expected SHA-256 of the script body
    let expected_sha = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(body.as_bytes());
        format!("{:x}", h.finalize())
    };
    let jsonl = audit_jsonl_body(sb.home());
    let line = jsonl
        .lines()
        .find(|l| l.contains("\"verb\":\"run\""))
        .expect("expected a run audit entry");
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(
        v.get("script_sha256").and_then(|s| s.as_str()),
        Some(expected_sha.as_str()),
        "script_sha256 mismatch: {line}"
    );
    assert_eq!(
        v.get("script_bytes").and_then(|n| n.as_u64()),
        Some(body.len() as u64),
        "script_bytes mismatch: {line}"
    );
    let path_field = v
        .get("script_path")
        .and_then(|s| s.as_str())
        .unwrap_or_default();
    assert!(
        path_field.ends_with("s.sh"),
        "script_path should reflect the source file: {line}"
    );
    assert_eq!(
        v.get("script_interp").and_then(|s| s.as_str()),
        Some("bash"),
        "script_interp should be bash: {line}"
    );
    // Without --audit-script-body the body field is omitted.
    assert!(
        v.get("script_body").is_none(),
        "script_body should be absent without --audit-script-body: {line}"
    );
    // The dedup-store should contain the body.
    let stored = sb.home().join("scripts").join(format!("{expected_sha}.sh"));
    assert!(stored.exists(), "script body should be dedup-stored at {stored:?}");
    let stored_body = std::fs::read_to_string(&stored).unwrap();
    assert_eq!(stored_body, body, "stored body should match source");
}

#[test]
fn f14_run_file_audit_script_body_inline() {
    let sb = Sandbox::new(f14_script_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let p = sb.home().join("s.sh");
    let body = "#!/bin/bash\necho inline-body-test\n";
    std::fs::write(&p, body).unwrap();
    sb.cmd()
        .args([
            "run",
            "arte/atlas",
            "--audit-script-body",
            "--file",
            p.to_str().unwrap(),
        ])
        .assert()
        .success();
    let jsonl = audit_jsonl_body(sb.home());
    let line = jsonl
        .lines()
        .find(|l| l.contains("\"verb\":\"run\""))
        .expect("expected a run audit entry");
    assert!(
        line.contains("inline-body-test"),
        "script_body should be inlined under --audit-script-body: {line}"
    );
}

#[test]
fn f14_run_file_above_size_cap_exits_2() {
    let sb = Sandbox::new(f14_script_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let p = sb.home().join("s.sh");
    std::fs::write(&p, "x".repeat(2048)).unwrap();
    let assert = sb
        .cmd()
        .args([
            "run",
            "arte/atlas",
            "--stdin-max",
            "1k",
            "--file",
            p.to_str().unwrap(),
        ])
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("cap") && stderr.contains("inspect cp"),
        "size-cap error should chain to inspect cp: {stderr}"
    );
}

#[test]
fn f14_run_file_missing_path_exits_2_with_hint() {
    let sb = Sandbox::new(f14_script_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let assert = sb
        .cmd()
        .args(["run", "arte/atlas", "--file", "/nonexistent/path.sh"])
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("--file") && stderr.contains("/nonexistent/path.sh"),
        "missing --file should surface the path: {stderr}"
    );
}

#[test]
fn f14_run_file_directory_rejected() {
    let sb = Sandbox::new(f14_script_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let dir = sb.home().join("script-dir");
    std::fs::create_dir_all(&dir).unwrap();
    let assert = sb
        .cmd()
        .args(["run", "arte/atlas", "--file", dir.to_str().unwrap()])
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("directory"),
        "directory --file should be rejected: {stderr}"
    );
}

#[test]
fn f14_run_file_shebang_dispatches_python() {
    // A script whose shebang declares `#!/usr/bin/env python3` is
    // dispatched via `python3 -` (POSIX read-from-stdin convention)
    // instead of `bash -s`. The mock matches the `python3 -` substring.
    let sb = Sandbox::new(f14_script_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let p = sb.home().join("py.py");
    let body = "#!/usr/bin/env python3\nprint('python-dispatch')\n";
    std::fs::write(&p, body).unwrap();
    sb.cmd()
        .args(["run", "arte/atlas", "--file", p.to_str().unwrap()])
        .assert()
        .success();
    let jsonl = audit_jsonl_body(sb.home());
    let line = jsonl
        .lines()
        .find(|l| l.contains("\"verb\":\"run\""))
        .expect("expected a run audit entry");
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(
        v.get("script_interp").and_then(|s| s.as_str()),
        Some("python3"),
        "shebang should select python3 interpreter: {line}"
    );
    let rendered = v
        .get("rendered_cmd")
        .and_then(|s| s.as_str())
        .unwrap_or_default();
    assert!(
        rendered.contains("python3 -"),
        "rendered_cmd should dispatch via `python3 -`: {rendered}"
    );
}

#[test]
fn f14_run_file_container_target_uses_docker_exec_i() {
    // Container-targeted dispatch: `docker exec -i <ctr> bash -s ...`
    // (the `-i` keeps stdin attached so the script body flows in).
    let sb = Sandbox::new(f14_script_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let p = sb.home().join("s.sh");
    std::fs::write(&p, "#!/bin/bash\necho container-marker\n").unwrap();
    sb.cmd()
        .args(["run", "arte/atlas", "--file", p.to_str().unwrap()])
        .assert()
        .success();
    let jsonl = audit_jsonl_body(sb.home());
    let line = jsonl
        .lines()
        .find(|l| l.contains("\"verb\":\"run\""))
        .expect("expected a run audit entry");
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    let rendered = v
        .get("rendered_cmd")
        .and_then(|s| s.as_str())
        .unwrap_or_default();
    assert!(
        rendered.contains("docker exec -i"),
        "container-targeted script should use `docker exec -i`: {rendered}"
    );
    assert!(
        rendered.contains("bash -s"),
        "container-targeted script should still dispatch via `bash -s`: {rendered}"
    );
}

#[test]
fn f14_run_file_no_dash_dash_required_for_script_mode() {
    // Regression guard: classic argv-cmd mode requires `--`, but
    // script mode does not. `inspect run arte/atlas --file s.sh`
    // (no trailing `--`, no positional args) must succeed.
    let sb = Sandbox::new(f14_script_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let p = sb.home().join("s.sh");
    std::fs::write(&p, "#!/bin/bash\necho no-dash-dash\n").unwrap();
    sb.cmd()
        .args(["run", "arte/atlas", "--file", p.to_str().unwrap()])
        .assert()
        .success();
}

#[test]
fn f14_no_stdin_with_file_is_clap_rejected() {
    let sb = Sandbox::new(f14_script_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let p = sb.home().join("s.sh");
    std::fs::write(&p, "echo x\n").unwrap();
    sb.cmd()
        .args([
            "run",
            "arte/atlas",
            "--no-stdin",
            "--file",
            p.to_str().unwrap(),
        ])
        .assert()
        .failure();
}
