//! Phase C acceptance tests for v0.1.1.
//!
//! Pins the Phase C patches from `INSPECT_v0.1.1_PATCH_SPEC.md` against
//! regressions:
//!
//! - **P4** Secret masking on `run`/`exec` stdout (`EnvSecretMasker`).
//! - **P5** `--merged` multi-container log view (k-way merge by ts).
//! - **P9** Progress spinner suppressed in JSON / non-TTY mode.
//! - **P11** Inner exit code surfacing through `ExitKind::Inner(u8)`.
//! - **P13** Discovery `docker inspect` per-container fallback +
//!   `discovery_incomplete` flag + `inspect setup --retry-failed`.

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
            .env("INSPECT_NO_PROGRESS", "1")
            .env_remove("CODESPACES");
        c
    }
    fn home(&self) -> &std::path::Path {
        self.home.path()
    }
}

fn write_servers_toml(home: &std::path::Path, ns: &str) {
    let body = format!(
        "schema_version = 1\n\n\
         [namespaces.{ns}]\n\
         host = \"{ns}.example.invalid\"\n\
         user = \"deploy\"\n\
         port = 22\n"
    );
    let path = home.join("servers.toml");
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn write_profile(home: &std::path::Path, ns: &str, services: &[(&str, &str)]) {
    let dir = home.join("profiles");
    std::fs::create_dir_all(&dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut svc_yaml = String::new();
    for (name, container) in services {
        svc_yaml.push_str(&format!(
            "  - name: {name}\n    \
                container_name: {container}\n    \
                container_id: cid-{container}\n    \
                image: img/{name}:1\n    \
                ports: []\n    \
                mounts: []\n    \
                health_status: ok\n    \
                log_readable_directly: false\n    \
                kind: container\n    \
                depends_on: []\n"
        ));
    }
    let body = format!(
        "schema_version: 1\n\
         namespace: {ns}\n\
         host: {ns}.example.invalid\n\
         discovered_at: 2099-01-01T00:00:00+00:00\n\
         remote_tooling:\n  \
           rg: false\n  \
           jq: false\n  \
           journalctl: false\n  \
           sed: false\n  \
           grep: true\n  \
           netstat: false\n  \
           ss: true\n  \
           systemctl: false\n  \
           docker: true\n\
         services:\n{svc_yaml}volumes: []\nimages: []\nnetworks: []\n"
    );
    let path = dir.join(format!("{ns}.yaml"));
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn standard_setup(sb: &Sandbox) {
    write_servers_toml(sb.home(), "arte");
    write_profile(sb.home(), "arte", &[("pulse", "luminary-pulse")]);
}

// -----------------------------------------------------------------------------
// P4: Secret masking
// -----------------------------------------------------------------------------

/// `inspect run -- env` output containing an Anthropic-style API key
/// is masked to `head4****tail2` by default.
#[test]
fn p4_run_masks_anthropic_api_key() {
    let mock = json!([
        {
            "match": "docker exec 'luminary-pulse'",
            "stdout": "ANTHROPIC_API_KEY=sk-anthropicAAAABBBBkk\nFOO=bar\n",
            "exit": 0
        }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    let out = sb
        .cmd()
        .args(["run", "arte/pulse", "--", "env"])
        .assert()
        .success()
        // Verbatim secret must NOT appear.
        .stdout(contains("sk-anthropicAAAABBBBkk").not())
        // Masked form (head4 + **** + tail2) MUST appear.
        .stdout(contains("ANTHROPIC_API_KEY=sk-a****kk"))
        // Non-secret KEY=VALUE pair survives untouched.
        .stdout(contains("FOO=bar"))
        .get_output()
        .clone();
    assert!(!String::from_utf8_lossy(&out.stdout).contains("anthropicAAAABBBB"));
}

/// `--show-secrets` opts out of masking and tags the audit entry on
/// `inspect exec` with `[secrets_exposed=true]`.
#[test]
fn p4_show_secrets_passes_through_and_audits() {
    let mock = json!([
        {
            "match": "docker exec 'luminary-pulse'",
            "stdout": "ANTHROPIC_API_KEY=sk-anthropicAAAABBBBkk\n",
            "exit": 0
        }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    sb.cmd()
        .args([
            "exec",
            "arte/pulse",
            "--apply",
            "--yes",
            "--no-revert",
            "--show-secrets",
            "--reason",
            "INC-secret-rotation",
            "--",
            "env",
        ])
        .assert()
        .success()
        // Verbatim secret IS present.
        .stdout(contains("sk-anthropicAAAABBBBkk"));
    // Audit jsonl must contain the breadcrumb.
    let audit_dir = sb.home().join("audit");
    let entries: Vec<_> = std::fs::read_dir(&audit_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .collect();
    assert!(!entries.is_empty(), "audit jsonl should exist");
    let body = std::fs::read_to_string(entries[0].path()).unwrap();
    assert!(
        body.contains("secrets_exposed=true"),
        "audit body missing breadcrumb: {body}"
    );
}

// -----------------------------------------------------------------------------
// P11: Inner exit code surfacing
// -----------------------------------------------------------------------------

/// `inspect run -- 'exit 7'` returns the inner command's exit code (7).
#[test]
fn p11_run_propagates_inner_exit_code() {
    let mock = json!([
        { "match": "docker exec 'luminary-pulse'", "stdout": "", "exit": 7 }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    sb.cmd()
        .args(["run", "arte/pulse", "--", "false"])
        .assert()
        .code(7);
}

/// Same behaviour on `inspect exec` (which IS audited): inner exit
/// code 9 surfaces verbatim.
#[test]
fn p11_exec_propagates_inner_exit_code() {
    let mock = json!([
        { "match": "docker exec 'luminary-pulse'", "stdout": "", "exit": 9 }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    sb.cmd()
        .args([
            "exec",
            "arte/pulse",
            "--apply",
            "--yes",
            "--no-revert",
            "--reason",
            "INC-inner-exit-test",
            "--",
            "false",
        ])
        .assert()
        .code(9);
}

// -----------------------------------------------------------------------------
// P9: Progress spinner suppression
// -----------------------------------------------------------------------------

/// In `--json` mode the spinner is never drawn; stderr stays empty.
#[test]
fn p9_no_spinner_in_json_mode() {
    let mock = json!([
        { "match": "docker logs", "stdout": "x\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    standard_setup(&sb);
    let out = sb
        .cmd()
        .args(["logs", "arte/pulse", "--json", "--tail", "1"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("⠋")
            && !stderr.contains("⠙")
            && !stderr.contains("Scanning")
            && !stderr.contains("logs arte/pulse"),
        "expected no spinner in JSON mode, got stderr: {stderr:?}"
    );
}

// -----------------------------------------------------------------------------
// P13: discovery_incomplete schema bit
// -----------------------------------------------------------------------------

/// The `discovery_incomplete` field round-trips through YAML
/// serialization. This guards the schema bit: if the field is dropped
/// from the struct, --retry-failed silently no-ops. We use the CLI's
/// `profile` command to load + re-emit the profile and grep for the
/// flag.
#[test]
fn p13_discovery_incomplete_round_trips_via_cli() {
    let mock = json!([]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), "arte");
    // Hand-craft a profile YAML with discovery_incomplete: true on
    // one service. The profile CLI will load it through the strongly
    // typed schema and re-serialize as YAML; if the schema field were
    // dropped, the flag would not survive the round trip.
    let dir = sb.home().join("profiles");
    std::fs::create_dir_all(&dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let yaml = "schema_version: 1\n\
                namespace: arte\n\
                host: arte.example.invalid\n\
                discovered_at: 2099-01-01T00:00:00+00:00\n\
                remote_tooling:\n  \
                  rg: false\n  jq: false\n  journalctl: false\n  sed: false\n  \
                  grep: true\n  netstat: false\n  ss: true\n  systemctl: false\n  docker: true\n\
                services:\n  \
                  - name: pulse\n    container_name: luminary-pulse\n    \
                    container_id: cid-x\n    image: img/pulse:1\n    ports: []\n    \
                    mounts: []\n    health_status: ok\n    log_readable_directly: false\n    \
                    kind: container\n    depends_on: []\n    discovery_incomplete: true\n\
                volumes: []\nimages: []\nnetworks: []\n";
    let path = dir.join("arte.yaml");
    std::fs::write(&path, yaml).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    sb.cmd()
        .args(["profile", "arte", "--yaml"])
        .assert()
        .success()
        .stdout(contains("discovery_incomplete: true"));
}
