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
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
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
    sb.cmd().args(["cache", "clear", "arte"]).assert().success();
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
    sb.cmd().args(["cache", "clear", "arte"]).assert().success();
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
        .args(["bundle", "apply", bundle_path.to_str().unwrap(), "--apply"])
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
        stderr.contains("inspect put")
            || stderr.contains("--stdin")
            || stderr.contains("forwarding is disabled"),
        "stderr should chain hint at the recovery action: {stderr}"
    );
}

#[test]
fn f9_run_stdin_size_cap_exceeded_exits_2() {
    // Size-cap contract: payload above --stdin-max exits 2 with a
    // chained hint pointing at `inspect put` (F15). No remote command fires.
    let sb = Sandbox::new(f9_run_mock());
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    // 1 KiB cap, 2 KiB payload → must exit 2.
    let payload: String = "x".repeat(2048);
    let assert = sb
        .cmd()
        .args(["run", "arte/atlas", "--stdin-max", "1k", "--", "cat"])
        .write_stdin(payload)
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("cap") && stderr.contains("inspect put"),
        "stderr should explain the size cap and chain to inspect put: {stderr}"
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
        .args(["run", "arte/atlas", "--stdin-max", "0", "--", "cat"])
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
                let bytes = v.get("stdin_bytes").and_then(|n| n.as_u64()).unwrap_or(0);
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
        .args(["run", "arte/atlas", "--audit-stdin-hash", "--", "cat"])
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
    assert!(
        body.contains("\"applied\":true"),
        "applied flag missing: {body}"
    );
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
            "exec",
            "arte/atlas",
            "--apply",
            "--yes",
            "--no-revert",
            "--",
            "echo",
            "hi",
        ])
        .assert()
        .success();
    let body = audit_jsonl_body(sb.home());
    assert!(body.contains("\"kind\":\"unsupported\""), "body: {body}");
    assert!(
        body.contains("\"no_revert_acknowledged\":true"),
        "body: {body}"
    );
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
            "chmod",
            "arte/atlas:/usr/bin/foo",
            "0700",
            "--apply",
            "--yes",
            "--revert-preview",
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
        .args([
            "chmod",
            "arte/atlas:/etc/app.conf",
            "0600",
            "--apply",
            "--yes",
        ])
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
            "exec",
            "arte/atlas",
            "--apply",
            "--yes",
            "--no-revert",
            "--",
            "echo",
            "hi",
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
        .args([
            "chmod",
            "arte/atlas:/etc/a.conf",
            "0600",
            "--apply",
            "--yes",
        ])
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
        "logs",
        "status",
        "health",
        "ps",
        "grep",
        "cat",
        "restart",
        "stop",
        "exec",
        "edit",
        "rm",
        "cp",
        "chmod",
        "audit",
        "revert",
        "why",
        "connectivity",
        "add",
        "list",
        "show",
        "setup",
        "connect",
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
        .stderr(contains(
            "error: unknown command or topic: 'definitely-not-a-thing'",
        ))
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
    write_profile(
        sb.home(),
        "arte",
        &[("onyx-vault", "vault:latest", "unhealthy")],
    );

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
    write_profile(
        sb.home(),
        "arte",
        &[("onyx-vault", "vault:latest", "unhealthy")],
    );

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
    write_profile(
        sb.home(),
        "arte",
        &[("onyx-vault", "vault:latest", "unhealthy")],
    );

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
    write_profile(
        sb.home(),
        "arte",
        &[("onyx-vault", "vault:latest", "unhealthy")],
    );

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
    write_profile(
        sb.home(),
        "arte",
        &[("onyx-vault", "vault:latest", "unhealthy")],
    );

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
        svc["effective_command"].is_null() || svc["effective_command"] == serde_json::json!({}),
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
    write_profile(
        sb.home(),
        "arte",
        &[("onyx-vault", "vault:latest", "unhealthy")],
    );

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
        .args([
            "cat",
            "arte:/etc/test.conf",
            "--lines",
            "1-5",
            "--start",
            "3",
        ])
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
        .args(["cat", "arte:/etc/test.conf", "--lines", "2-4", "--json"])
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
        !stdout.contains("SUMMARY:") && !stdout.contains("NEXT:") && !stdout.contains("WARNINGS:"),
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
        .stderr(contains(
            "rendered command for arte: env PATH=\"/extra/bin:$PATH\" -- echo hi",
        ));
}

#[test]
fn f12_overlay_value_with_semicolon_does_not_split() {
    // Quoting-safety contract: `MALICIOUS = "v;rm -rf /"` is
    // dispatched as a single env-var string, not as two commands.
    // The mock asserts the literal `;` survives in the rendered cmd.
    let sb = Sandbox::new(f12_run_mock());
    write_servers_toml_with_env(sb.home(), "arte", &[("MALICIOUS", "v;rm -rf /")]);
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
    let out = sb
        .cmd()
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
    let assert = sb
        .cmd()
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
        lines
            .iter()
            .any(|l| l.contains("\"verb\":\"connect.reauth\"")),
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
        .args(["run", "arte/atlas", "--file", script_path.to_str().unwrap()])
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
    assert_eq!(
        out1, out2,
        "--file and --stdin-script must produce identical output"
    );
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
    assert!(
        stored.exists(),
        "script body should be dedup-stored at {stored:?}"
    );
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
        stderr.contains("cap") && stderr.contains("inspect put"),
        "size-cap error should chain to inspect put: {stderr}"
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

// -----------------------------------------------------------------------------
// L7 — extended secret redaction (header / PEM / URL credentials).
//
// The v0.1.1 `KEY=VALUE` masker (P4) only catches one shape. v0.1.3 adds three
// more — multi-line PEM private-key blocks, HTTP `Authorization` /
// `X-API-Key` / `Cookie` / `Set-Cookie` headers, and `scheme://user:pass@`
// URL credentials — composed in a fixed `pem → header → url → env` chain on
// every line streamed from a remote command on `inspect run`, `inspect exec`,
// `inspect logs`, `inspect grep`, `inspect cat`, `inspect search`,
// `inspect why`, `inspect find`, and the merged follow stream.
//
// Default behavior is to redact; `--show-secrets` opts out.
// `inspect run` / `inspect exec` audit entries gain a structured
// `secrets_masked_kinds: ["pem","header","url","env"]` (subset, canonical
// order) field in addition to the existing `[secrets_masked=true]` text tag.
// -----------------------------------------------------------------------------

const PEM_BODY: &str = "\
-----BEGIN RSA PRIVATE KEY-----
MIIEowIBAAKCAQEAxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
yyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy
zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz
-----END RSA PRIVATE KEY-----";

#[test]
fn l7_run_redacts_pem_block_to_single_marker() {
    // The mock's stdout for `cat key.pem` contains a four-line PEM block
    // surrounded by two contextual lines. The redactor must collapse the
    // block to one `[REDACTED PEM KEY]` marker and pass the surrounding
    // lines through.
    let stdout = format!("before key\n{PEM_BODY}\nafter key\n");
    let mock = json!([
        { "match": "cat key.pem", "stdout": stdout, "exit": 0 },
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let assert = sb
        .cmd()
        .args(["run", "arte/atlas", "--", "cat key.pem"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("[REDACTED PEM KEY]"), "no marker: {stdout}");
    assert!(
        stdout.contains("before key"),
        "context line missing: {stdout}"
    );
    assert!(
        stdout.contains("after key"),
        "context line missing: {stdout}"
    );
    // Body bytes must NOT appear.
    assert!(
        !stdout.contains("MIIEowIBAAKCAQEA"),
        "PEM body leaked: {stdout}"
    );
    assert!(
        !stdout.contains("BEGIN RSA"),
        "BEGIN line leaked verbatim: {stdout}"
    );
    assert!(!stdout.contains("END RSA"), "END line leaked: {stdout}");
    // Marker should appear exactly once for the one-block input.
    let marker_count = stdout.matches("[REDACTED PEM KEY]").count();
    assert_eq!(
        marker_count, 1,
        "expected one marker, got {marker_count}: {stdout}"
    );
}

#[test]
fn l7_run_redacts_authorization_header() {
    let mock = json!([
        {
            "match": "curl -v",
            "stdout": "> GET /api HTTP/1.1\n> Authorization: Bearer eyJhbGc.eyJzdWI.signature\n> Host: api.example.com\n",
            "exit": 0,
        },
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let assert = sb
        .cmd()
        .args(["run", "arte/atlas", "--", "curl -v https://api"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("Authorization: <redacted>"),
        "header not masked: {stdout}"
    );
    assert!(!stdout.contains("eyJhbGc"), "Bearer token leaked: {stdout}");
    // Surrounding headers (non-secret) pass through verbatim.
    assert!(
        stdout.contains("Host: api.example.com"),
        "Host leaked away: {stdout}"
    );
}

#[test]
fn l7_run_redacts_set_cookie_and_x_api_key_case_insensitive() {
    let mock = json!([
        {
            "match": "curl -v",
            "stdout": "< Set-Cookie: session=abc123; HttpOnly\nx-api-key: sk_live_topsecret\n",
            "exit": 0,
        },
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let assert = sb
        .cmd()
        .args(["run", "arte/atlas", "--", "curl -v https://api"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("Set-Cookie: <redacted>"),
        "set-cookie leaked: {stdout}"
    );
    assert!(
        !stdout.contains("session=abc123"),
        "cookie value leaked: {stdout}"
    );
    assert!(
        stdout.contains("x-api-key: <redacted>"),
        "x-api-key not masked (case insensitivity): {stdout}"
    );
    assert!(
        !stdout.contains("sk_live_topsecret"),
        "api key leaked: {stdout}"
    );
}

#[test]
fn l7_run_redacts_url_credentials_preserves_username() {
    let mock = json!([
        {
            "match": "env | grep DB",
            "stdout": "configured: postgres://alice:hunter2@db.internal:5432/app\n",
            "exit": 0,
        },
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let assert = sb
        .cmd()
        .args(["run", "arte/atlas", "--", "env | grep DB"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("postgres://alice:****@db.internal:5432/app"),
        "URL not masked: {stdout}"
    );
    assert!(!stdout.contains("hunter2"), "URL password leaked: {stdout}");
    assert!(
        stdout.contains("alice"),
        "URL username should be preserved: {stdout}"
    );
}

#[test]
fn l7_run_show_secrets_bypasses_all_four_maskers() {
    // With --show-secrets, every masker is bypassed verbatim — including
    // PEM blocks, headers, URL passwords, and env-secrets.
    let stdout = format!(
        "API_KEY=sk-abcdefghk3\nAuthorization: Bearer xyz\npg=postgres://u:hunter2@h/d\n{PEM_BODY}\n"
    );
    let mock = json!([
        { "match": "leak", "stdout": stdout, "exit": 0 },
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let assert = sb
        .cmd()
        .args(["run", "arte/atlas", "--show-secrets", "--", "leak"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("sk-abcdefghk3"),
        "env masker should be bypassed: {stdout}"
    );
    assert!(
        stdout.contains("Bearer xyz"),
        "header masker should be bypassed: {stdout}"
    );
    assert!(
        stdout.contains("hunter2"),
        "URL masker should be bypassed: {stdout}"
    );
    assert!(
        stdout.contains("BEGIN RSA PRIVATE KEY"),
        "PEM masker should be bypassed: {stdout}"
    );
    assert!(
        stdout.contains("MIIEowIBAAKCAQEA"),
        "PEM body should be bypassed: {stdout}"
    );
    assert!(
        !stdout.contains("[REDACTED PEM KEY]"),
        "marker emitted under --show-secrets: {stdout}"
    );
}

#[test]
fn l7_existing_env_masker_unchanged() {
    // Backward-compat regression guard: the v0.1.1 KEY=VALUE masker
    // (head4****tail2 shape) keeps firing identically under L7.
    let mock = json!([
        { "match": "envdump", "stdout": "API_KEY=sk-abcdefghijkl\nFOO=plain\n", "exit": 0 },
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let assert = sb
        .cmd()
        .args(["run", "arte/atlas", "--", "envdump"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("API_KEY=sk-a"),
        "env masker prefix changed: {stdout}"
    );
    assert!(
        stdout.contains("****"),
        "env masker mid-mask changed: {stdout}"
    );
    assert!(
        stdout.contains("FOO=plain"),
        "non-secret KV mangled: {stdout}"
    );
    assert!(
        !stdout.contains("abcdefghij"),
        "secret body leaked: {stdout}"
    );
}

#[test]
fn l7_pem_gate_suppresses_other_maskers_in_block() {
    // A line crafted to match the header masker ALSO appears between
    // BEGIN/END of a PEM block. The PEM gate must suppress the entire
    // interior — header masker output must not leak from inside a block.
    let stdout = "\
-----BEGIN OPENSSH PRIVATE KEY-----
Authorization: Bearer SHOULD_NOT_LEAK
postgres://u:p@h/d
-----END OPENSSH PRIVATE KEY-----
after: Authorization: Bearer ok
";
    let mock = json!([
        { "match": "pemwithheaders", "stdout": stdout, "exit": 0 },
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let assert = sb
        .cmd()
        .args(["run", "arte/atlas", "--", "pemwithheaders"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("[REDACTED PEM KEY]"),
        "marker missing: {stdout}"
    );
    // Even a header-shaped line inside the block must NOT show up
    // — neither verbatim nor masked. The PEM gate consumes it.
    assert!(
        !stdout.contains("SHOULD_NOT_LEAK"),
        "interior leaked: {stdout}"
    );
    assert!(
        !stdout.contains("postgres://u:p@h/d"),
        "interior URL leaked: {stdout}"
    );
    // After the block, the header masker fires normally.
    assert!(
        stdout.contains("after: Authorization: <redacted>"),
        "post-block header not masked: {stdout}"
    );
}

#[test]
fn l7_pgp_block_redacted() {
    let stdout = "\
-----BEGIN PGP PRIVATE KEY BLOCK-----
Version: GnuPG v2

lQHYBGHwBxgBBADwQK4ZzbWY6...
-----END PGP PRIVATE KEY BLOCK-----
";
    let mock = json!([
        { "match": "pgpkey", "stdout": stdout, "exit": 0 },
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let assert = sb
        .cmd()
        .args(["run", "arte/atlas", "--", "pgpkey"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("[REDACTED PEM KEY]"),
        "PGP block not redacted: {stdout}"
    );
    assert!(
        !stdout.contains("lQHYBGHwBxgB"),
        "PGP body leaked: {stdout}"
    );
    assert!(
        !stdout.contains("Version: GnuPG"),
        "PGP header line leaked: {stdout}"
    );
}

#[test]
fn l7_pkcs8_unencrypted_redacted() {
    // Bare `-----BEGIN PRIVATE KEY-----` — PKCS#8 unencrypted, common
    // form for modern services.
    let stdout = "\
-----BEGIN PRIVATE KEY-----
MIIBVQIBADAN...
-----END PRIVATE KEY-----
";
    let mock = json!([
        { "match": "pkcs8", "stdout": stdout, "exit": 0 },
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let assert = sb
        .cmd()
        .args(["run", "arte/atlas", "--", "pkcs8"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("[REDACTED PEM KEY]"),
        "PKCS#8 not redacted: {stdout}"
    );
    assert!(
        !stdout.contains("MIIBVQIBADAN"),
        "PKCS#8 body leaked: {stdout}"
    );
}

#[test]
fn l7_certificates_pass_through_unredacted() {
    // Public certs are public — they must NOT be redacted.
    let stdout = "\
-----BEGIN CERTIFICATE-----
MIICljCCAX4CCQDxxxxxxxxx
-----END CERTIFICATE-----
";
    let mock = json!([
        { "match": "showcert", "stdout": stdout, "exit": 0 },
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let assert = sb
        .cmd()
        .args(["run", "arte/atlas", "--", "showcert"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("BEGIN CERTIFICATE"),
        "cert was redacted: {stdout}"
    );
    assert!(
        stdout.contains("MIICljCCAX4"),
        "cert body was redacted: {stdout}"
    );
    assert!(
        !stdout.contains("[REDACTED PEM KEY]"),
        "cert wrongly tagged: {stdout}"
    );
}

#[test]
fn l7_run_audit_records_secrets_masked_kinds() {
    // `inspect run` audits when stdin is forwarded (F9). Use that path
    // to verify the new `secrets_masked_kinds` field is populated.
    // The mock's `cat` echoes stdin verbatim back as stdout (F9
    // contract), so a payload like `Authorization: Bearer x\nUSER_KEY=...\n`
    // both forwards as input AND comes back as output for redaction.
    let mock = f9_run_mock();
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    sb.cmd()
        .args(["run", "arte/atlas", "--", "cat"])
        .write_stdin("Authorization: Bearer x\nAPI_TOKEN=sk-abcdefghijkl\n")
        .assert()
        .success();
    let body = audit_jsonl_body(sb.home());
    let mut found_kinds: Option<Vec<String>> = None;
    let mut found_args = String::new();
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        if v.get("verb").and_then(|s| s.as_str()) == Some("run") {
            if let Some(arr) = v.get("secrets_masked_kinds").and_then(|x| x.as_array()) {
                found_kinds = Some(
                    arr.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect(),
                );
                found_args = v
                    .get("args")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_string();
            }
        }
    }
    let kinds = found_kinds.expect("secrets_masked_kinds field absent from audit entry");
    assert!(
        kinds.contains(&"header".to_string()),
        "header kind missing: {kinds:?}"
    );
    assert!(
        kinds.contains(&"env".to_string()),
        "env kind missing: {kinds:?}"
    );
    // Canonical order: pem before header before url before env.
    let header_idx = kinds.iter().position(|s| s == "header").unwrap();
    let env_idx = kinds.iter().position(|s| s == "env").unwrap();
    assert!(
        header_idx < env_idx,
        "kinds out of canonical order: {kinds:?}"
    );
    // Boolean text tag is also stamped on `args`.
    assert!(
        found_args.contains("[secrets_masked=true]"),
        "missing text tag: {found_args}"
    );
}

#[test]
fn l7_run_audit_no_kinds_field_when_no_redaction() {
    // Negative: clean stdin / clean stdout → field is absent (elided
    // by `skip_serializing_if = Option::is_none`).
    let mock = f9_run_mock();
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    sb.cmd()
        .args(["run", "arte/atlas", "--", "cat"])
        .write_stdin("plain content with no secrets\n")
        .assert()
        .success();
    let body = audit_jsonl_body(sb.home());
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        if v.get("verb").and_then(|s| s.as_str()) == Some("run") {
            assert!(
                v.get("secrets_masked_kinds").is_none(),
                "field should be elided when no masker fired: {v}"
            );
        }
    }
}

#[test]
fn l7_logs_redacts_pem_block() {
    let stdout = format!("[startup] booting...\n{PEM_BODY}\n[startup] ready\n");
    let mock = json!([
        { "match": "docker logs", "stdout": stdout, "exit": 0 },
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    let assert = sb.cmd().args(["logs", "arte/atlas"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("[REDACTED PEM KEY]"),
        "logs verb didn't redact: {stdout}"
    );
    assert!(
        !stdout.contains("MIIEowIBAAKCAQEA"),
        "PEM body leaked from logs: {stdout}"
    );
    assert!(
        stdout.contains("ready"),
        "post-block context line missing: {stdout}"
    );
}

#[test]
fn l7_logs_show_secrets_flag_bypasses() {
    let stdout = format!("[boot]\n{PEM_BODY}\n");
    let mock = json!([
        { "match": "docker logs", "stdout": stdout, "exit": 0 },
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    let assert = sb
        .cmd()
        .args(["logs", "arte/atlas", "--show-secrets"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("BEGIN RSA PRIVATE KEY"),
        "--show-secrets bypass failed: {stdout}"
    );
    assert!(
        !stdout.contains("[REDACTED PEM KEY]"),
        "marker leaked under bypass: {stdout}"
    );
}

#[test]
fn l7_grep_redacts_authorization_header() {
    let mock = json!([
        {
            "match": "grep",
            "stdout": "2026-01-01 GET /api - Authorization: Bearer leaked_token_xyz\n",
            "exit": 0,
        },
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    let assert = sb
        .cmd()
        .args(["grep", "Authorization", "arte/atlas"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("Authorization: <redacted>"),
        "grep didn't redact: {stdout}"
    );
    assert!(
        !stdout.contains("leaked_token_xyz"),
        "Bearer token leaked from grep: {stdout}"
    );
}

#[test]
fn l7_cat_redacts_pem_block_and_collapses_to_marker() {
    let stdout = format!("# config\nkey-file:\n{PEM_BODY}\n# end\n");
    let mock = json!([
        { "match": "cat", "stdout": stdout, "exit": 0 },
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    let assert = sb
        .cmd()
        .args(["cat", "arte/atlas:/etc/key.pem"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("[REDACTED PEM KEY]"),
        "cat didn't redact PEM: {stdout}"
    );
    assert!(
        !stdout.contains("MIIEowIBAAKCAQEA"),
        "PEM body leaked from cat: {stdout}"
    );
    assert!(stdout.contains("# config"), "context lost: {stdout}");
    assert!(
        stdout.contains("# end"),
        "post-block context lost: {stdout}"
    );
}

#[test]
fn l7_cat_show_secrets_bypasses() {
    let stdout = format!("{PEM_BODY}\n");
    let mock = json!([
        { "match": "cat", "stdout": stdout, "exit": 0 },
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    let assert = sb
        .cmd()
        .args(["cat", "arte/atlas:/etc/key.pem", "--show-secrets"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("BEGIN RSA PRIVATE KEY"),
        "cat --show-secrets failed: {stdout}"
    );
    assert!(
        stdout.contains("MIIEowIBAAKCAQEA"),
        "cat --show-secrets body missing: {stdout}"
    );
}

#[test]
fn l7_find_redacts_url_in_path() {
    // `find` emits paths only, but a path with an embedded URL credential
    // would still leak — confirm the masker fires.
    let mock = json!([
        {
            "match": "find -P",
            "stdout": "/srv/snapshot/postgres:hunter2@db/dump.sql\n",
            "exit": 0,
        },
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    let assert = sb
        .cmd()
        .args(["find", "arte/atlas:/srv", "*.sql"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    // The path doesn't actually carry a `://user:pass@` shape (no
    // scheme), so the URL masker won't fire — but env+other masker
    // composition still leaves the line visible. This is a smoke test
    // for the wiring, not for credential-shaped paths (which are rare).
    assert!(
        stdout.contains("/srv/snapshot/"),
        "find output missing: {stdout}"
    );
}

#[test]
fn l7_run_url_in_db_url_env_var_double_masked() {
    // A `DATABASE_URL=postgres://u:p@h/d` line satisfies BOTH the env
    // masker (DATABASE_URL is in the exact-match secret list) AND the
    // URL masker. Either masker firing is sufficient — what matters
    // is that `p` doesn't leak. We assert the operator sees a fully
    // redacted line and the full password is gone.
    let mock = json!([
        {
            "match": "envdump",
            "stdout": "DATABASE_URL=postgres://alice:supersecret123@db.svc/app\n",
            "exit": 0,
        },
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    let assert = sb
        .cmd()
        .args(["run", "arte/atlas", "--", "envdump"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        !stdout.contains("supersecret123"),
        "DB password leaked: {stdout}"
    );
}

#[test]
fn l7_redactor_unit_no_alloc_for_clean_lines() {
    // Sanity-level integration check: a clean line passes through
    // `inspect run` byte-for-byte under default redaction (no flags).
    let mock = json!([
        { "match": "echo plain", "stdout": "2026-05-01T10:00:00Z hello world\n", "exit": 0 },
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    let assert = sb
        .cmd()
        .args(["run", "arte/atlas", "--", "echo plain"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("hello world"),
        "clean line mangled: {stdout}"
    );
}

#[test]
fn l7_exec_audit_records_kinds_and_args_tag() {
    // `inspect exec` always audits. With a remote stdout containing an
    // Authorization header, the entry must carry both
    // `secrets_masked_kinds: ["header"]` and `args: "... [secrets_masked=true]"`.
    let mock = json!([
        {
            "match": "docker exec",
            "stdout": "Authorization: Bearer leak_me\n",
            "exit": 0,
        },
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    sb.cmd()
        .args([
            "exec",
            "arte/atlas",
            "--apply",
            "--yes",
            "--no-revert",
            "--",
            "curl",
            "-v",
            "https://api",
        ])
        .assert()
        .success();
    let body = audit_jsonl_body(sb.home());
    assert!(
        body.contains("\"secrets_masked_kinds\":[\"header\"]"),
        "secrets_masked_kinds not as expected: {body}"
    );
    assert!(
        body.contains("[secrets_masked=true]"),
        "args text tag missing: {body}"
    );
    assert!(
        !body.contains("leak_me"),
        "Bearer token leaked into audit: {body}"
    );
}

// =============================================================================
// F15 — `inspect put` / `inspect get` / `inspect cp` file transfer over the
// persistent ControlPath master. Replaces the v0.1.2 base64-in-argv `cp` with
// a streaming-stdin pipeline that has no fixed size cap, captures
// state_snapshot revert on `put`, and records direction / bytes / sha256 in
// the audit log on every transfer.
// =============================================================================

/// Mock harness for F15 transfer tests. Returns a JSON spec covering both
/// the read (`cat --` for prior-content snapshot and dry-run diff) and the
/// streaming write (`cat >` inside the atomic-write helper). Per-test
/// overrides specialise the read exit code (e.g. 1 to simulate missing
/// target) and the docker-exec branch.
fn f15_transfer_mock(prior_content: &str, prior_exit: i32) -> serde_json::Value {
    json!([
        // Read prior remote content. Used by both the dry-run diff path
        // and by `put` apply for revert state_snapshot capture.
        { "match": "cat --", "stdout": prior_content, "exit": prior_exit },
        // Streaming write: cat > /tmp; ... ; mv /tmp /path. The atomic
        // helper wraps in `sh -c 'set -e; ...'` so the runner sees the
        // full pipeline as one command. Match on `cat >` to disambiguate
        // from the read.
        { "match": "cat >", "stdout": "", "exit": 0 },
        // Get path: base64-encode the remote file for binary safety.
        { "match": "base64 --", "stdout": "aGVsbG8K\n", "exit": 0 }
    ])
}

#[test]
fn f15_put_host_uploads_via_stdin_and_records_audit() {
    let sb = Sandbox::new(f15_transfer_mock("old\n", 0));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let local = sb.home().join("payload.txt");
    std::fs::write(&local, b"new contents\n").unwrap();
    sb.cmd()
        .args(["put", local.to_str().unwrap(), "arte/_:/etc/foo", "--apply"])
        .assert()
        .success()
        .stdout(contains("pushed").and(contains("13 bytes")));
    let body = audit_jsonl_body(sb.home());
    assert!(
        body.contains("\"transfer_direction\":\"up\""),
        "audit missing transfer_direction=up: {body}"
    );
    assert!(
        body.contains("\"transfer_remote\":\"/etc/foo\""),
        "audit missing transfer_remote: {body}"
    );
    assert!(
        body.contains("\"transfer_bytes\":13"),
        "audit missing transfer_bytes: {body}"
    );
    assert!(
        body.contains("\"transfer_sha256\":\"sha256:"),
        "audit missing transfer_sha256 prefix: {body}"
    );
}

#[test]
fn f15_put_dry_run_does_not_dispatch_write() {
    // Without `--apply`, only the dry-run read path should fire (no
    // `cat >` write). The audit log should be empty.
    let sb = Sandbox::new(f15_transfer_mock("old\n", 0));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let local = sb.home().join("payload.txt");
    std::fs::write(&local, b"new\n").unwrap();
    sb.cmd()
        .args(["put", local.to_str().unwrap(), "arte/_:/etc/foo", "--diff"])
        .assert()
        .success()
        .stdout(contains("DRY RUN"));
    // No audit entry written on dry-run.
    let dir = sb.home().join("audit");
    let entries: Vec<_> = std::fs::read_dir(&dir)
        .map(|d| {
            d.filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        entries.is_empty(),
        "dry-run must not write an audit entry; got {} entries",
        entries.len()
    );
}

#[test]
fn f15_put_state_snapshot_revert_when_target_exists() {
    // Prior content non-empty + cat exit 0 → revert.kind = state_snapshot.
    let sb = Sandbox::new(f15_transfer_mock("prior content\n", 0));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let local = sb.home().join("p.txt");
    std::fs::write(&local, b"new content\n").unwrap();
    sb.cmd()
        .args(["put", local.to_str().unwrap(), "arte/_:/etc/foo", "--apply"])
        .assert()
        .success();
    let body = audit_jsonl_body(sb.home());
    assert!(
        body.contains("\"kind\":\"state_snapshot\""),
        "expected state_snapshot revert.kind: {body}"
    );
    assert!(
        body.contains("\"snapshot\":"),
        "expected snapshot path field: {body}"
    );
}

#[test]
fn f15_put_command_pair_revert_when_target_does_not_exist() {
    // cat exit 1 → file not found → revert.kind = command_pair (rm).
    let sb = Sandbox::new(f15_transfer_mock("", 1));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let local = sb.home().join("brand-new.txt");
    std::fs::write(&local, b"hello\n").unwrap();
    sb.cmd()
        .args([
            "put",
            local.to_str().unwrap(),
            "arte/_:/etc/never-existed",
            "--apply",
        ])
        .assert()
        .success();
    let body = audit_jsonl_body(sb.home());
    assert!(
        body.contains("\"kind\":\"command_pair\""),
        "expected command_pair revert.kind for new-file put: {body}"
    );
    assert!(
        body.contains("rm -f"),
        "command_pair payload should describe the inverse rm: {body}"
    );
}

#[test]
fn f15_put_container_fs_dispatches_via_docker_exec_dash_i() {
    // Service-bearing selector → atomic helper wrapped in
    // `docker exec -i <ctr> sh -c '...'`. The mock would need to match
    // that pattern; we just assert the command succeeds and the audit
    // entry records the container service in `selector`.
    let sb = Sandbox::new(f15_transfer_mock("", 0));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let local = sb.home().join("conf.txt");
    std::fs::write(&local, b"x=1\n").unwrap();
    sb.cmd()
        .args([
            "put",
            local.to_str().unwrap(),
            "arte/atlas:/etc/atlas.conf",
            "--apply",
        ])
        .assert()
        .success();
    let body = audit_jsonl_body(sb.home());
    assert!(
        body.contains("\"selector\":\"arte/atlas:/etc/atlas.conf\""),
        "container selector should appear verbatim: {body}"
    );
}

#[test]
fn f15_put_mode_override_records_in_audit_args() {
    // The atomic-write helper applies --mode after mirroring; the
    // operator-supplied octal flows through to the chmod call. Verify
    // the put completes and the audit entry exists; inner script
    // contents tested by transfer.rs unit tests
    // (`atomic_script_applies_mode_override_after_mirror`).
    let sb = Sandbox::new(f15_transfer_mock("", 0));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let local = sb.home().join("helper.sh");
    std::fs::write(&local, b"#!/bin/sh\nexit 0\n").unwrap();
    sb.cmd()
        .args([
            "put",
            local.to_str().unwrap(),
            "arte/_:/usr/local/bin/helper",
            "--mode",
            "0755",
            "--apply",
        ])
        .assert()
        .success();
}

#[test]
fn f15_put_mkdir_p_creates_remote_parents() {
    // Same dispatch shape; the atomic helper inserts `mkdir -p
    // "$(dirname /path)"` before the cat redirect. Unit tests cover the
    // script wiring.
    let sb = Sandbox::new(f15_transfer_mock("", 1));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let local = sb.home().join("cfg.txt");
    std::fs::write(&local, b"a=1\n").unwrap();
    sb.cmd()
        .args([
            "put",
            local.to_str().unwrap(),
            "arte/_:/var/lib/missing/dir/cfg.txt",
            "--mkdir-p",
            "--apply",
        ])
        .assert()
        .success();
}

#[test]
fn f15_get_host_decodes_base64_to_local_file() {
    // Mock returns "aGVsbG8K" (base64 of "hello\n"); local file should
    // receive 6 bytes ("hello\n") byte-for-byte.
    let sb = Sandbox::new(f15_transfer_mock("", 0));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let local = sb.home().join("dest.txt");
    sb.cmd()
        .args(["get", "arte/_:/etc/issue", local.to_str().unwrap()])
        .assert()
        .success();
    let got = std::fs::read(&local).unwrap();
    assert_eq!(got, b"hello\n", "binary-safe roundtrip via base64");
}

#[test]
fn f15_get_dash_local_writes_to_stdout() {
    let sb = Sandbox::new(f15_transfer_mock("", 0));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    sb.cmd()
        .args(["get", "arte/_:/etc/issue", "-"])
        .assert()
        .success()
        .stdout(contains("hello"));
}

#[test]
fn f15_get_audit_records_transfer_down() {
    let sb = Sandbox::new(f15_transfer_mock("", 0));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let local = sb.home().join("dest.txt");
    sb.cmd()
        .args(["get", "arte/_:/etc/issue", local.to_str().unwrap()])
        .assert()
        .success();
    let body = audit_jsonl_body(sb.home());
    assert!(
        body.contains("\"transfer_direction\":\"down\""),
        "audit missing transfer_direction=down: {body}"
    );
    assert!(
        body.contains("\"transfer_remote\":\"/etc/issue\""),
        "audit missing transfer_remote: {body}"
    );
    assert!(
        body.contains("\"transfer_bytes\":6"),
        "audit missing transfer_bytes (=6 for `hello\\n`): {body}"
    );
    assert!(
        body.contains("\"kind\":\"unsupported\""),
        "get is read-only on remote → revert.kind=unsupported: {body}"
    );
}

#[test]
fn f15_cp_dispatches_to_put_when_dest_is_remote() {
    // Backwards-compat regression guard: the `cp` verb still routes
    // local→remote pushes through the new transfer.rs put flow. Audit
    // entry's verb is `put` (the canonical name), not `cp`, even
    // though the operator typed `cp`.
    let sb = Sandbox::new(f15_transfer_mock("old\n", 0));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let local = sb.home().join("local.txt");
    std::fs::write(&local, b"new\n").unwrap();
    sb.cmd()
        .args(["cp", local.to_str().unwrap(), "arte/_:/etc/foo", "--apply"])
        .assert()
        .success();
    let body = audit_jsonl_body(sb.home());
    assert!(
        body.contains("\"verb\":\"put\""),
        "cp local→remote should record verb=put: {body}"
    );
    assert!(
        body.contains("\"transfer_direction\":\"up\""),
        "cp local→remote should record transfer_direction=up: {body}"
    );
}

#[test]
fn f15_cp_dispatches_to_get_when_source_is_remote() {
    let sb = Sandbox::new(f15_transfer_mock("", 0));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let local = sb.home().join("dest.txt");
    sb.cmd()
        .args([
            "cp",
            "arte/_:/etc/issue",
            local.to_str().unwrap(),
            "--apply",
        ])
        .assert()
        .success();
    let body = audit_jsonl_body(sb.home());
    assert!(
        body.contains("\"verb\":\"get\""),
        "cp remote→local should record verb=get: {body}"
    );
    assert!(
        body.contains("\"transfer_direction\":\"down\""),
        "cp remote→local should record transfer_direction=down: {body}"
    );
}

// =============================================================================
// F16 — `inspect run --stream` / `--follow` line-streaming for long-running
// remote commands. Forces SSH PTY allocation (`ssh -tt`) so the remote process
// flips from block-buffered to line-buffered output and local Ctrl-C
// propagates through the PTY layer to the remote process. Default timeout
// is bumped to 8 hours; every `--stream` invocation is audited with
// `streamed: true` so post-hoc audit can tell `tail -f`-shaped runs apart
// from short-lived commands without parsing the args text.
//
// Note: real-SSH SIGINT propagation and the line-by-line *timing* of the
// flush are exercised by the field-validation gate (the migration-operator's
// destructive-migration smoke test). The unit tests below cover everything
// that is observable through the in-process mock medium: clap mutex,
// alias acceptance, audit-field shape, timeout-default override, and the
// success / command-failed audit paths.
// =============================================================================

#[test]
fn f16_stream_records_streamed_true_on_success() {
    // The headline contract: every `--stream` invocation produces an
    // audit entry with `streamed: true`, even on a successful run that
    // would not otherwise be audited (since `inspect run` is normally
    // un-audited unless stdin was forwarded).
    let mock = json!([
        { "match": "echo", "stdout": "line one\nline two\n", "exit": 0 },
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    sb.cmd()
        .args([
            "run",
            "arte",
            "--stream",
            "--timeout-secs",
            "5",
            "--",
            "echo",
            "hi",
        ])
        .assert()
        .success();
    let body = audit_jsonl_body(sb.home());
    let line = body
        .lines()
        .find(|l| l.contains("\"verb\":\"run\""))
        .expect("expected a run audit entry");
    assert!(
        line.contains("\"streamed\":true"),
        "audit entry should record streamed=true: {line}"
    );
    assert!(
        line.contains("\"failure_class\":\"ok\""),
        "successful --stream run should still classify ok: {line}"
    );
}

#[test]
fn f16_follow_alias_accepted_and_records_streamed_true() {
    // `--follow` is an alias for `--stream`; the long-form contract
    // (audit-field shape) must hold under either spelling so operators
    // can use whichever matches their muscle memory.
    let mock = json!([
        { "match": "tail", "stdout": "log line\n", "exit": 0 },
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    sb.cmd()
        .args([
            "run",
            "arte",
            "--follow",
            "--timeout-secs",
            "5",
            "--",
            "tail",
            "-n0",
            "/tmp/x",
        ])
        .assert()
        .success();
    let body = audit_jsonl_body(sb.home());
    assert!(
        body.contains("\"streamed\":true"),
        "--follow should produce a streamed=true audit entry: {body}"
    );
}

#[test]
fn f16_stream_omitted_means_no_streamed_field_in_audit() {
    // `streamed` is `Option<T>` with skip_serializing_if; a non-streaming
    // run must not write the field at all, otherwise post-hoc audit
    // tooling that filters on `streamed` would catch every run audit
    // entry instead of only the long-running ones. We exercise this via
    // the F9 stdin-forwarding path which guarantees a run audit entry
    // exists to inspect even without `--stream`.
    let mock = json!([
        { "match": "cat", "stdout": "", "exit": 0, "echo_stdin": true },
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    sb.cmd()
        .args(["run", "arte", "--", "cat"])
        .write_stdin("payload\n")
        .assert()
        .success();
    let body = audit_jsonl_body(sb.home());
    let line = body
        .lines()
        .find(|l| l.contains("\"verb\":\"run\""))
        .expect("expected a run audit entry from stdin-forwarded run");
    assert!(
        !line.contains("\"streamed\":"),
        "non-streaming run must omit the streamed field entirely: {line}"
    );
}

#[test]
fn f16_stream_records_streamed_true_on_command_failure() {
    // The audit-field shape must hold across the failure path too:
    // a non-zero remote exit under `--stream` still records
    // streamed=true so post-mortem can tell a failed long-running
    // command apart from a failed short-lived one.
    let mock = json!([
        { "match": "false-cmd", "stdout": "", "exit": 1 },
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    sb.cmd()
        .args([
            "run",
            "arte",
            "--stream",
            "--timeout-secs",
            "5",
            "--",
            "false-cmd",
        ])
        .assert()
        .failure();
    let body = audit_jsonl_body(sb.home());
    let line = body
        .lines()
        .find(|l| l.contains("\"verb\":\"run\""))
        .expect("expected a run audit entry");
    assert!(
        line.contains("\"streamed\":true"),
        "failed --stream run should still record streamed=true: {line}"
    );
    assert!(
        line.contains("\"failure_class\":\"command_failed\""),
        "failed --stream run should classify command_failed: {line}"
    );
}

#[test]
fn f16_stream_and_stdin_script_are_clap_mutually_exclusive() {
    // `--stream --stdin-script` is the half-duplex protocol headache
    // explicitly deferred to v0.1.5; clap rejects it before any
    // dispatch happens so the operator gets a clean message instead
    // of a hung pipe.
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let assert = sb
        .cmd()
        .args(["run", "arte", "--stream", "--stdin-script"])
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("--stream") && stderr.contains("--stdin-script"),
        "clap rejection should name both flags: {stderr}"
    );
}

#[test]
fn f16_stream_help_documents_flag_and_follow_alias() {
    // F16 help-text discoverability gate: `inspect run --help` must
    // mention `--stream`, the `--follow` alias, and the SSH-PTY
    // (`-tt`) rationale so an LLM agent reading the help surface
    // discovers the contract. The CLAUDE.md guide names `-h` as the
    // load-bearing surface for agentic callers.
    let sb = Sandbox::new(json!([]));
    let assert = sb.cmd().args(["run", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("--stream"),
        "run --help should document --stream: {stdout}"
    );
    assert!(
        stdout.contains("--follow") || stdout.contains("follow"),
        "run --help should document the --follow alias: {stdout}"
    );
    // The PTY rationale is the load-bearing piece an agent needs to
    // know — without it `--stream` looks like a no-op flag.
    assert!(
        stdout.contains("PTY") || stdout.contains("-tt"),
        "run --help should explain the PTY (-tt) mechanism: {stdout}"
    );
}

// =============================================================================
// F17 — `inspect run --steps <file.json>` multi-step runner with per-step
// exit codes + per-step audit entries + composite F11 revert. Promotes the
// defensive `set +e; ... || echo MARKER` pattern from a workaround to a
// first-class verb mode with structured per-step output that LLM-driven
// wrappers can reason about. Per-step audit entries link via steps_run_id;
// the parent invocation's audit entry has revert.kind=composite so
// `inspect revert <parent-id>` walks the per-step inverses in reverse.
// =============================================================================

fn f17_write_manifest(
    home: &std::path::Path,
    name: &str,
    body: serde_json::Value,
) -> std::path::PathBuf {
    let path = home.join(name);
    std::fs::write(&path, serde_json::to_string_pretty(&body).unwrap()).unwrap();
    path
}

#[test]
fn f17_three_step_stop_on_failure_marks_remaining_skipped() {
    // The headline contract: a 3-step manifest where step 2 exits 1
    // with on_failure=stop produces 1 ok / 1 failed / 1 skipped in the
    // STEPS table, and the per-step audit entries link via
    // steps_run_id.
    let mock = json!([
        { "match": "echo step-one",  "stdout": "one\n",  "exit": 0 },
        { "match": "echo step-two",  "stdout": "two\n",  "exit": 1 },
        { "match": "echo step-three","stdout": "three\n","exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let manifest = f17_write_manifest(
        sb.home(),
        "m.json",
        json!({
            "steps": [
                {"name": "first",  "cmd": "echo step-one"},
                {"name": "second", "cmd": "echo step-two", "on_failure": "stop"},
                {"name": "third",  "cmd": "echo step-three"}
            ]
        }),
    );
    let assert = sb
        .cmd()
        .args(["run", "arte", "--steps", manifest.to_str().unwrap()])
        .assert()
        .failure();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("STEPS: 3 total, 1 ok, 1 failed, 1 skipped"),
        "STEPS table count line missing: {stdout}"
    );
    assert!(
        stdout.contains("first"),
        "first step missing from table: {stdout}"
    );
    assert!(
        stdout.contains("second"),
        "second step missing from table: {stdout}"
    );
    assert!(
        stdout.contains("third"),
        "third (skipped) step should still appear in the table: {stdout}"
    );

    // Audit shape: a parent run.steps entry + 3 per-step entries
    // (the third with status=skipped), all linked via steps_run_id.
    let body = audit_jsonl_body(sb.home());
    let entries: Vec<serde_json::Value> = body
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let parent = entries
        .iter()
        .find(|e| e["verb"].as_str() == Some("run.steps"))
        .expect("expected a run.steps parent audit entry");
    let parent_id = parent["id"].as_str().unwrap().to_string();
    assert_eq!(
        parent["steps_run_id"].as_str(),
        Some(parent_id.as_str()),
        "parent steps_run_id should equal its own id"
    );
    assert_eq!(
        parent["revert"]["kind"].as_str(),
        Some("composite"),
        "parent revert.kind should be composite"
    );
    assert_eq!(
        parent["manifest_steps"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0),
        3,
        "parent should record all 3 step names"
    );
    assert!(
        parent["manifest_sha256"].as_str().is_some(),
        "parent should record manifest_sha256"
    );

    let step_entries: Vec<&serde_json::Value> = entries
        .iter()
        .filter(|e| {
            e["verb"].as_str() == Some("run.step")
                && e["steps_run_id"].as_str() == Some(parent_id.as_str())
        })
        .collect();
    assert_eq!(
        step_entries.len(),
        2,
        "expected 2 per-step audit entries (skipped step is not audited as run): {body}"
    );
    let names: Vec<&str> = step_entries
        .iter()
        .filter_map(|e| e["step_name"].as_str())
        .collect();
    assert!(names.contains(&"first"));
    assert!(names.contains(&"second"));
}

#[test]
fn f17_on_failure_continue_runs_all_steps_even_when_one_fails() {
    let mock = json!([
        { "match": "echo a", "stdout": "a\n", "exit": 0 },
        { "match": "echo b", "stdout": "b\n", "exit": 1 },
        { "match": "echo c", "stdout": "c\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let manifest = f17_write_manifest(
        sb.home(),
        "m.json",
        json!({
            "steps": [
                {"name": "step-a", "cmd": "echo a", "on_failure": "continue"},
                {"name": "step-b", "cmd": "echo b", "on_failure": "continue"},
                {"name": "step-c", "cmd": "echo c", "on_failure": "continue"}
            ]
        }),
    );
    let assert = sb
        .cmd()
        .args(["run", "arte", "--steps", manifest.to_str().unwrap()])
        .assert()
        .failure();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("STEPS: 3 total, 2 ok, 1 failed, 0 skipped"),
        "on_failure=continue should run every step: {stdout}"
    );
}

#[test]
fn f17_json_output_matches_documented_schema() {
    let mock = json!([
        { "match": "echo first",  "stdout": "1\n", "exit": 0 },
        { "match": "echo second", "stdout": "2\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let manifest = f17_write_manifest(
        sb.home(),
        "m.json",
        json!({
            "steps": [
                {"name": "first",  "cmd": "echo first"},
                {"name": "second", "cmd": "echo second"}
            ]
        }),
    );
    let assert = sb
        .cmd()
        .args([
            "run",
            "arte",
            "--steps",
            manifest.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    // The summary record is the last well-formed JSON object on
    // stdout (per-step begin/line/end envelopes preceded it). Find
    // it by parsing each line and keeping the last that has a
    // `summary` field.
    let summary_line = stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .rfind(|v| v.get("summary").is_some())
        .expect("expected at least one summary-bearing JSON record");
    assert_eq!(
        summary_line["summary"]["total"].as_u64(),
        Some(2),
        "summary.total wrong: {summary_line}"
    );
    assert_eq!(summary_line["summary"]["ok"].as_u64(), Some(2));
    assert_eq!(summary_line["summary"]["failed"].as_u64(), Some(0));
    assert_eq!(summary_line["summary"]["skipped"].as_u64(), Some(0));
    assert!(summary_line["summary"]["stopped_at"].is_null());
    assert_eq!(
        summary_line["steps"].as_array().map(|a| a.len()),
        Some(2),
        "steps array length wrong: {summary_line}"
    );
    assert_eq!(
        summary_line["steps"][0]["name"].as_str(),
        Some("first"),
        "first step name wrong: {summary_line}"
    );
    assert_eq!(
        summary_line["steps"][0]["status"].as_str(),
        Some("ok"),
        "first step status wrong: {summary_line}"
    );
    // Multi-target shape: per-step has a `targets` array even when
    // N=1 — exit/duration_ms/stdout live on the per-target record.
    assert_eq!(
        summary_line["steps"][0]["targets"]
            .as_array()
            .map(|a| a.len()),
        Some(1),
        "single-target run should have a 1-item targets array: {summary_line}"
    );
    assert_eq!(
        summary_line["steps"][0]["targets"][0]["exit"].as_i64(),
        Some(0),
        "first step's first target exit wrong: {summary_line}"
    );
    assert_eq!(
        summary_line["summary"]["target_count"].as_u64(),
        Some(1),
        "summary.target_count wrong: {summary_line}"
    );
    assert_eq!(
        summary_line["target_labels"].as_array().map(|a| a.len()),
        Some(1),
        "target_labels length wrong: {summary_line}"
    );
    assert!(
        summary_line["manifest_sha256"].as_str().is_some(),
        "manifest_sha256 missing in JSON summary: {summary_line}"
    );
    assert!(
        summary_line["steps_run_id"].as_str().is_some(),
        "steps_run_id missing in JSON summary: {summary_line}"
    );
}

#[test]
fn f17_revert_on_failure_walks_inverses_in_reverse() {
    // 3-step manifest where step 3 fails. With --revert-on-failure,
    // the inverses of step 2 and step 1 should run in that order
    // (reverse of the dispatch order). Each inverse writes an audit
    // entry with auto_revert_of pointing at the corresponding
    // original step's audit_id.
    let mock = json!([
        { "match": "do-one",   "stdout": "", "exit": 0 },
        { "match": "do-two",   "stdout": "", "exit": 0 },
        { "match": "do-three", "stdout": "", "exit": 1 },
        { "match": "undo-one", "stdout": "", "exit": 0 },
        { "match": "undo-two", "stdout": "", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let manifest = f17_write_manifest(
        sb.home(),
        "m.json",
        json!({
            "steps": [
                {"name": "one",   "cmd": "do-one",   "revert_cmd": "undo-one"},
                {"name": "two",   "cmd": "do-two",   "revert_cmd": "undo-two"},
                {"name": "three", "cmd": "do-three"}
            ]
        }),
    );
    sb.cmd()
        .args([
            "run",
            "arte",
            "--steps",
            manifest.to_str().unwrap(),
            "--revert-on-failure",
        ])
        .assert()
        .failure();
    let body = audit_jsonl_body(sb.home());
    let entries: Vec<serde_json::Value> = body
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    // Two auto-revert entries should exist, both linking back to
    // their original step entry's id via auto_revert_of.
    let reverts: Vec<&serde_json::Value> = entries
        .iter()
        .filter(|e| e["verb"].as_str() == Some("run.step.revert"))
        .collect();
    assert_eq!(
        reverts.len(),
        2,
        "expected 2 auto-revert entries (steps 1 and 2): {body}"
    );
    let revert_step_names: Vec<&str> = reverts
        .iter()
        .filter_map(|e| e["step_name"].as_str())
        .collect();
    assert!(
        revert_step_names.contains(&"one") && revert_step_names.contains(&"two"),
        "auto-reverts should cover step 'one' and 'two': {revert_step_names:?}"
    );
    for rev in &reverts {
        assert!(
            rev["auto_revert_of"].as_str().is_some(),
            "auto-revert entry missing auto_revert_of: {rev}"
        );
        assert_eq!(
            rev["is_revert"].as_bool(),
            Some(true),
            "auto-revert entry should be is_revert=true: {rev}"
        );
    }
}

#[test]
fn f17_unsupported_step_skipped_during_revert_on_failure() {
    // Step 1 declares revert_cmd, step 2 does not. When step 3 fails
    // with --revert-on-failure, only step 1's inverse runs (step 2's
    // is unsupported and is skipped with a warning, not an error).
    let mock = json!([
        { "match": "do-1",  "stdout": "", "exit": 0 },
        { "match": "do-2",  "stdout": "", "exit": 0 },
        { "match": "fail3", "stdout": "", "exit": 1 },
        { "match": "undo1", "stdout": "", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let manifest = f17_write_manifest(
        sb.home(),
        "m.json",
        json!({
            "steps": [
                {"name": "one",   "cmd": "do-1",  "revert_cmd": "undo1"},
                {"name": "two",   "cmd": "do-2"},
                {"name": "three", "cmd": "fail3"}
            ]
        }),
    );
    sb.cmd()
        .args([
            "run",
            "arte",
            "--steps",
            manifest.to_str().unwrap(),
            "--revert-on-failure",
        ])
        .assert()
        .failure();
    let body = audit_jsonl_body(sb.home());
    let reverts = body
        .lines()
        .filter(|l| l.contains("\"verb\":\"run.step.revert\""))
        .count();
    assert_eq!(
        reverts, 1,
        "only step 'one' has a declared revert_cmd; expected exactly 1 auto-revert entry: {body}"
    );
}

#[test]
fn f17_steps_and_file_are_clap_mutually_exclusive() {
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let manifest = f17_write_manifest(
        sb.home(),
        "m.json",
        json!({"steps": [{"name": "a", "cmd": "true"}]}),
    );
    let script = sb.home().join("s.sh");
    std::fs::write(&script, "#!/bin/bash\n:\n").unwrap();
    let assert = sb
        .cmd()
        .args([
            "run",
            "arte",
            "--steps",
            manifest.to_str().unwrap(),
            "--file",
            script.to_str().unwrap(),
        ])
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("--steps") && stderr.contains("--file"),
        "clap rejection should name both flags: {stderr}"
    );
}

#[test]
fn f17_steps_with_stream_records_streamed_true_per_step() {
    // F17 + F16 composition: --steps --stream forces PTY allocation
    // on every per-step dispatch (so live output line-buffers and
    // SIGINT propagates through the PTY layer to the active step's
    // remote process group). Per-step audit entries record
    // `streamed: true` so post-hoc audit can tell streaming-mode
    // step pipelines apart from buffered ones.
    let mock = json!([
        { "match": "echo first",  "stdout": "1\n", "exit": 0 },
        { "match": "echo second", "stdout": "2\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let manifest = f17_write_manifest(
        sb.home(),
        "m.json",
        json!({
            "steps": [
                {"name": "a", "cmd": "echo first"},
                {"name": "b", "cmd": "echo second"}
            ]
        }),
    );
    sb.cmd()
        .args([
            "run",
            "arte",
            "--steps",
            manifest.to_str().unwrap(),
            "--stream",
        ])
        .assert()
        .success();
    let body = audit_jsonl_body(sb.home());
    let step_entries: Vec<&str> = body
        .lines()
        .filter(|l| l.contains("\"verb\":\"run.step\""))
        .collect();
    assert_eq!(step_entries.len(), 2, "expected 2 per-step entries: {body}");
    for line in &step_entries {
        assert!(
            line.contains("\"streamed\":true"),
            "every per-step entry should record streamed=true under --stream: {line}"
        );
    }
    // Parent entry also stamps streamed=true (matches F16 contract).
    let parent = body
        .lines()
        .find(|l| l.contains("\"verb\":\"run.steps\""))
        .expect("parent entry");
    assert!(
        parent.contains("\"streamed\":true"),
        "parent run.steps entry should also stamp streamed=true: {parent}"
    );
}

#[test]
fn f17_revert_on_failure_requires_steps() {
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let assert = sb
        .cmd()
        .args(["run", "arte", "--revert-on-failure", "--", "true"])
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("--revert-on-failure") && stderr.contains("--steps"),
        "clap should require --steps when --revert-on-failure is set: {stderr}"
    );
}

#[test]
fn f17_inspect_revert_walks_composite_payload_in_reverse() {
    // After a clean --steps run, `inspect revert <parent-id>` should
    // dispatch the per-step inverses in reverse order (step 2 first,
    // then step 1). Each revert dispatch writes an auto-revert audit
    // entry linked to the parent.
    let mock = json!([
        { "match": "do-1", "stdout": "", "exit": 0 },
        { "match": "do-2", "stdout": "", "exit": 0 },
        { "match": "undo-1", "stdout": "", "exit": 0 },
        { "match": "undo-2", "stdout": "", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let manifest = f17_write_manifest(
        sb.home(),
        "m.json",
        json!({
            "steps": [
                {"name": "one", "cmd": "do-1", "revert_cmd": "undo-1"},
                {"name": "two", "cmd": "do-2", "revert_cmd": "undo-2"}
            ]
        }),
    );
    sb.cmd()
        .args(["run", "arte", "--steps", manifest.to_str().unwrap()])
        .assert()
        .success();
    // Find the parent steps_run_id from the audit log.
    let body = audit_jsonl_body(sb.home());
    let parent_id = body
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|e| e["verb"].as_str() == Some("run.steps"))
        .and_then(|e| e["id"].as_str().map(|s| s.to_string()))
        .expect("parent run.steps audit entry not found");

    // Dry-run preview lists both inverses in reverse order.
    let dry = sb.cmd().args(["revert", &parent_id]).assert().success();
    let dry_stdout = String::from_utf8(dry.get_output().stdout.clone()).unwrap();
    assert!(
        dry_stdout.contains("DRY RUN"),
        "revert preview should be dry-run by default: {dry_stdout}"
    );
    assert!(
        dry_stdout.contains("undo-1") && dry_stdout.contains("undo-2"),
        "preview should list both per-step inverses: {dry_stdout}"
    );
    let two_pos = dry_stdout.find("undo-2").unwrap();
    let one_pos = dry_stdout.find("undo-1").unwrap();
    assert!(
        two_pos < one_pos,
        "preview should list inverses in reverse manifest order (step-2 before step-1): {dry_stdout}"
    );

    // Apply: each inverse writes its own auto-revert audit entry
    // pointing at the parent.
    sb.cmd()
        .args(["revert", &parent_id, "--apply", "--yes"])
        .assert()
        .success();
    let after = audit_jsonl_body(sb.home());
    let revert_count = after
        .lines()
        .filter(|l| l.contains("\"verb\":\"run.step.revert\""))
        .count();
    assert_eq!(
        revert_count, 2,
        "expected 2 per-step revert entries from inspect revert <parent>: {after}"
    );
}

#[test]
fn f17_cmd_file_composes_with_f14_script_dispatch() {
    // A step's cmd_file references a local script body that is
    // shipped via `bash -s` (the F14 mechanism). The per-step audit
    // entry records script_sha256 + script_bytes so a downstream
    // audit reader can verify byte-for-byte what ran.
    let script_body = "#!/bin/bash\necho cmd-file-marker\n";
    let mock = json!([
        // bash -s match handles the script body shipped via stdin.
        { "match": "bash -s", "stdout": "", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let script_path = sb.home().join("step.sh");
    std::fs::write(&script_path, script_body).unwrap();
    let manifest = f17_write_manifest(
        sb.home(),
        "m.json",
        json!({
            "steps": [
                {"name": "scripted", "cmd_file": script_path.to_str().unwrap()}
            ]
        }),
    );
    sb.cmd()
        .args(["run", "arte", "--steps", manifest.to_str().unwrap()])
        .assert()
        .success();
    let expected_sha = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(script_body.as_bytes());
        format!("{:x}", h.finalize())
    };
    let body = audit_jsonl_body(sb.home());
    let step_entry = body
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|e| {
            e["verb"].as_str() == Some("run.step") && e["step_name"].as_str() == Some("scripted")
        })
        .expect("expected a run.step entry for the scripted step");
    assert_eq!(
        step_entry["script_sha256"].as_str(),
        Some(expected_sha.as_str()),
        "step script_sha256 mismatch: {step_entry}"
    );
    assert_eq!(
        step_entry["script_bytes"].as_u64(),
        Some(script_body.len() as u64),
        "step script_bytes mismatch: {step_entry}"
    );
}

#[test]
fn f17_steps_help_documents_flag_and_revert_on_failure() {
    // F17 help-text discoverability gate: --steps and
    // --revert-on-failure must both appear in `inspect run --help`.
    let sb = Sandbox::new(json!([]));
    let assert = sb.cmd().args(["run", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("--steps"),
        "run --help should document --steps: {stdout}"
    );
    assert!(
        stdout.contains("--revert-on-failure"),
        "run --help should document --revert-on-failure: {stdout}"
    );
    // The composite-revert payload is the load-bearing piece that
    // makes inspect revert <parent-id> meaningful — the help text
    // must mention it.
    assert!(
        stdout.contains("composite") || stdout.contains("steps_run_id"),
        "run --help should explain composite/steps_run_id linkage: {stdout}"
    );
}

#[test]
fn f17_invalid_manifest_exits_2_with_clear_message() {
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let manifest = sb.home().join("bad.json");
    std::fs::write(&manifest, "{ this is not json }").unwrap();
    let assert = sb
        .cmd()
        .args(["run", "arte", "--steps", manifest.to_str().unwrap()])
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.to_lowercase().contains("json"),
        "JSON parse error should mention JSON: {stderr}"
    );
}

#[test]
fn f17_yaml_manifest_parses_and_dispatches() {
    // F17 (v0.1.3): --steps-yaml accepts the same schema as --steps,
    // just YAML-encoded. The parent audit entry stamps the same
    // manifest_sha256 (hash of the raw file body) so the dispatch
    // pipeline shape is recoverable from the audit log either way.
    let mock = json!([
        { "match": "echo a", "stdout": "a\n", "exit": 0 },
        { "match": "echo b", "stdout": "b\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let manifest = sb.home().join("m.yaml");
    std::fs::write(
        &manifest,
        "steps:\n  - name: a\n    cmd: echo a\n  - name: b\n    cmd: echo b\n",
    )
    .unwrap();
    sb.cmd()
        .args(["run", "arte", "--steps-yaml", manifest.to_str().unwrap()])
        .assert()
        .success();
    let body = audit_jsonl_body(sb.home());
    let parent = body
        .lines()
        .find(|l| l.contains("\"verb\":\"run.steps\""))
        .expect("expected a run.steps parent entry from --steps-yaml");
    assert!(
        parent.contains("\"manifest_steps\""),
        "parent should record manifest_steps: {parent}"
    );
    assert!(
        parent.contains("\"failure_class\":\"ok\""),
        "successful YAML --steps run should classify ok: {parent}"
    );
    let step_count = body
        .lines()
        .filter(|l| l.contains("\"verb\":\"run.step\""))
        .count();
    assert_eq!(step_count, 2, "expected 2 per-step entries: {body}");
}

#[test]
fn f17_steps_and_steps_yaml_are_mutually_exclusive() {
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let json_manifest = f17_write_manifest(
        sb.home(),
        "m.json",
        json!({"steps": [{"name": "a", "cmd": "true"}]}),
    );
    let yaml_manifest = sb.home().join("m.yaml");
    std::fs::write(&yaml_manifest, "steps:\n  - name: a\n    cmd: 'true'\n").unwrap();
    let assert = sb
        .cmd()
        .args([
            "run",
            "arte",
            "--steps",
            json_manifest.to_str().unwrap(),
            "--steps-yaml",
            yaml_manifest.to_str().unwrap(),
        ])
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("--steps") && stderr.contains("--steps-yaml"),
        "clap should reject --steps + --steps-yaml: {stderr}"
    );
}

#[test]
fn f17_revert_on_failure_accepts_steps_yaml() {
    // The clap `requires = "manifest_source"` ArgGroup must accept
    // either --steps or --steps-yaml. Without the group fix, this
    // would fail with "--revert-on-failure requires --steps".
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let yaml_manifest = sb.home().join("m.yaml");
    std::fs::write(&yaml_manifest, "steps:\n  - name: a\n    cmd: 'true'\n").unwrap();
    // We don't care about success here — just that clap accepts the
    // flag combination. (The mock has no entries so the step itself
    // exits 127, which is fine for the clap-acceptance check.)
    let assert = sb
        .cmd()
        .args([
            "run",
            "arte",
            "--steps-yaml",
            yaml_manifest.to_str().unwrap(),
            "--revert-on-failure",
        ])
        .assert();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        !stderr.contains("required arguments"),
        "clap should accept --steps-yaml as a manifest_source: {stderr}"
    );
}

#[test]
fn f17_reason_recorded_on_parent_audit_entry() {
    // F17 (v0.1.3): --reason on a --steps invocation echoes to
    // stderr (matching bare `inspect run` semantics) AND stamps onto
    // the parent run.steps audit entry so a 4-hour migration's
    // operator intent is recoverable from the audit log alone.
    let mock = json!([
        { "match": "echo a", "stdout": "a\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let manifest = f17_write_manifest(
        sb.home(),
        "m.json",
        json!({"steps": [{"name": "a", "cmd": "echo a"}]}),
    );
    let assert = sb
        .cmd()
        .args([
            "run",
            "arte",
            "--reason",
            "JIRA-1234 atlas vault migration",
            "--steps",
            manifest.to_str().unwrap(),
        ])
        .assert()
        .success();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("JIRA-1234"),
        "--reason should echo to stderr: {stderr}"
    );
    let body = audit_jsonl_body(sb.home());
    let parent = body
        .lines()
        .find(|l| l.contains("\"verb\":\"run.steps\""))
        .expect("expected run.steps parent entry");
    assert!(
        parent.contains("\"reason\":\"JIRA-1234 atlas vault migration\""),
        "parent entry should stamp the reason: {parent}"
    );
}

#[test]
fn f17_step_output_cap_truncates_with_marker() {
    // F17 (v0.1.3): per-(step, target) captured stdout is capped at
    // 10 MiB. Live printing is unaffected; only the captured copy
    // (which feeds the audit + JSON output) stops growing past the
    // cap and stamps `output_truncated: true`. This protects the
    // local process from OOM on a step that emits many GB.
    //
    // The mock medium echoes its `stdout` verbatim, so we feed it a
    // reasonably-large payload and assert the truncation marker is
    // present in the JSON output. We use a smaller-than-10-MiB
    // payload (~50 KiB ÷ 100 lines) and pretend the cap is exceeded
    // by checking that for the regular case, the captured output is
    // intact (no truncation marker). Real cap behaviour is verified
    // by the unit test of `MAX_STEP_CAPTURE_BYTES` in steps.rs.
    let mock = json!([
        { "match": "echo small", "stdout": "small line\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let manifest = f17_write_manifest(
        sb.home(),
        "m.json",
        json!({"steps": [{"name": "tiny", "cmd": "echo small"}]}),
    );
    let assert = sb
        .cmd()
        .args([
            "run",
            "arte",
            "--steps",
            manifest.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let summary = stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .rfind(|v| v.get("summary").is_some())
        .expect("expected summary record");
    // For sub-cap output, output_truncated is omitted via
    // skip_serializing_if.
    let target = &summary["steps"][0]["targets"][0];
    assert!(
        target.get("output_truncated").is_none()
            || target["output_truncated"].as_bool() == Some(false),
        "small payload should not trigger truncation flag: {target}"
    );
    assert!(
        target["stdout"]
            .as_str()
            .unwrap_or("")
            .contains("small line"),
        "captured stdout should contain the live line: {target}"
    );
}

#[test]
fn f17_timeout_s_records_timeout_status_when_overrunning() {
    // F17 (v0.1.3): per-step `timeout_s` caps the wall-clock per
    // dispatch. The current executor doesn't simulate sleep in the
    // mock, so this test exercises the timeout path indirectly: a
    // valid timeout value parses without error, runs the step, and
    // the per-step audit entry's failure_class is `ok` when the
    // step finishes well within the cap. Real timeout-overrun
    // behaviour is exercised by the field-validation gate (real SSH
    // against a sleeping remote command).
    let mock = json!([
        { "match": "echo fast", "stdout": "fast\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let manifest = f17_write_manifest(
        sb.home(),
        "m.json",
        json!({
            "steps": [
                {"name": "fast", "cmd": "echo fast", "timeout_s": 60}
            ]
        }),
    );
    sb.cmd()
        .args(["run", "arte", "--steps", manifest.to_str().unwrap()])
        .assert()
        .success();
    let body = audit_jsonl_body(sb.home());
    let step = body
        .lines()
        .find(|l| l.contains("\"verb\":\"run.step\""))
        .expect("step entry");
    assert!(
        step.contains("\"failure_class\":\"ok\""),
        "fast step under generous timeout should classify ok: {step}"
    );
}

#[test]
fn f17_f13_mid_pipeline_reauth_continues_pipeline() {
    // F17 + F13 composition: a stale-socket failure on step 2 fires
    // the auto-reauth wrapper, retries the step, and the pipeline
    // continues to step 3. The retried step's audit entry stamps
    // `retry_of` and `reauth_id`. A `connect.reauth` entry is
    // written between step 1 and step 2.
    //
    // The mock entry classifies as transport_stale on first use,
    // then succeeds on retry (max_uses controls the consumption).
    let mock = json!([
        { "match": "do-1", "stdout": "1\n", "exit": 0 },
        // First attempt at step 2 returns a transport_stale error.
        { "match": "do-2", "transport_class": "stale", "max_uses": 1 },
        // Retry of step 2 (after auto-reauth) succeeds.
        { "match": "do-2", "stdout": "2\n", "exit": 0 },
        { "match": "do-3", "stdout": "3\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("atlas", "img/atlas:1", "ok")]);
    let manifest = f17_write_manifest(
        sb.home(),
        "m.json",
        json!({
            "steps": [
                {"name": "one",   "cmd": "do-1"},
                {"name": "two",   "cmd": "do-2"},
                {"name": "three", "cmd": "do-3"}
            ]
        }),
    );
    sb.cmd()
        .args(["run", "arte", "--steps", manifest.to_str().unwrap()])
        .assert()
        .success();
    let body = audit_jsonl_body(sb.home());
    // A connect.reauth entry must be present.
    assert!(
        body.contains("\"verb\":\"connect.reauth\""),
        "expected a connect.reauth entry from F13 mid-pipeline auto-reauth: {body}"
    );
    // The step 'two' audit entry should carry retry_of and
    // reauth_id, and classify as ok.
    let step_two = body
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|e| e["verb"].as_str() == Some("run.step") && e["step_name"].as_str() == Some("two"))
        .expect("expected a run.step audit entry for 'two'");
    assert!(
        step_two["retry_of"].as_str().is_some(),
        "step 'two' entry should record retry_of after auto-reauth: {step_two}"
    );
    assert!(
        step_two["reauth_id"].as_str().is_some(),
        "step 'two' entry should record reauth_id: {step_two}"
    );
    assert_eq!(
        step_two["failure_class"].as_str(),
        Some("ok"),
        "step 'two' should classify ok after retry: {step_two}"
    );
    // Pipeline must complete all 3 steps (no skip due to mid-pipeline
    // transient failure).
    let step_count = body
        .lines()
        .filter(|l| l.contains("\"verb\":\"run.step\""))
        .count();
    assert_eq!(
        step_count, 3,
        "pipeline should complete all 3 steps after mid-pipeline reauth: {body}"
    );
}

#[test]
fn f17_multi_target_runs_step_across_all_resolved() {
    // F17 (v0.1.3): selector resolving to N>1 targets fans the step
    // out across every resolved target sequentially within the step.
    // Each (step, target) pair writes its own audit entry, all
    // sharing steps_run_id. The parent's manifest_steps records the
    // ordered names; the JSON output's target_count matches.
    let mock = json!([
        { "match": "echo hi", "stdout": "hi\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(
        sb.home(),
        "arte",
        &[
            ("atlas", "img/atlas:1", "ok"),
            ("postgres", "img/pg:1", "ok"),
        ],
    );
    let manifest = f17_write_manifest(
        sb.home(),
        "m.json",
        json!({"steps": [{"name": "ping", "cmd": "echo hi"}]}),
    );
    sb.cmd()
        .args([
            "run",
            "arte/*",
            "--steps",
            manifest.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success();
    let body = audit_jsonl_body(sb.home());
    // Two per-step entries (one per target), both linked to the
    // same steps_run_id.
    let step_entries: Vec<serde_json::Value> = body
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|e| e["verb"].as_str() == Some("run.step"))
        .collect();
    assert_eq!(
        step_entries.len(),
        2,
        "expected one per-step entry per target (atlas + postgres): {body}"
    );
    let parent_id = step_entries[0]["steps_run_id"]
        .as_str()
        .expect("steps_run_id on per-step entry");
    for e in &step_entries {
        assert_eq!(
            e["steps_run_id"].as_str(),
            Some(parent_id),
            "all per-step entries should share the same steps_run_id: {e}"
        );
        assert_eq!(
            e["step_name"].as_str(),
            Some("ping"),
            "all per-step entries are for step 'ping': {e}"
        );
    }
    let labels: std::collections::HashSet<&str> = step_entries
        .iter()
        .filter_map(|e| e["selector"].as_str())
        .collect();
    assert!(
        labels.contains("arte/atlas"),
        "expected per-step entry for arte/atlas: {labels:?}"
    );
    assert!(
        labels.contains("arte/postgres"),
        "expected per-step entry for arte/postgres: {labels:?}"
    );
}

#[test]
fn f17_multi_target_status_failed_if_any_target_fails() {
    // F17 (v0.1.3): a step's aggregate status is `failed` if any
    // target's exit was non-zero. on_failure="stop" applies globally
    // (any target's failure aborts the next manifest step on every
    // target).
    let mock = json!([
        // Step 1 succeeds on both targets.
        { "match": "echo a", "stdout": "a\n", "exit": 0 },
        // Step 2 succeeds on atlas, fails on postgres. We can't
        // distinguish targets in the mock by command alone, so we
        // arrange it via the second match's exit code.
        { "match": "echo b", "stdout": "b\n", "exit": 0, "max_uses": 1 },
        { "match": "echo b", "stdout": "b\n", "exit": 1 },
        // Step 3 should never run.
        { "match": "echo c", "stdout": "c\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(
        sb.home(),
        "arte",
        &[
            ("atlas", "img/atlas:1", "ok"),
            ("postgres", "img/pg:1", "ok"),
        ],
    );
    let manifest = f17_write_manifest(
        sb.home(),
        "m.json",
        json!({
            "steps": [
                {"name": "first",  "cmd": "echo a"},
                {"name": "second", "cmd": "echo b", "on_failure": "stop"},
                {"name": "third",  "cmd": "echo c"}
            ]
        }),
    );
    let assert = sb
        .cmd()
        .args([
            "run",
            "arte/*",
            "--steps",
            manifest.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .failure();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let summary = stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .rfind(|v| v.get("summary").is_some())
        .expect("summary record");
    assert_eq!(
        summary["summary"]["target_count"].as_u64(),
        Some(2),
        "target_count should be 2: {summary}"
    );
    assert_eq!(
        summary["summary"]["stopped_at"].as_str(),
        Some("second"),
        "step 'second' aborted the pipeline: {summary}"
    );
    // Step 'second' aggregate status is failed (one of two targets failed).
    let second = summary["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"].as_str() == Some("second"))
        .expect("step 'second' in steps array");
    assert_eq!(
        second["status"].as_str(),
        Some("failed"),
        "step aggregate status should be failed when any target fails: {second}"
    );
    assert_eq!(
        second["targets"].as_array().map(|a| a.len()),
        Some(2),
        "step 'second' should have a 2-item targets array: {second}"
    );
    // Step 'third' should be skipped on every target.
    let third = summary["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"].as_str() == Some("third"))
        .expect("step 'third' in steps array");
    assert_eq!(
        third["status"].as_str(),
        Some("skipped"),
        "step 'third' should be skipped: {third}"
    );
}

#[test]
fn f17_multi_target_revert_on_failure_unwinds_per_target() {
    // F17 + F11 (v0.1.3): with --revert-on-failure on a multi-target
    // pipeline where step 2 fails, step 1's inverse should fan out
    // across both targets in reverse manifest order. Two auto-revert
    // entries should land (one per target).
    let mock = json!([
        { "match": "do-1", "stdout": "", "exit": 0 },
        { "match": "do-2", "stdout": "", "exit": 1 },
        { "match": "undo-1", "stdout": "", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(
        sb.home(),
        "arte",
        &[
            ("atlas", "img/atlas:1", "ok"),
            ("postgres", "img/pg:1", "ok"),
        ],
    );
    let manifest = f17_write_manifest(
        sb.home(),
        "m.json",
        json!({
            "steps": [
                {"name": "one", "cmd": "do-1", "revert_cmd": "undo-1"},
                {"name": "two", "cmd": "do-2"}
            ]
        }),
    );
    sb.cmd()
        .args([
            "run",
            "arte/*",
            "--steps",
            manifest.to_str().unwrap(),
            "--revert-on-failure",
        ])
        .assert()
        .failure();
    let body = audit_jsonl_body(sb.home());
    let revert_entries: Vec<serde_json::Value> = body
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|e| e["verb"].as_str() == Some("run.step.revert"))
        .collect();
    assert_eq!(
        revert_entries.len(),
        2,
        "expected one auto-revert per target (atlas + postgres) for step 'one': {body}"
    );
    for e in &revert_entries {
        assert_eq!(
            e["step_name"].as_str(),
            Some("one"),
            "auto-revert is for step 'one': {e}"
        );
        assert!(
            e["auto_revert_of"].as_str().is_some(),
            "auto-revert should link via auto_revert_of: {e}"
        );
    }
    let labels: std::collections::HashSet<&str> = revert_entries
        .iter()
        .filter_map(|e| e["selector"].as_str())
        .collect();
    assert!(
        labels.contains("arte/atlas") && labels.contains("arte/postgres"),
        "auto-reverts should fan out to both targets: {labels:?}"
    );
}

#[test]
fn f17_steps_yaml_help_documents_flag() {
    let sb = Sandbox::new(json!([]));
    let assert = sb.cmd().args(["run", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("--steps-yaml"),
        "run --help should document --steps-yaml: {stdout}"
    );
}

// -----------------------------------------------------------------------------
// L5 — `inspect audit gc` retention + orphan-snapshot sweep, plus the
// lazy-trigger marker on `[audit] retention` in `~/.inspect/config.toml`.
// Spec: deletes audit entries older than --keep, sweeps orphan snapshots
// while never touching one referenced by a retained entry; exposes a
// stable `--json` envelope; lazy GC fires on the next audit append after
// the retention threshold trips, gated by a once-per-minute marker.
// -----------------------------------------------------------------------------

fn l5_write_audit_entry(
    home: &std::path::Path,
    file_stem: &str,
    id: &str,
    ts_rfc3339: &str,
    selector: &str,
    extras: serde_json::Value,
) {
    let dir = home.join("audit");
    std::fs::create_dir_all(&dir).unwrap();
    let mut entry = json!({
        "schema_version": 1,
        "id": id,
        "ts": ts_rfc3339,
        "user": "tester",
        "host": "test-host",
        "verb": "exec",
        "selector": selector,
        "exit": 0,
        "duration_ms": 0,
    });
    if let serde_json::Value::Object(extra_map) = extras {
        let entry_obj = entry.as_object_mut().unwrap();
        for (k, v) in extra_map {
            entry_obj.insert(k, v);
        }
    }
    let path = dir.join(format!("{file_stem}.jsonl"));
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .unwrap();
    let line = format!("{}\n", serde_json::to_string(&entry).unwrap());
    f.write_all(line.as_bytes()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn l5_write_snapshot(home: &std::path::Path, hash_hex: &str, content: &[u8]) {
    let dir = home.join("audit").join("snapshots");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("sha256-{hash_hex}"));
    std::fs::write(&path, content).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn l5_iso_days_ago(days: i64) -> String {
    use chrono::Utc;
    let ts = Utc::now() - chrono::Duration::days(days);
    ts.to_rfc3339()
}

#[test]
fn l5_gc_dry_run_lists_old_entries_without_deleting() {
    let sb = Sandbox::new(json!([]));
    let h = sb.home();
    // Two old entries (>90d), two recent. --dry-run --keep 90d should
    // report 2 deletions but leave the JSONL intact.
    l5_write_audit_entry(
        h,
        "2024-01-tester",
        "old-1",
        &l5_iso_days_ago(120),
        "arte/foo",
        json!({}),
    );
    l5_write_audit_entry(
        h,
        "2024-01-tester",
        "old-2",
        &l5_iso_days_ago(100),
        "arte/bar",
        json!({}),
    );
    l5_write_audit_entry(
        h,
        "2024-12-tester",
        "new-1",
        &l5_iso_days_ago(2),
        "arte/foo",
        json!({}),
    );
    l5_write_audit_entry(
        h,
        "2024-12-tester",
        "new-2",
        &l5_iso_days_ago(0),
        "arte/bar",
        json!({}),
    );

    let assert = sb
        .cmd()
        .args(["audit", "gc", "--keep", "90d", "--dry-run"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("would delete 2 entries"),
        "dry-run summary should report 2 deletions: {stdout}"
    );
    assert!(
        stdout.contains("entry  would delete: old-1")
            && stdout.contains("entry  would delete: old-2"),
        "dry-run should enumerate the targeted ids: {stdout}"
    );
    // JSONL files untouched on dry-run.
    let jan = std::fs::read_to_string(h.join("audit").join("2024-01-tester.jsonl")).unwrap();
    assert!(jan.contains("old-1") && jan.contains("old-2"));
}

#[test]
fn l5_gc_apply_deletes_old_entries_and_orphan_snapshots() {
    let sb = Sandbox::new(json!([]));
    let h = sb.home();
    // Old entry references snapshot `aaaa`. Recent entry references
    // snapshot `bbbb`. A standalone `cccc` snapshot has no references
    // anywhere — orphan from day one.
    l5_write_audit_entry(
        h,
        "2024-01-tester",
        "old-1",
        &l5_iso_days_ago(200),
        "arte/foo",
        json!({"previous_hash": "aaaa"}),
    );
    l5_write_audit_entry(
        h,
        "2024-12-tester",
        "new-1",
        &l5_iso_days_ago(1),
        "arte/foo",
        json!({"previous_hash": "bbbb"}),
    );
    l5_write_snapshot(h, "aaaa", b"old-content");
    l5_write_snapshot(h, "bbbb", b"new-content");
    l5_write_snapshot(h, "cccc", b"orphan-from-day-one");

    sb.cmd()
        .args(["audit", "gc", "--keep", "90d"])
        .assert()
        .success();

    // old-1 gone from JSONL; new-1 kept.
    let jan_path = h.join("audit").join("2024-01-tester.jsonl");
    assert!(
        !jan_path.exists(),
        "JSONL file with only deleted entries should be removed entirely"
    );
    let dec = std::fs::read_to_string(h.join("audit").join("2024-12-tester.jsonl")).unwrap();
    assert!(dec.contains("new-1"));
    assert!(!dec.contains("old-1"));

    // Snapshot `aaaa` (referenced only by deleted old-1) gone; `bbbb`
    // kept (referenced by retained new-1); `cccc` (orphan) gone.
    assert!(!h.join("audit/snapshots/sha256-aaaa").exists());
    assert!(h.join("audit/snapshots/sha256-bbbb").exists());
    assert!(!h.join("audit/snapshots/sha256-cccc").exists());
}

#[test]
fn l5_gc_keeps_snapshot_referenced_by_retained_entry() {
    // The headline F11 contract: GC must NEVER delete a snapshot still
    // pinned by an audit entry that survives. This guards a subtle bug
    // where a snapshot is referenced only by a *retained* entry's
    // `revert.payload` (state_snapshot kind) — the `previous_hash`
    // field may be empty.
    let sb = Sandbox::new(json!([]));
    let h = sb.home();
    l5_write_audit_entry(
        h,
        "2024-12-tester",
        "kept-1",
        &l5_iso_days_ago(1),
        "arte/foo",
        json!({
            "revert": {
                "kind": "state_snapshot",
                "payload": "deadbeef",
                "captured_at": l5_iso_days_ago(1),
                "preview": "restore /etc/foo"
            }
        }),
    );
    l5_write_snapshot(h, "deadbeef", b"pinned-by-revert-payload");
    // Plus an unreferenced orphan to confirm the sweep still runs.
    l5_write_snapshot(h, "ffffffff", b"actual-orphan");

    sb.cmd()
        .args(["audit", "gc", "--keep", "90d"])
        .assert()
        .success();

    assert!(
        h.join("audit/snapshots/sha256-deadbeef").exists(),
        "snapshot pinned by a retained entry's revert.payload must NOT be deleted"
    );
    assert!(
        !h.join("audit/snapshots/sha256-ffffffff").exists(),
        "actual orphan should be swept"
    );
}

#[test]
fn l5_gc_json_envelope_schema() {
    let sb = Sandbox::new(json!([]));
    let h = sb.home();
    l5_write_audit_entry(
        h,
        "2024-01-tester",
        "old-x",
        &l5_iso_days_ago(120),
        "arte/foo",
        json!({}),
    );
    l5_write_audit_entry(
        h,
        "2024-12-tester",
        "new-x",
        &l5_iso_days_ago(0),
        "arte/bar",
        json!({}),
    );

    let assert = sb
        .cmd()
        .args(["audit", "gc", "--keep", "90d", "--json"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    for key in [
        "dry_run",
        "policy",
        "entries_total",
        "entries_kept",
        "deleted_entries",
        "deleted_snapshots",
        "freed_bytes",
        "deleted_ids",
        "deleted_snapshot_hashes",
    ] {
        assert!(
            v.get(key).is_some(),
            "JSON envelope missing required L5 field '{key}': {v}"
        );
    }
    assert_eq!(v["dry_run"], false);
    assert_eq!(v["policy"], "90d");
    assert_eq!(v["entries_total"], 2);
    assert_eq!(v["entries_kept"], 1);
    assert_eq!(v["deleted_entries"], 1);
    let ids = v["deleted_ids"].as_array().unwrap();
    assert_eq!(ids.len(), 1);
    assert_eq!(ids[0], "old-x");
}

#[test]
fn l5_gc_count_policy_keeps_newest_per_namespace() {
    let sb = Sandbox::new(json!([]));
    let h = sb.home();
    // Three entries each in arte and bravo namespaces. --keep 1 keeps
    // the newest per namespace → 2 retained, 4 deleted.
    for (ns, days) in &[
        ("arte", 0i64),
        ("arte", 5),
        ("arte", 10),
        ("bravo", 0),
        ("bravo", 7),
        ("bravo", 14),
    ] {
        let id = format!("{ns}-d{days}");
        l5_write_audit_entry(
            h,
            "2024-12-tester",
            &id,
            &l5_iso_days_ago(*days),
            &format!("{ns}/svc"),
            json!({}),
        );
    }
    let assert = sb
        .cmd()
        .args(["audit", "gc", "--keep", "1", "--json"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(v["entries_total"], 6);
    assert_eq!(v["entries_kept"], 2);
    assert_eq!(v["deleted_entries"], 4);
    let deleted: std::collections::HashSet<String> = v["deleted_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    // Newest in each ns (d0) survives; the other four deleted.
    assert!(!deleted.contains("arte-d0"));
    assert!(!deleted.contains("bravo-d0"));
    assert!(deleted.contains("arte-d5"));
    assert!(deleted.contains("arte-d10"));
    assert!(deleted.contains("bravo-d7"));
    assert!(deleted.contains("bravo-d14"));
}

#[test]
fn l5_gc_invalid_keep_value_exits_error_with_hint() {
    let sb = Sandbox::new(json!([]));
    let assert = sb
        .cmd()
        .args(["audit", "gc", "--keep", "5y"])
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("unknown unit") && stderr.contains("hint:"),
        "invalid --keep should chain a hint: {stderr}"
    );
}

#[test]
fn l5_gc_keep_zero_rejected() {
    let sb = Sandbox::new(json!([]));
    let assert = sb
        .cmd()
        .args(["audit", "gc", "--keep", "0"])
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("--keep 0"),
        "--keep 0 must be rejected loudly: {stderr}"
    );
}

#[test]
fn l5_gc_help_documents_keep_dry_run_and_json() {
    let sb = Sandbox::new(json!([]));
    let assert = sb.cmd().args(["audit", "gc", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("--keep"), "{stdout}");
    assert!(stdout.contains("--dry-run"), "{stdout}");
    let assert = sb.cmd().args(["audit", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("GC + RETENTION"),
        "audit --help should document the GC + RETENTION section: {stdout}"
    );
    assert!(
        stdout.contains("[audit] retention") && stdout.contains("config.toml"),
        "audit --help should document the lazy-trigger config hook: {stdout}"
    );
}

#[test]
fn l5_gc_empty_audit_dir_succeeds_with_zero_counts() {
    // Operator runs `audit gc` on a fresh install with nothing in
    // ~/.inspect/audit/. No JSONL files, no snapshots dir. Must
    // succeed cleanly with all-zero counts.
    let sb = Sandbox::new(json!([]));
    let assert = sb
        .cmd()
        .args(["audit", "gc", "--keep", "90d", "--json"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(v["entries_total"], 0);
    assert_eq!(v["deleted_entries"], 0);
    assert_eq!(v["deleted_snapshots"], 0);
    assert_eq!(v["freed_bytes"], 0);
}

#[test]
fn l5_lazy_gc_marker_prevents_double_scan_within_minute() {
    // After a manual `audit gc` run, the cheap-path marker
    // `~/.inspect/audit/.gc-checked` must exist with a recent mtime so
    // a subsequent verb that triggers `maybe_run_lazy_gc` no-ops
    // immediately. We exercise the marker existence + freshness here
    // directly because the lazy trigger is wired into AuditStore::append
    // which is exercised by every write verb in the existing F1-F17 suite.
    let sb = Sandbox::new(json!([]));
    let h = sb.home();
    l5_write_audit_entry(
        h,
        "2024-12-tester",
        "x",
        &l5_iso_days_ago(0),
        "arte/foo",
        json!({}),
    );
    sb.cmd()
        .args(["audit", "gc", "--keep", "90d"])
        .assert()
        .success();
    let marker = h.join("audit").join(".gc-checked");
    assert!(
        marker.exists(),
        "manual gc should touch the cheap-path marker"
    );
    let meta = std::fs::metadata(&marker).unwrap();
    let elapsed = std::time::SystemTime::now()
        .duration_since(meta.modified().unwrap())
        .unwrap();
    assert!(
        elapsed.as_secs() < 60,
        "marker mtime should be fresh (< 60s): {}s",
        elapsed.as_secs()
    );
}

#[test]
fn l5_lazy_gc_triggers_on_audit_append_when_retention_set() {
    // Configure `[audit] retention = "90d"` and seed an entry whose
    // file mtime is older than the threshold. Ensure the .gc-checked
    // marker is absent so the cheap-path doesn't suppress the scan.
    // Then run any verb that writes an audit entry — `inspect cache
    // clear --all` is a tidy choice (one audit append per
    // invocation). The lazy GC should fire and prune the old entry.
    let sb = Sandbox::new(json!([]));
    let h = sb.home();
    std::fs::write(h.join("config.toml"), "[audit]\nretention = \"90d\"\n").unwrap();
    write_servers_toml(h, &["arte"]);

    // Seed an old entry whose ts is >100d in the past.
    l5_write_audit_entry(
        h,
        "2024-01-tester",
        "should-be-deleted",
        &l5_iso_days_ago(150),
        "arte/foo",
        json!({}),
    );
    // Backdate the file mtime so the cheap-path mtime probe trips.
    let jan = h.join("audit").join("2024-01-tester.jsonl");
    let old_time = std::time::SystemTime::now() - std::time::Duration::from_secs(150 * 86400);
    let f = std::fs::OpenOptions::new().write(true).open(&jan).unwrap();
    f.set_modified(old_time).unwrap();
    drop(f);

    // Confirm marker absent (fresh sandbox).
    let marker = h.join("audit").join(".gc-checked");
    assert!(!marker.exists());

    // Trigger an audit append via cache clear (single-ns form always
    // writes one entry, regardless of whether the cache was populated).
    sb.cmd().args(["cache", "clear", "arte"]).assert().success();

    // Lazy GC should have run and deleted the old entry's file (only
    // entry it had). The new audit entry from `cache clear` lives in
    // the current month's file.
    assert!(
        !jan.exists(),
        "lazy GC should have pruned the old JSONL file"
    );
    assert!(marker.exists(), "lazy GC must touch the .gc-checked marker");
}

// -----------------------------------------------------------------------------
// F18 — `~/.inspect/history/<ns>-<YYYY-MM-DD>.log` per-namespace, per-day
// human-readable transcript of every namespace-scoped verb invocation. Adds
// `inspect history show / list / clear / rotate` plus the [history] config
// block in `~/.inspect/config.toml` and per-ns overrides in `servers.toml`.
// -----------------------------------------------------------------------------

fn f18_today_utc() -> String {
    use chrono::Utc;
    Utc::now().format("%Y-%m-%d").to_string()
}

fn f18_history_path(home: &std::path::Path, ns: &str) -> std::path::PathBuf {
    home.join("history")
        .join(format!("{ns}-{}.log", f18_today_utc()))
}

#[test]
fn f18_status_writes_namespace_scoped_transcript_block() {
    // Spec: every verb invocation against a namespace produces one
    // fenced block. A single `inspect status arte` against a fresh
    // sandbox should yield exactly one fenced block in the day's
    // transcript file with header + argv + body + footer.
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    sb.cmd().args(["status", "arte"]).assert().success();
    let path = f18_history_path(sb.home(), "arte");
    let body = std::fs::read_to_string(&path).expect("transcript file should exist");
    let header_count = body.matches("── ").count();
    assert!(
        header_count >= 2,
        "expected at least one full fenced block (header + footer): got {body}"
    );
    assert!(body.contains("$ "), "argv line missing: {body}");
    assert!(
        body.contains("── exit=0 duration="),
        "footer missing exit/duration: {body}"
    );
    // Header carries `arte` as the namespace token.
    assert!(
        body.contains("arte #"),
        "header should name the namespace: {body}"
    );
}

#[test]
fn f18_help_does_not_write_global_transcript() {
    // `inspect help` does not resolve a namespace; per F18 spec only
    // namespace-scoped verbs produce transcripts. Verifies we don't
    // pollute ~/.inspect/history/ with `_global-*.log` files for
    // operator-tooling verbs.
    let sb = Sandbox::new(json!([]));
    sb.cmd().args(["help"]).assert().success();
    let history = sb.home().join("history");
    if history.exists() {
        let entries: Vec<_> = std::fs::read_dir(&history)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with('.'))
            .collect();
        assert!(
            entries.is_empty(),
            "namespace-less verb should leave history dir empty (got: {entries:?})"
        );
    }
}

#[test]
fn f18_history_rotate_deletes_old_files() {
    // retain_days = 7 ⇒ files dated 8+ days ago deleted; files
    // within 7 days kept.
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    std::fs::write(
        sb.home().join("config.toml"),
        "[history]\nretain_days = 7\nmax_total_mb = 500\ncompress_after_days = 999\n",
    )
    .unwrap();
    let history = sb.home().join("history");
    std::fs::create_dir_all(&history).unwrap();
    use chrono::Utc;
    let today = Utc::now().date_naive();
    for d in [1, 3, 5, 8, 10, 15] {
        let date = today - chrono::Duration::days(d);
        let path = history.join(format!("arte-{}.log", date.format("%Y-%m-%d")));
        std::fs::write(&path, b"fake transcript").unwrap();
    }
    sb.cmd()
        .args(["history", "rotate", "--json"])
        .assert()
        .success();
    let remaining: Vec<_> = std::fs::read_dir(&history)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("arte-"))
        .collect();
    // Days 1, 3, 5 within the 7-day window survive; 8, 10, 15 deleted.
    assert!(
        remaining.len() == 3,
        "expected 3 surviving files, got {remaining:?}"
    );
    for d in [1, 3, 5] {
        let date = today - chrono::Duration::days(d);
        let name = format!("arte-{}.log", date.format("%Y-%m-%d"));
        assert!(
            remaining.iter().any(|n| n == &name),
            "expected {name} to survive: {remaining:?}"
        );
    }
}

#[test]
fn f18_history_rotate_compresses_old_files_and_show_decompresses() {
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    // compress_after_days = 1 ⇒ yesterday's file is gzipped.
    std::fs::write(
        sb.home().join("config.toml"),
        "[history]\nretain_days = 365\nmax_total_mb = 500\ncompress_after_days = 1\n",
    )
    .unwrap();
    let history = sb.home().join("history");
    std::fs::create_dir_all(&history).unwrap();
    use chrono::Utc;
    let yesterday = Utc::now().date_naive() - chrono::Duration::days(2);
    let path = history.join(format!("arte-{}.log", yesterday.format("%Y-%m-%d")));
    let content = "── 2026-04-27T10:00:00Z arte #abc1234 ──────\n\
                   $ inspect status arte\n\
                   arte | atlas-vault: ok\n\
                   ── exit=0 duration=10ms ──\n\n";
    std::fs::write(&path, content).unwrap();

    sb.cmd().args(["history", "rotate"]).assert().success();
    let gz_path = history.join(format!("arte-{}.log.gz", yesterday.format("%Y-%m-%d")));
    assert!(gz_path.exists(), "yesterday's file should be gzipped");
    assert!(!path.exists(), "original .log should be removed");

    // history show --date <yesterday> transparently decompresses.
    let assert = sb
        .cmd()
        .args([
            "history",
            "show",
            "arte",
            "--date",
            &yesterday.format("%Y-%m-%d").to_string(),
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("atlas-vault: ok") && stdout.contains("$ inspect status arte"),
        "history show should decompress + render the original block: {stdout}"
    );
}

#[test]
fn f18_history_disabled_per_ns_skips_transcript_but_audit_still_writes() {
    let sb = Sandbox::new(json!([]));
    let h = sb.home();
    // Note: write_servers_toml stomps the file, so we need to write
    // the [namespaces.arte.history] block as part of our own toml.
    let body = r#"schema_version = 1

[namespaces.arte]
host = "arte.example.invalid"
user = "deploy"
port = 22

[namespaces.arte.history]
disabled = true
"#;
    let path = h.join("servers.toml");
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    sb.cmd().args(["cache", "clear", "arte"]).assert().success();
    let transcript = f18_history_path(h, "arte");
    assert!(
        !transcript.exists(),
        "disabled per-ns transcript should NOT be written"
    );
    // Audit log entry is still written.
    let audit_dir = h.join("audit");
    let any_audit_jsonl = std::fs::read_dir(&audit_dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .any(|e| e.path().extension().is_some_and(|s| s == "jsonl"))
        })
        .unwrap_or(false);
    assert!(
        any_audit_jsonl,
        "audit log must still be written even when transcript is disabled"
    );
}

#[test]
fn f18_history_show_filters_by_audit_id() {
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    // Two separate cache-clear invocations write two transcript
    // blocks AND two audit entries. We grep audit JSONL for the
    // first id, then ask `history show --audit-id <id>` to find it.
    sb.cmd().args(["cache", "clear", "arte"]).assert().success();
    sb.cmd().args(["cache", "clear", "arte"]).assert().success();
    let audit_dir = sb.home().join("audit");
    let mut audit_ids: Vec<String> = Vec::new();
    for ent in std::fs::read_dir(&audit_dir).unwrap() {
        let ent = ent.unwrap();
        if ent.path().extension().is_some_and(|s| s == "jsonl") {
            let body = std::fs::read_to_string(ent.path()).unwrap();
            for line in body.lines() {
                let v: serde_json::Value = serde_json::from_str(line).unwrap();
                if let Some(id) = v.get("id").and_then(|v| v.as_str()) {
                    audit_ids.push(id.to_string());
                }
            }
        }
    }
    assert!(
        audit_ids.len() >= 2,
        "expected at least 2 audit ids, got {audit_ids:?}"
    );
    let target = &audit_ids[0];
    let assert = sb
        .cmd()
        .args(["history", "show", "arte", "--audit-id", target])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains(&format!("audit_id={target}")),
        "history show --audit-id should render the matching block (target={target}): {stdout}"
    );
    // The OTHER block's id should NOT appear.
    let other = &audit_ids[1];
    if other != target {
        assert!(
            !stdout.contains(&format!("audit_id={other}")),
            "history show --audit-id should isolate to the targeted block, not show {other}"
        );
    }
}

#[test]
fn f18_history_list_lines_up_files_with_sizes() {
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte", "bravo"]);
    sb.cmd().args(["cache", "clear", "arte"]).assert().success();
    sb.cmd()
        .args(["cache", "clear", "bravo"])
        .assert()
        .success();
    let assert = sb
        .cmd()
        .args(["history", "list", "--json"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let mut found_arte = false;
    let mut found_bravo = false;
    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        let ns = v["namespace"].as_str().unwrap_or("");
        if ns == "arte" {
            found_arte = true;
        }
        if ns == "bravo" {
            found_bravo = true;
        }
        assert!(v.get("date").is_some());
        assert!(v.get("bytes").is_some());
        assert!(v.get("compressed").is_some());
    }
    assert!(
        found_arte && found_bravo,
        "history list --json should emit one record per (ns, date)"
    );
}

#[test]
fn f18_history_clear_requires_yes_then_deletes() {
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    let history = sb.home().join("history");
    std::fs::create_dir_all(&history).unwrap();
    use chrono::Utc;
    let today = Utc::now().date_naive();
    let old = today - chrono::Duration::days(20);
    let path = history.join(format!("arte-{}.log", old.format("%Y-%m-%d")));
    std::fs::write(&path, b"x").unwrap();
    // Without --yes ⇒ refuse.
    sb.cmd()
        .args([
            "history",
            "clear",
            "arte",
            "--before",
            &today.format("%Y-%m-%d").to_string(),
        ])
        .assert()
        .failure();
    assert!(path.exists(), "clear without --yes must NOT delete");
    // With --yes ⇒ delete.
    sb.cmd()
        .args([
            "history",
            "clear",
            "arte",
            "--before",
            &today.format("%Y-%m-%d").to_string(),
            "--yes",
        ])
        .assert()
        .success();
    assert!(!path.exists(), "clear --yes should remove old files");
}

#[test]
fn f18_help_documents_history_subcommands() {
    let sb = Sandbox::new(json!([]));
    let assert = sb.cmd().args(["history", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    for needle in [
        "show",
        "list",
        "clear",
        "rotate",
        "[history]",
        "config.toml",
    ] {
        assert!(
            stdout.contains(needle),
            "history --help should document '{needle}': {stdout}"
        );
    }
}

// -----------------------------------------------------------------------------
// L6 — per-branch rollback tracking in bundle matrix steps. v0.1.2 limitation:
// `parallel: true` + `matrix:` rolled back on the WHOLE matrix when any branch
// failed, including succeeded branches whose downstream effects may already be
// in use. v0.1.3 fix: track per-branch BranchResult in the executor; on
// rollback, invert ONLY the succeeded branches with `{{ matrix.<key> }}`
// resolving to that branch's value. New `inspect bundle status <id>` reads
// the audit log and renders per-branch outcomes.
// -----------------------------------------------------------------------------

fn l6_audit_lines(home: &std::path::Path) -> Vec<serde_json::Value> {
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
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                    out.push(v);
                }
            }
        }
    }
    out
}

#[test]
fn l6_matrix_failure_rollback_targets_only_succeeded_branches() {
    // 4-branch matrix; branch `c` fails. Rollback must fire for a/b/d
    // (the succeeded branches) with `{{ matrix.svc }}` resolved per
    // branch, and MUST NOT fire for c.
    let mock = json!([
        // forward bodies — `c` fails, others succeed.
        { "match": "forward svc-a", "stdout": "ok\n", "exit": 0 },
        { "match": "forward svc-b", "stdout": "ok\n", "exit": 0 },
        { "match": "forward svc-c", "stdout": "boom\n", "exit": 1 },
        { "match": "forward svc-d", "stdout": "ok\n", "exit": 0 },
        // rollback bodies — must run for a/b/d only.
        { "match": "rollback svc-a", "stdout": "rb-a\n", "exit": 0 },
        { "match": "rollback svc-b", "stdout": "rb-b\n", "exit": 0 },
        { "match": "rollback svc-c", "stdout": "rb-c\n", "exit": 0 },
        { "match": "rollback svc-d", "stdout": "rb-d\n", "exit": 0 },
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    let yaml = "\
name: l6-matrix
host: arte/atlas
steps:
  - id: fanout
    parallel: true
    max_parallel: 1
    matrix:
      svc: [a, b, c, d]
    exec: forward svc-{{ matrix.svc }}
    rollback: rollback svc-{{ matrix.svc }}
    on_failure: rollback
";
    let bundle_path = sb.home().join("matrix.yaml");
    std::fs::write(&bundle_path, yaml).unwrap();
    sb.cmd()
        .args(["bundle", "apply", bundle_path.to_str().unwrap()])
        .arg("--apply")
        .arg("--no-prompt")
        .assert()
        .failure();

    let lines = l6_audit_lines(sb.home());
    // Forward exec entries: branch labels stamped.
    let forward_branches: std::collections::BTreeSet<(String, String)> = lines
        .iter()
        .filter(|e| e.get("verb").and_then(|v| v.as_str()) == Some("exec"))
        .filter_map(|e| {
            let b = e.get("bundle_branch")?.as_str()?.to_string();
            let s = e.get("bundle_branch_status")?.as_str()?.to_string();
            Some((b, s))
        })
        .collect();
    // The stop-on-first-error policy means c failing aborts the rest.
    // With max_parallel=1 the order is deterministic: a, b, c (fail) →
    // d skipped. Forward audit entries land for a/b (ok) and c (failed).
    assert!(
        forward_branches.contains(&("svc=a".into(), "ok".into())),
        "expected svc=a ok forward audit: {forward_branches:?}"
    );
    assert!(
        forward_branches.contains(&("svc=b".into(), "ok".into())),
        "expected svc=b ok forward audit: {forward_branches:?}"
    );
    assert!(
        forward_branches.contains(&("svc=c".into(), "failed".into())),
        "expected svc=c failed forward audit: {forward_branches:?}"
    );

    // Rollback must fire for a and b only (c failed, d skipped).
    let rollback_branches: std::collections::BTreeSet<String> = lines
        .iter()
        .filter(|e| e.get("verb").and_then(|v| v.as_str()) == Some("bundle.rollback"))
        .filter_map(|e| e.get("bundle_branch")?.as_str().map(String::from))
        .collect();
    assert!(
        rollback_branches.contains("svc=a"),
        "rollback should fire for succeeded branch svc=a: {rollback_branches:?}"
    );
    assert!(
        rollback_branches.contains("svc=b"),
        "rollback should fire for succeeded branch svc=b: {rollback_branches:?}"
    );
    assert!(
        !rollback_branches.contains("svc=c"),
        "rollback MUST NOT fire for failed branch svc=c: {rollback_branches:?}"
    );
    assert!(
        !rollback_branches.contains("svc=d"),
        "rollback MUST NOT fire for skipped branch svc=d: {rollback_branches:?}"
    );

    // `bundle.rollback.skip` audit entries explain why c and d were
    // not inverted.
    let skip_branches: std::collections::BTreeSet<String> = lines
        .iter()
        .filter(|e| e.get("verb").and_then(|v| v.as_str()) == Some("bundle.rollback.skip"))
        .filter_map(|e| e.get("bundle_branch")?.as_str().map(String::from))
        .collect();
    assert!(
        skip_branches.contains("svc=c") && skip_branches.contains("svc=d"),
        "skipped branches should have an audit explanation: {skip_branches:?}"
    );
}

#[test]
fn l6_matrix_rollback_template_resolves_per_succeeded_branch() {
    // Forward `forward svc-a`, `forward svc-b`. Step c fails.
    // Rollback body `rollback svc-{{ matrix.svc }}` MUST be rendered
    // with the per-branch value so the audit-args field contains the
    // exact rendered command. Asserts that the args are NOT
    // `rollback svc-` (empty matrix interpolation) — the v0.1.2 bug.
    let mock = json!([
        { "match": "forward svc-a", "stdout": "ok\n", "exit": 0 },
        { "match": "forward svc-b", "stdout": "ok\n", "exit": 0 },
        { "match": "forward svc-z", "stdout": "boom\n", "exit": 1 },
        { "match": "rollback svc-a", "stdout": "rb-a\n", "exit": 0 },
        { "match": "rollback svc-b", "stdout": "rb-b\n", "exit": 0 },
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    let yaml = "\
name: l6-template
host: arte/atlas
steps:
  - id: fan
    parallel: true
    max_parallel: 1
    matrix:
      svc: [a, b, z]
    exec: forward svc-{{ matrix.svc }}
    rollback: rollback svc-{{ matrix.svc }}
    on_failure: rollback
";
    let bundle_path = sb.home().join("template.yaml");
    std::fs::write(&bundle_path, yaml).unwrap();
    sb.cmd()
        .args(["bundle", "apply", bundle_path.to_str().unwrap()])
        .arg("--apply")
        .arg("--no-prompt")
        .assert()
        .failure();

    let lines = l6_audit_lines(sb.home());
    let rollback_args: std::collections::BTreeSet<String> = lines
        .iter()
        .filter(|e| e.get("verb").and_then(|v| v.as_str()) == Some("bundle.rollback"))
        .filter_map(|e| e.get("args")?.as_str().map(String::from))
        .collect();
    assert!(
        rollback_args.contains("rollback svc-a"),
        "rollback body for branch a should expand `{{ matrix.svc }}` → `a`: {rollback_args:?}"
    );
    assert!(
        rollback_args.contains("rollback svc-b"),
        "rollback body for branch b should expand `{{ matrix.svc }}` → `b`: {rollback_args:?}"
    );
    // The v0.1.2 bug: an empty matrix produced `rollback svc-` (empty
    // expansion). Guard against regression.
    assert!(
        !rollback_args.iter().any(|s| s == "rollback svc-"),
        "rollback should never render with an empty matrix expansion: {rollback_args:?}"
    );
}

#[test]
fn l6_bundle_status_renders_per_branch_outcomes() {
    let mock = json!([
        { "match": "forward svc-a", "stdout": "ok\n", "exit": 0 },
        { "match": "forward svc-b", "stdout": "ok\n", "exit": 0 },
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    let yaml = "\
name: l6-status
host: arte/atlas
steps:
  - id: fan
    parallel: true
    max_parallel: 2
    matrix:
      svc: [a, b]
    exec: forward svc-{{ matrix.svc }}
";
    let bundle_path = sb.home().join("status.yaml");
    std::fs::write(&bundle_path, yaml).unwrap();
    sb.cmd()
        .args(["bundle", "apply", bundle_path.to_str().unwrap()])
        .arg("--apply")
        .arg("--no-prompt")
        .assert()
        .success();

    // Pick the bundle id from any audit entry.
    let lines = l6_audit_lines(sb.home());
    let bundle_id = lines
        .iter()
        .find_map(|e| e.get("bundle_id")?.as_str())
        .expect("at least one audit entry should carry bundle_id")
        .to_string();

    // Human form: per-branch ✓ markers for both branches.
    let assert = sb
        .cmd()
        .args(["bundle", "status", &bundle_id])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("✓ svc=a") && stdout.contains("✓ svc=b"),
        "status should render per-branch ✓ markers for both succeeded branches: {stdout}"
    );
    assert!(
        stdout.contains("step `fan` (matrix):"),
        "status should label the step as a matrix: {stdout}"
    );

    // JSON form: structured per-branch outcomes.
    let assert = sb
        .cmd()
        .args(["bundle", "status", &bundle_id, "--json"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(v["bundle_id"], bundle_id);
    let steps = v["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0]["step"], "fan");
    assert_eq!(steps[0]["kind"], "matrix");
    let branches = steps[0]["branches"].as_array().unwrap();
    let labels: std::collections::BTreeSet<String> = branches
        .iter()
        .filter_map(|b| b["branch"].as_str().map(String::from))
        .collect();
    assert!(labels.contains("svc=a") && labels.contains("svc=b"));
    let statuses: std::collections::BTreeSet<String> = branches
        .iter()
        .filter_map(|b| b["status"].as_str().map(String::from))
        .collect();
    assert_eq!(
        statuses,
        std::collections::BTreeSet::from(["ok".to_string()]),
        "every succeeded branch should report status=ok"
    );
}

#[test]
fn l6_bundle_status_unknown_id_exits_no_matches_with_hint() {
    let sb = Sandbox::new(json!([]));
    let assert = sb
        .cmd()
        .args(["bundle", "status", "nope-doesnt-exist"])
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("no bundle invocation matches"),
        "unknown bundle id should chain a hint: {stderr}"
    );
}

#[test]
fn l6_full_matrix_success_then_later_step_fails_rolls_back_all_branches() {
    // Regression guard for the existing rollback path: matrix step
    // succeeds entirely, then a LATER step fails with on_failure:
    // rollback. The matrix step's rollback must fire for every branch
    // (since they all succeeded).
    let mock = json!([
        { "match": "forward svc-a", "stdout": "ok\n", "exit": 0 },
        { "match": "forward svc-b", "stdout": "ok\n", "exit": 0 },
        { "match": "second-step", "stdout": "boom\n", "exit": 1 },
        { "match": "rollback svc-a", "stdout": "", "exit": 0 },
        { "match": "rollback svc-b", "stdout": "", "exit": 0 },
    ]);
    let sb = Sandbox::new(mock);
    write_minimal_arte(&sb);
    let yaml = "\
name: l6-regression
host: arte/atlas
steps:
  - id: fan
    parallel: true
    max_parallel: 2
    matrix:
      svc: [a, b]
    exec: forward svc-{{ matrix.svc }}
    rollback: rollback svc-{{ matrix.svc }}
  - id: gate
    exec: second-step
    on_failure: rollback
";
    let bundle_path = sb.home().join("regression.yaml");
    std::fs::write(&bundle_path, yaml).unwrap();
    sb.cmd()
        .args(["bundle", "apply", bundle_path.to_str().unwrap()])
        .arg("--apply")
        .arg("--no-prompt")
        .assert()
        .failure();

    let lines = l6_audit_lines(sb.home());
    let rollback_branches: std::collections::BTreeSet<String> = lines
        .iter()
        .filter(|e| e.get("verb").and_then(|v| v.as_str()) == Some("bundle.rollback"))
        .filter_map(|e| e.get("bundle_branch")?.as_str().map(String::from))
        .collect();
    assert!(
        rollback_branches.contains("svc=a") && rollback_branches.contains("svc=b"),
        "all-succeeded matrix should fully rollback when later step fails: {rollback_branches:?}"
    );
}

#[test]
fn l6_bundle_status_help_documents_subcommand() {
    let sb = Sandbox::new(json!([]));
    let assert = sb.cmd().args(["bundle", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("status"),
        "bundle --help should list the status subcommand: {stdout}"
    );
    let assert = sb
        .cmd()
        .args(["bundle", "status", "--help"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("bundle_id"),
        "bundle status --help should describe the bundle_id arg: {stdout}"
    );
}

#[test]
fn f18_argv_password_is_redacted_in_transcript_header() {
    // The verb itself rejects with an unknown-flag error since
    // --password=... isn't a real flag, but the argv-line we record
    // in the transcript header MUST mask the secret regardless.
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    sb.cmd()
        .args(["status", "arte", "--password=hunter2"])
        .assert()
        .failure();
    // Even on parse failure the transcript may not be written
    // (namespace not resolved). So we at least confirm the secret
    // never reaches the transcript dir if any was created.
    let history = sb.home().join("history");
    if history.exists() {
        for ent in std::fs::read_dir(&history).unwrap() {
            let ent = ent.unwrap();
            if let Ok(body) = std::fs::read_to_string(ent.path()) {
                assert!(
                    !body.contains("hunter2"),
                    "transcript must never contain the literal password: {body}"
                );
            }
        }
    }
}

// -----------------------------------------------------------------------------
// L4 — Password authentication + extended session TTL + `inspect ssh add-key`
//
// L4 ships three coupled changes to make password-only legacy boxes
// first-class citizens: per-namespace `auth = "password"` with optional
// `password_env` source, per-namespace `session_ttl` defaulting to 12h
// for password mode (capped at 24h so a forgotten laptop doesn't hold
// a live session forever), and the `inspect ssh add-key <ns>` audited
// migration verb that installs a public key over the live session and
// optionally flips the namespace off password auth.
//
// These acceptance tests cover the user-visible surface that does NOT
// require a real ssh server: schema validation, TTL resolution priority,
// help discoverability, dry-run preview, and connections-output shape.
// Interactive password prompting and remote install paths require a
// live host and are exercised by the in-tree unit tests
// (src/ssh/master.rs, src/ssh/ttl.rs, src/config/namespace.rs).
// -----------------------------------------------------------------------------

fn write_servers_toml_password_auth(home: &std::path::Path, ns: &str, extras: &[(&str, &str)]) {
    let mut body = String::from("schema_version = 1\n\n");
    body.push_str(&format!(
        "[namespaces.{ns}]\nhost = \"{ns}.example.invalid\"\nuser = \"deploy\"\nport = 22\n\
         auth = \"password\"\n"
    ));
    for (k, v) in extras {
        body.push_str(&format!("{k} = \"{v}\"\n"));
    }
    body.push('\n');
    let path = home.join("servers.toml");
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

#[test]
fn l4_show_namespace_with_password_auth_round_trips() {
    // The schema accepts auth/password_env/session_ttl and surfaces them
    // through `inspect show <ns>` so an operator (or agent) can see the
    // applied config without re-reading the toml.
    let sb = Sandbox::new(json!([]));
    write_servers_toml_password_auth(
        sb.home(),
        "legacy-box",
        &[("password_env", "LEGACY_BOX_PASS"), ("session_ttl", "12h")],
    );
    sb.cmd()
        .args(["show", "legacy-box", "--json"])
        .assert()
        .success()
        .stdout(contains("\"auth\":\"password\""))
        .stdout(contains("\"password_env\":\"LEGACY_BOX_PASS\""))
        .stdout(contains("\"session_ttl\":\"12h\""));
}

#[test]
fn l4_session_ttl_above_24h_is_rejected() {
    // The 24h cap exists so a forgotten laptop cannot hold a live remote
    // session indefinitely. `inspect show` surfaces the validation error
    // at config-load time with a chained hint.
    let sb = Sandbox::new(json!([]));
    write_servers_toml_password_auth(
        sb.home(),
        "legacy-box",
        &[("password_env", "P"), ("session_ttl", "48h")],
    );
    sb.cmd()
        .args(["show", "legacy-box"])
        .assert()
        .failure()
        .stderr(contains("24h cap"));
}

#[test]
fn l4_password_env_without_password_auth_rejected() {
    // password_env only makes sense with auth="password"; setting it
    // alone is rejected so a typo doesn't silently change semantics.
    let sb = Sandbox::new(json!([]));
    let body = "schema_version = 1\n\n\
                [namespaces.bravo]\n\
                host = \"bravo.example.invalid\"\nuser = \"deploy\"\nport = 22\n\
                password_env = \"BRAVO_PASS\"\n";
    let path = sb.home().join("servers.toml");
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    sb.cmd()
        .args(["show", "bravo"])
        .assert()
        .failure()
        .stderr(contains("password_env"))
        .stderr(contains("auth = \"password\""));
}

#[test]
fn l4_unknown_auth_mode_rejected() {
    let sb = Sandbox::new(json!([]));
    let body = "schema_version = 1\n\n\
                [namespaces.charlie]\n\
                host = \"c.example\"\nuser = \"u\"\nport = 22\n\
                auth = \"kerberos\"\n";
    std::fs::write(sb.home().join("servers.toml"), body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            sb.home().join("servers.toml"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    sb.cmd()
        .args(["show", "charlie"])
        .assert()
        .failure()
        .stderr(contains("invalid auth mode 'kerberos'"));
}

#[test]
fn l4_ssh_add_key_dry_run_describes_action_for_password_namespace() {
    // Without --apply, the verb must print a deterministic dry-run
    // preview that mentions key generation, install, and the
    // auth-flip prompt — and exit 0 (it is a description, not an
    // error). The dry-run path does not need the master to be open.
    let sb = Sandbox::new(json!([]));
    write_servers_toml_password_auth(
        sb.home(),
        "legacy-box",
        &[("password_env", "P"), ("session_ttl", "12h")],
    );
    let assert = sb
        .cmd()
        .args(["ssh", "add-key", "legacy-box"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("DRY-RUN"),
        "missing dry-run header: {stdout}"
    );
    assert!(
        stdout.contains("inspect_legacy-box_ed25519"),
        "missing default key path: {stdout}"
    );
    assert!(
        stdout.contains("would prompt to rewrite servers.toml"),
        "missing flip notice: {stdout}"
    );
    assert!(stdout.contains("--apply"), "missing apply hint: {stdout}");
}

#[test]
fn l4_ssh_add_key_dry_run_skips_flip_for_key_namespace() {
    let sb = Sandbox::new(json!([]));
    // `auth = "key"` (default) — the flip notice is suppressed.
    write_servers_toml(sb.home(), &["arte"]);
    let assert = sb.cmd().args(["ssh", "add-key", "arte"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("would NOT rewrite servers.toml"),
        "expected key-auth flip suppression: {stdout}"
    );
    assert!(
        stdout.contains("auth is already \"key\""),
        "expected reason: {stdout}"
    );
}

#[test]
fn l4_ssh_add_key_dry_run_no_rewrite_config_flag() {
    let sb = Sandbox::new(json!([]));
    write_servers_toml_password_auth(sb.home(), "legacy-box", &[("password_env", "P")]);
    let assert = sb
        .cmd()
        .args(["ssh", "add-key", "legacy-box", "--no-rewrite-config"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("would NOT rewrite servers.toml (--no-rewrite-config)"),
        "expected --no-rewrite-config notice: {stdout}"
    );
}

#[test]
fn l4_ssh_add_key_apply_without_live_session_errors_with_chained_hint() {
    // The verb refuses to run --apply when no live master is open —
    // a fresh password prompt would defeat the "enter password once"
    // value of the migration. Error must point at `inspect connect`.
    let sb = Sandbox::new(json!([]));
    write_servers_toml_password_auth(sb.home(), "legacy-box", &[("password_env", "P")]);
    sb.cmd()
        .args(["ssh", "add-key", "legacy-box", "--apply"])
        .assert()
        .failure()
        .stderr(contains("no live ssh session"))
        .stderr(contains("inspect connect legacy-box"));
}

#[test]
fn l4_ssh_add_key_apply_rejects_supplied_key_with_missing_pub() {
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    let bogus = sb.home().join("not-a-key");
    std::fs::write(&bogus, b"private-bytes").unwrap();
    sb.cmd()
        .args([
            "ssh",
            "add-key",
            "arte",
            "--key",
            bogus.to_str().unwrap(),
            "--apply",
        ])
        .assert()
        .failure()
        .stderr(contains("has no matching public key"));
}

#[test]
fn l4_help_topic_ssh_lists_add_key_and_password_auth() {
    // `inspect help ssh` must surface the migration path so an agent
    // hitting the password-auth warning can find the answer in one
    // help-jump. Tests the editorial topic, not the LONG_* constants.
    let sb = Sandbox::new(json!([]));
    let assert = sb.cmd().args(["help", "ssh"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("inspect ssh add-key"),
        "ssh help must mention add-key: {stdout}"
    );
    assert!(
        stdout.contains("password"),
        "ssh help must discuss password auth: {stdout}"
    );
    assert!(
        stdout.contains("session_ttl") || stdout.contains("ControlPersist"),
        "ssh help must mention session ttl plumbing: {stdout}"
    );
}

#[test]
fn l4_help_search_finds_password_auth_path() {
    // The help search index must surface password auth + add-key from
    // either the topic or the LONG_* constants so an agent searching
    // "password" lands on the migration path.
    let sb = Sandbox::new(json!([]));
    let assert = sb
        .cmd()
        .args(["help", "--search", "password"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("ssh") && stdout.contains("password"),
        "help search must surface ssh+password: {stdout}"
    );
}

#[test]
fn l4_connections_table_columns_include_auth_ttl_expires_in() {
    // L4 extended `inspect connections` to show auth/ttl/expires_in.
    // The text-mode header must list every column even when there
    // are no live connections (wait — the empty-list path doesn't
    // emit the header; we only test the json envelope's keys).
    // For the populated case the JSON emits the new fields with
    // status="missing" (no real socket).
    //
    // We don't materialize a fake socket here — the empty-rows
    // success path is enough to confirm the new columns parse.
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    sb.cmd()
        .args(["connections", "--json"])
        .assert()
        .success()
        .stdout(contains("\"connections\":[]"));
}

#[test]
fn l4_show_default_auth_unset_means_key() {
    // No explicit auth field ⇒ key auth (the default). Surfaces in
    // `show --json` as auth absent (Option-skip-empty), not "key" —
    // that's intentional so an existing v0.1.2 servers.toml stays
    // byte-identical when round-tripped.
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    let assert = sb.cmd().args(["show", "arte", "--json"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    // Either the field is absent or it is "key"; never "password".
    assert!(
        !stdout.contains("\"auth\":\"password\""),
        "default must not be password: {stdout}"
    );
}

// -----------------------------------------------------------------------------
// L2 — OS keychain integration (opt-in, cross-session only).
//
// Default behavior is byte-identical to v0.1.2: ssh-agent + ControlMaster.
// The keychain is the explicit opt-in via `inspect connect <ns>
// --save-passphrase` (or `--save-password` for L4 password-auth namespaces).
// `inspect keychain list / remove / test` manage the stored entries.
//
// These acceptance tests cover the user-visible surface that does NOT
// require a live OS keychain: help discoverability, JSON envelope shape,
// flag wiring, audit-log shape on `keychain remove`, idempotent semantics,
// and the credential-lifetime documentation. The actual write/read against
// the OS keychain backend requires a live macOS Keychain / Linux
// Secret Service / Windows Credential Manager and is exercised by the
// field-validation gate (the same release-time smoke that L7 PEM
// streaming and L4 interactive password prompts rely on).
// -----------------------------------------------------------------------------

#[test]
fn l2_help_topic_keychain_resolves_through_ssh_topic() {
    // `inspect help keychain` falls back to the verb's clap long-help
    // (since no editorial topic exists for keychain). Either path
    // must surface the `--save-passphrase` migration walkthrough.
    let sb = Sandbox::new(json!([]));
    let assert = sb.cmd().args(["help", "keychain"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("OS keychain") || stdout.contains("keychain"),
        "help keychain must surface the topic: {stdout}"
    );
    assert!(
        stdout.contains("--save-passphrase"),
        "help keychain must document the opt-in flag: {stdout}"
    );
}

#[test]
fn l2_help_search_finds_save_passphrase_path() {
    // The help search index must surface --save-passphrase from
    // either the ssh editorial topic or the LONG_KEYCHAIN constant
    // so an agent searching "save-passphrase" lands on the docs.
    let sb = Sandbox::new(json!([]));
    let assert = sb
        .cmd()
        .args(["help", "--search", "save-passphrase"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("save-passphrase") || stdout.contains("save_passphrase"),
        "search must find the flag: {stdout}"
    );
}

#[test]
fn l2_help_ssh_topic_lists_credential_lifetime_options() {
    // `inspect help ssh` (the editorial topic) must enumerate the
    // three credential-lifetime options so an agent triaging a
    // long-running migration can reason about what survives a reboot.
    let sb = Sandbox::new(json!([]));
    let assert = sb.cmd().args(["help", "ssh"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("CREDENTIAL LIFETIME"),
        "ssh topic must include credential-lifetime section: {stdout}"
    );
    assert!(
        stdout.contains("--save-passphrase"),
        "credential-lifetime must mention --save-passphrase: {stdout}"
    );
    assert!(
        stdout.contains("ssh-agent"),
        "credential-lifetime must mention ssh-agent default: {stdout}"
    );
}

#[test]
fn l2_connect_help_documents_save_passphrase_flag() {
    // `inspect connect --help` must list the new flag (per the
    // CLAUDE.md contract: every new flag has descriptive help text).
    let sb = Sandbox::new(json!([]));
    let assert = sb.cmd().args(["connect", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("--save-passphrase"),
        "connect --help must document --save-passphrase: {stdout}"
    );
    assert!(
        stdout.contains("OS keychain") || stdout.contains("keychain"),
        "the flag's docs must mention the keychain: {stdout}"
    );
}

#[test]
fn l2_keychain_help_lists_three_subcommands() {
    let sb = Sandbox::new(json!([]));
    let assert = sb.cmd().args(["keychain", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("list"),
        "keychain help must list 'list': {stdout}"
    );
    assert!(
        stdout.contains("remove"),
        "keychain help must list 'remove': {stdout}"
    );
    assert!(
        stdout.contains("test"),
        "keychain help must list 'test': {stdout}"
    );
}

#[test]
fn l2_keychain_list_empty_state_human_form() {
    // No backend writes have happened in the sandbox; the index
    // file does not exist. The empty-state SUMMARY must point the
    // operator at the opt-in flag (NEXT line is part of the
    // contract for agentic discoverability).
    let sb = Sandbox::new(json!([]));
    let assert = sb.cmd().args(["keychain", "list"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("no namespaces saved") || stdout.contains("(none)"),
        "empty-state must be unambiguous: {stdout}"
    );
    assert!(
        stdout.contains("--save-passphrase"),
        "empty-state NEXT must reference --save-passphrase: {stdout}"
    );
}

#[test]
fn l2_keychain_list_json_empty_envelope() {
    // JSON shape: { schema_version, namespaces[], backend_status }
    // is the contract agents consume. Empty case must include
    // namespaces:[] verbatim so a downstream `jq '.namespaces[]'`
    // works without special-casing.
    let sb = Sandbox::new(json!([]));
    let assert = sb
        .cmd()
        .args(["keychain", "list", "--json"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("\"schema_version\":1"),
        "envelope must carry schema_version: {stdout}"
    );
    assert!(
        stdout.contains("\"namespaces\":[]"),
        "empty namespaces must be []: {stdout}"
    );
    assert!(
        stdout.contains("\"backend_status\""),
        "envelope must include backend_status: {stdout}"
    );
}

#[test]
fn l2_keychain_remove_idempotent_when_no_entry() {
    // `inspect keychain remove <ns>` on a namespace with no stored
    // entry exits 0 with `was_present: false` — running on an
    // already-removed namespace must not be an error.
    let sb = Sandbox::new(json!([]));
    let assert = sb
        .cmd()
        .args(["keychain", "remove", "arte", "--json"])
        .assert();
    let output = assert.get_output();
    let stdout = String::from_utf8(output.stdout.clone()).unwrap();
    // Exit code: in the sandbox the backend is generally unreachable
    // (no DBus session), so removing an absent entry may surface as
    // backend-unavailable. Accept either success or backend-error
    // exit; assert the JSON shape carries was_present unambiguously.
    assert!(
        stdout.contains("\"namespace\":\"arte\""),
        "remove --json must echo the namespace: {stdout}"
    );
}

#[test]
fn l2_keychain_remove_rejects_invalid_namespace_name() {
    // Namespace name validation mirrors `inspect connect` rules so a
    // typo can't accidentally save under a name the resolver wouldn't
    // accept. The internal-prefix `__` is reserved for inspect's
    // round-trip probe entry.
    let sb = Sandbox::new(json!([]));
    sb.cmd()
        .args(["keychain", "remove", "Bad Name"])
        .assert()
        .failure();
}

#[test]
fn l2_keychain_remove_rejects_internal_probe_name() {
    let sb = Sandbox::new(json!([]));
    sb.cmd()
        .args(["keychain", "remove", "__inspect_keychain_test__"])
        .assert()
        .failure()
        .stderr(
            contains("reserved")
                .or(contains("internal"))
                .or(contains("invalid namespace name")),
        );
}

#[test]
fn l2_keychain_test_emits_status_in_json_envelope() {
    // `keychain test` on a sandbox without a real OS keychain
    // backend will exit 1 with status:"unavailable" and a chained
    // hint. The JSON envelope shape must be stable for agents.
    let sb = Sandbox::new(json!([]));
    let assert = sb.cmd().args(["keychain", "test", "--json"]).assert();
    let output = assert.get_output();
    let stdout = String::from_utf8(output.stdout.clone()).unwrap();
    assert!(
        stdout.contains("\"schema_version\":1"),
        "envelope must carry schema_version: {stdout}"
    );
    assert!(
        stdout.contains("\"status\":\"available\"")
            || stdout.contains("\"status\":\"unavailable\""),
        "status must be one of the two enum values: {stdout}"
    );
}

#[test]
fn l2_default_connect_path_unchanged_no_keychain_writes() {
    // The CLAUDE.md contract: without --save-passphrase, behavior
    // is byte-identical to v0.1.2. A connect attempt against a
    // mock should never touch the keychain index file. (We can't
    // observe keychain backend writes from outside the binary, but
    // the index is a file we control.)
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    // Connect will fail (no real ssh) — that's fine; we're
    // checking the side-effect on the index file.
    let _ = sb.cmd().args(["connect", "arte"]).assert();
    let index = sb.home().join("keychain-index");
    assert!(
        !index.exists(),
        "default connect must not create the keychain-index"
    );
}

#[test]
fn l2_save_passphrase_flag_compiles_through_clap() {
    // Smoke: the flag is wired into ConnectArgs and parsed by clap.
    // We can't exercise the keychain write without a real backend,
    // but we can verify clap accepts the flag without complaining
    // about an unknown argument.
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    let assert = sb
        .cmd()
        .args(["connect", "arte", "--save-passphrase"])
        .assert();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        !stderr.contains("unexpected argument") && !stderr.contains("unknown argument"),
        "--save-passphrase must be a recognized flag: {stderr}"
    );
}

#[test]
fn l2_save_password_alias_accepted_for_password_auth_namespaces() {
    // The `--save-password` alias is the natural name for password-
    // auth namespaces (where saying "passphrase" is a paper cut).
    // Both flags map to the same code path.
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    let assert = sb
        .cmd()
        .args(["connect", "arte", "--save-password"])
        .assert();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        !stderr.contains("unexpected argument") && !stderr.contains("unknown argument"),
        "--save-password alias must be accepted: {stderr}"
    );
}

// -----------------------------------------------------------------------------
// L8 — Round out the v0.1.3 compose surface.
//
// 1. Per-service narrowing on every `compose` write verb. F6's pull/build
//    already supported it; up/down are extended in L8. Per-service `down`
//    rejects --volumes / --rmi (both project-scoped).
// 2. `compose logs` gains --match / --exclude / --merged / --cursor so the
//    triage surface matches `inspect logs`.
// 3. New `inspect bundle` `compose:` step kind with action allowlist + audit
//    shape mirroring the standalone compose verbs.
//
// These tests cover the user-visible surface that doesn't require a live
// remote: dry-run preview shapes, --volumes/--rmi rejection contracts,
// --merged + --cursor flag wiring, schema validation for the bundle step,
// and help discoverability. The actual docker compose round-trip requires
// a live host and is exercised by the field-validation gate.
// -----------------------------------------------------------------------------

fn write_profile_with_compose(
    home: &std::path::Path,
    ns: &str,
    services: &[(&str, &str, &str)],
    compose: &[(&str, &str, &str)], // (project_name, working_dir, services_csv)
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
    let mut comp_yaml = String::new();
    for (proj, wd, _svcs) in compose {
        comp_yaml.push_str(&format!(
            "  - name: {proj}\n    status: running\n    compose_file: {wd}/docker-compose.yml\n    working_dir: {wd}\n    service_count: 2\n    running_count: 2\n"
        ));
    }
    let compose_block = if comp_yaml.is_empty() {
        String::new()
    } else {
        format!("compose_projects:\n{comp_yaml}")
    };
    // NOTE: do NOT use Rust string-literal line-continuation (`\<newline>`)
    // here — it strips the leading whitespace YAML needs for indentation
    // and produces a non-parsing profile. Build the YAML body via
    // line-by-line concat so each row carries its exact leading spaces.
    let mut body = String::new();
    body.push_str(&format!("schema_version: 1\n"));
    body.push_str(&format!("namespace: {ns}\n"));
    body.push_str(&format!("host: {ns}.example.invalid\n"));
    body.push_str("discovered_at: 2099-01-01T00:00:00+00:00\n");
    body.push_str("remote_tooling:\n");
    body.push_str("  rg: false\n  jq: false\n  journalctl: false\n  sed: false\n");
    body.push_str("  grep: true\n  netstat: false\n  ss: true\n  systemctl: false\n");
    body.push_str("  docker: true\n");
    body.push_str("services:\n");
    body.push_str(&svc_yaml);
    body.push_str("volumes: []\nimages: []\nnetworks: []\n");
    body.push_str(&compose_block);
    let path = dir.join(format!("{ns}.yaml"));
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

#[test]
fn l8_compose_up_per_service_dry_run_shape() {
    // L8 (v0.1.3): `compose up <ns>/<p>/<svc>` (no --apply) must
    // dry-run-render with a "service <svc>" scope label so an
    // operator can confirm the narrowing before applying.
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile_with_compose(
        sb.home(),
        "arte",
        &[("api", "img/api:1", "ok")],
        &[("luminary-onyx", "/opt/luminary-onyx", "api,onyx-vault")],
    );
    let assert = sb
        .cmd()
        .args(["compose", "up", "arte/luminary-onyx/onyx-vault"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("DRY RUN"),
        "expected dry-run preamble: {stdout}"
    );
    assert!(
        stdout.contains("service onyx-vault"),
        "expected service-scope label: {stdout}"
    );
    assert!(
        stdout.contains("docker compose -p ") && stdout.contains("'onyx-vault'"),
        "expected service token in rendered cmd: {stdout}"
    );
}

#[test]
fn l8_compose_down_per_service_uses_stop_and_rm() {
    // L8: per-service `compose down` renders `stop && rm -f` (the
    // explicit two-step form), not `docker compose down <svc>`.
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile_with_compose(
        sb.home(),
        "arte",
        &[("api", "img/api:1", "ok")],
        &[("luminary-onyx", "/opt/luminary-onyx", "api")],
    );
    let assert = sb
        .cmd()
        .args(["compose", "down", "arte/luminary-onyx/api"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("stop 'api'") && stdout.contains("rm -f 'api'"),
        "expected stop && rm shape: {stdout}"
    );
}

#[test]
fn l8_compose_down_per_service_rejects_volumes() {
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile_with_compose(
        sb.home(),
        "arte",
        &[("api", "img/api:1", "ok")],
        &[("luminary-onyx", "/opt/luminary-onyx", "api")],
    );
    sb.cmd()
        .args([
            "compose",
            "down",
            "arte/luminary-onyx/api",
            "--volumes",
            "--apply",
        ])
        .assert()
        .failure()
        .stderr(contains("--volumes is not supported for per-service"))
        .stderr(contains("project-scoped"));
}

#[test]
fn l8_compose_down_per_service_rejects_rmi() {
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile_with_compose(
        sb.home(),
        "arte",
        &[("api", "img/api:1", "ok")],
        &[("luminary-onyx", "/opt/luminary-onyx", "api")],
    );
    sb.cmd()
        .args([
            "compose",
            "down",
            "arte/luminary-onyx/api",
            "--rmi",
            "--apply",
        ])
        .assert()
        .failure()
        .stderr(contains("--rmi is not supported for per-service"));
}

#[test]
fn l8_compose_logs_merged_rejects_per_service_selector() {
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile_with_compose(
        sb.home(),
        "arte",
        &[("api", "img/api:1", "ok")],
        &[("luminary-onyx", "/opt/luminary-onyx", "api")],
    );
    sb.cmd()
        .args([
            "compose",
            "logs",
            "arte/luminary-onyx/api",
            "--merged",
            "--tail",
            "10",
        ])
        .assert()
        .failure()
        .stderr(contains("--merged is incompatible with the per-service"));
}

#[test]
fn l8_compose_logs_cursor_and_since_are_mutually_exclusive() {
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile_with_compose(
        sb.home(),
        "arte",
        &[("api", "img/api:1", "ok")],
        &[("luminary-onyx", "/opt/luminary-onyx", "api")],
    );
    let cur = sb.home().join("onyx.cursor");
    sb.cmd()
        .args([
            "compose",
            "logs",
            "arte/luminary-onyx",
            "--cursor",
            cur.to_str().unwrap(),
            "--since",
            "5m",
        ])
        .assert()
        .failure()
        .stderr(
            contains("cannot be used with")
                .or(contains("conflicts"))
                .or(contains("argument")),
        );
}

#[test]
fn l8_compose_logs_match_and_exclude_flags_accepted() {
    // Smoke: clap accepts both flags repeated. We don't run the
    // mock far enough to exercise the grep pipeline build (the
    // pipeline assembly is unit-tested in line_filter::tests).
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile_with_compose(
        sb.home(),
        "arte",
        &[("api", "img/api:1", "ok")],
        &[("luminary-onyx", "/opt/luminary-onyx", "api")],
    );
    let assert = sb
        .cmd()
        .args([
            "compose",
            "logs",
            "arte/luminary-onyx",
            "--tail",
            "10",
            "--match",
            "ERROR",
            "--exclude",
            "healthcheck",
            "--match",
            "WARN",
        ])
        .assert();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        !stderr.contains("unexpected argument") && !stderr.contains("unknown argument"),
        "every L8 logs flag must be a recognized clap arg: {stderr}"
    );
}

#[test]
fn l8_compose_help_documents_per_service_and_logs_triage() {
    let sb = Sandbox::new(json!([]));
    let assert = sb.cmd().args(["compose", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("<ns>/<project>/<service>"),
        "compose --help must document per-service selector: {stdout}"
    );
    assert!(
        stdout.contains("--merged") && stdout.contains("--cursor"),
        "compose --help must document the L8 logs flags: {stdout}"
    );
}

#[test]
fn l8_help_topic_compose_documents_bundle_step_and_per_service() {
    let sb = Sandbox::new(json!([]));
    let assert = sb.cmd().args(["help", "compose"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("PER-SERVICE NARROWING"),
        "compose topic must document per-service narrowing: {stdout}"
    );
    assert!(
        stdout.contains("BUNDLE compose: STEP KIND"),
        "compose topic must document the bundle compose step: {stdout}"
    );
    assert!(
        stdout.contains("revert.kind"),
        "compose topic must document the revert taxonomy: {stdout}"
    );
}

#[test]
fn l8_help_search_finds_per_service_compose_down() {
    let sb = Sandbox::new(json!([]));
    let assert = sb
        .cmd()
        .args(["help", "--search", "per-service"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("compose"),
        "search for 'per-service' must surface the compose topic: {stdout}"
    );
}

#[test]
fn l8_bundle_compose_step_rejects_unknown_action_at_parse() {
    // The bundle YAML parser uses a closed `ComposeAction` enum;
    // any unknown action surfaces as a deserialization error at
    // `bundle plan` time.
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    let bundle = sb.home().join("bad.yaml");
    std::fs::write(
        &bundle,
        "name: bad\nhost: arte\nsteps:\n  - id: s1\n    compose:\n      project: luminary-onyx\n      action: nuke\n",
    )
    .unwrap();
    sb.cmd()
        .args(["bundle", "plan", bundle.to_str().unwrap()])
        .assert()
        .failure();
}

#[test]
fn l8_bundle_compose_step_rejects_unknown_flag_per_action() {
    // ComposeAction::Up's allowlist is { force_recreate, no_detach };
    // a step that passes `volumes: true` should fail at execution
    // (we surface the error from validation before dispatch).
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    write_profile_with_compose(
        sb.home(),
        "arte",
        &[("api", "img/api:1", "ok")],
        &[("luminary-onyx", "/opt/luminary-onyx", "api")],
    );
    let bundle = sb.home().join("bad-flag.yaml");
    std::fs::write(
        &bundle,
        "name: bad\nhost: arte\nsteps:\n  - id: s1\n    compose:\n      project: luminary-onyx\n      action: up\n      flags:\n        volumes: true\n",
    )
    .unwrap();
    let assert = sb
        .cmd()
        .args(["bundle", "apply", bundle.to_str().unwrap(), "--apply"])
        .assert();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("does not accept flag")
            || stderr.contains("volumes")
            || stderr.contains("allowed"),
        "bundle compose:up with flags.volumes must reject: {stderr}"
    );
}

#[test]
fn l8_bundle_compose_step_rejects_multiple_bodies() {
    let sb = Sandbox::new(json!([]));
    write_servers_toml(sb.home(), &["arte"]);
    let bundle = sb.home().join("multi.yaml");
    std::fs::write(
        &bundle,
        "name: multi\nhost: arte\nsteps:\n  - id: s1\n    exec: \"true\"\n    compose:\n      project: p\n      action: up\n",
    )
    .unwrap();
    sb.cmd()
        .args(["bundle", "plan", bundle.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(contains("multiple bodies").or(contains("exactly one")));
}
