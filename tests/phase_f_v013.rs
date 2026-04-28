//! Phase F (v0.1.3) acceptance tests — field-feedback regressions and
//! ergonomic gaps fixed in the v0.1.3 patch backlog. Each test is named
//! `f<N>_<short>` so the locked backlog item is obvious from the test
//! name alone.
//!
//! All tests run against the in-process mock medium (no real SSH).

use std::sync::{Mutex, MutexGuard, OnceLock};

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
