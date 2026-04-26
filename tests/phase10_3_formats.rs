//! Phase 10.3 — universal output format integration tests.
//!
//! These cover the user-facing behavior of `--json` / `--jsonl` / `--csv`
//! / `--tsv` / `--yaml` / `--table` / `--md` / `--format '<tpl>'` /
//! `--raw` and the bible's mutual-exclusivity error message.

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
    fn new(mock_responses: Value) -> Self {
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
    services: &[(&str, &str, &str)],
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
            "  - name: {name}\n    container_id: cid-{name}\n    image: {image}\n    ports: []\n    mounts: []\n    health_status: {hs}\n    log_readable_directly: false\n    kind: container\n    depends_on: []\n"
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

fn ps_sandbox() -> Sandbox {
    let mock = json!([
        { "match": "docker ps", "stdout": "{\"Names\":\"pulse\",\"Image\":\"luminary/pulse:1\",\"Status\":\"Up 3h\"}\n{\"Names\":\"atlas\",\"Image\":\"luminary/atlas:1\",\"Status\":\"Up 1h\"}\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("pulse", "luminary/pulse:1", "ok"), ("atlas", "luminary/atlas:1", "ok")]);
    sb
}

// ---------------------------------------------------------------------------
// Mutual exclusivity (bible-mandated wording).
// ---------------------------------------------------------------------------

#[test]
fn json_and_csv_are_mutually_exclusive() {
    let sb = ps_sandbox();
    let out = sb.cmd().args(["ps", "arte", "--json", "--csv"]).output().unwrap();
    assert!(!out.status.success(), "expected non-zero exit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--json"), "stderr missing --json: {stderr}");
    assert!(stderr.contains("--csv"), "stderr missing --csv: {stderr}");
    assert!(
        stderr.contains("mutually exclusive. Pick one output format."),
        "exact bible wording missing: {stderr}"
    );
}

#[test]
fn yaml_and_format_are_mutually_exclusive() {
    let sb = ps_sandbox();
    let out = sb
        .cmd()
        .args(["ps", "arte", "--yaml", "--format", "{{.service}}"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("mutually exclusive"), "stderr: {stderr}");
}

// ---------------------------------------------------------------------------
// --jsonl is an alias for --json.
// ---------------------------------------------------------------------------

#[test]
fn jsonl_emits_same_shape_as_json() {
    let sb = ps_sandbox();
    let a = sb.cmd().args(["ps", "arte", "--json"]).output().unwrap();
    let b = sb.cmd().args(["ps", "arte", "--jsonl"]).output().unwrap();
    assert!(a.status.success());
    assert!(b.status.success());
    assert_eq!(a.stdout, b.stdout, "--jsonl should be identical to --json");
}

// ---------------------------------------------------------------------------
// CSV / TSV.
// ---------------------------------------------------------------------------

#[test]
fn csv_emits_header_and_rows_no_envelope() {
    let sb = ps_sandbox();
    let out = sb.cmd().args(["ps", "arte", "--csv"]).output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(!stdout.contains("SUMMARY:"), "csv must suppress SUMMARY: {stdout}");
    assert!(!stdout.contains("NEXT:"), "csv must suppress NEXT: {stdout}");
    let header = lines.first().expect("csv must have a header");
    assert!(header.starts_with("_source,_medium,server,service"), "header order: {header}");
    let body = lines[1..].join("\n");
    assert!(body.contains("pulse"), "csv missing pulse: {stdout}");
    assert!(body.contains("atlas"), "csv missing atlas: {stdout}");
}

#[test]
fn csv_quotes_fields_with_commas() {
    // The Status field can include commas in the wild ("Up 3 hours, healthy").
    let mock = json!([
        { "match": "docker ps", "stdout": "{\"Names\":\"pulse\",\"Image\":\"a\",\"Status\":\"Up 3h, healthy\"}\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("pulse", "a", "ok")]);
    let out = sb.cmd().args(["ps", "arte", "--csv"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("\"Up 3h, healthy\""), "comma not quoted: {stdout}");
}

#[test]
fn tsv_uses_tabs_no_quoting() {
    let sb = ps_sandbox();
    let out = sb.cmd().args(["ps", "arte", "--tsv"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let header = stdout.lines().next().unwrap();
    assert!(header.contains('\t'), "tsv header has no tabs: {header:?}");
    assert!(!header.contains(','), "tsv must not contain commas in header: {header:?}");
}

// ---------------------------------------------------------------------------
// YAML.
// ---------------------------------------------------------------------------

#[test]
fn yaml_emits_summary_comment_and_documents() {
    let sb = ps_sandbox();
    let out = sb.cmd().args(["ps", "arte", "--yaml"]).output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("# summary:"), "yaml must lead with comment: {stdout}");
    assert!(stdout.contains("pulse"), "yaml missing pulse: {stdout}");
    // serde_yaml produces a list when the input is an array.
    assert!(stdout.contains("- "), "yaml list marker missing: {stdout}");
}

// ---------------------------------------------------------------------------
// --md (Markdown table).
// ---------------------------------------------------------------------------

#[test]
fn md_emits_pipe_table() {
    let sb = ps_sandbox();
    let out = sb.cmd().args(["ps", "arte", "--md"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("SUMMARY:"), "md must keep SUMMARY: {stdout}");
    assert!(stdout.contains("| _source"), "md table header: {stdout}");
    assert!(stdout.contains("| --- |"), "md separator row: {stdout}");
    assert!(stdout.contains("pulse"), "md missing pulse: {stdout}");
}

// ---------------------------------------------------------------------------
// --table (plain ASCII).
// ---------------------------------------------------------------------------

#[test]
fn table_is_plain_ascii_with_envelope() {
    let sb = ps_sandbox();
    let out = sb.cmd().args(["ps", "arte", "--table"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("SUMMARY:"));
    assert!(stdout.contains("pulse"));
    // No box-drawing or ANSI escapes.
    assert!(!stdout.contains("\u{2500}"), "must not contain box-drawing: {stdout}");
    assert!(!stdout.contains('\x1b'), "must not contain ANSI escapes: {stdout}");
}

// ---------------------------------------------------------------------------
// --format '<template>'.
// ---------------------------------------------------------------------------

#[test]
fn format_template_renders_per_record() {
    let sb = ps_sandbox();
    let out = sb
        .cmd()
        .args(["ps", "arte", "--format", "{{.service}}"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines, vec!["pulse", "atlas"]);
}

#[test]
fn format_template_pipes_work() {
    let sb = ps_sandbox();
    let out = sb
        .cmd()
        .args(["ps", "arte", "--format", "{{.service | upper}}"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines, vec!["PULSE", "ATLAS"]);
}

#[test]
fn format_template_conditional() {
    let sb = ps_sandbox();
    // The status column for both rows starts with "Up " — pick a less
    // ambiguous predicate using the service name.
    let out = sb
        .cmd()
        .args([
            "ps",
            "arte",
            "--format",
            "{{if eq .service \"pulse\"}}HOT: {{.service}}{{end}}",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("HOT: pulse"), "conditional miss: {stdout}");
    // The atlas row evaluates to empty.
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(lines.iter().any(|l| l.is_empty()), "atlas should produce empty line: {stdout}");
}

// ---------------------------------------------------------------------------
// --raw.
// ---------------------------------------------------------------------------

#[test]
fn raw_strips_envelope_and_emits_scalars() {
    let sb = ps_sandbox();
    let out = sb.cmd().args(["ps", "arte", "--raw"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("SUMMARY:"), "raw must suppress SUMMARY: {stdout}");
    assert!(!stdout.contains("NEXT:"), "raw must suppress NEXT: {stdout}");
    // Raw should pick the most meaningful scalar — service name takes
    // priority once present, so we should see both names.
    assert!(stdout.contains("pulse"), "raw missing pulse: {stdout}");
    assert!(stdout.contains("atlas"), "raw missing atlas: {stdout}");
}

// ---------------------------------------------------------------------------
// Backward compatibility: `--json` keeps producing one envelope per line.
// ---------------------------------------------------------------------------

#[test]
fn json_remains_line_delimited_envelopes() {
    let sb = ps_sandbox();
    let out = sb.cmd().args(["ps", "arte", "--json"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<Value> = stdout
        .lines()
        .filter(|l| l.trim_start().starts_with('{'))
        .map(|l| serde_json::from_str(l).expect("each line is valid JSON"))
        .collect();
    assert_eq!(lines.len(), 2, "expected one envelope per row: {stdout}");
    for v in &lines {
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["_medium"], "state");
        assert_eq!(v["server"], "arte");
        assert!(v.get("service").is_some());
    }
}

// ---------------------------------------------------------------------------
// `--no-color` flag is accepted by every command (smoke test).
// ---------------------------------------------------------------------------

#[test]
fn no_color_flag_is_accepted_globally() {
    let sb = ps_sandbox();
    let out = sb.cmd().args(["ps", "arte", "--no-color"]).output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("pulse"));
}

// ---------------------------------------------------------------------------
// OutputDoc-style commands also honor format dispatch (status verb).
// ---------------------------------------------------------------------------

#[test]
fn status_csv_emits_services_table() {
    let mock = json!([
        { "match": "docker ps", "stdout": "pulse\natlas\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("pulse", "p:1", "ok"), ("atlas", "a:1", "unhealthy")]);
    let out = sb.cmd().args(["status", "arte/*", "--csv"]).output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("SUMMARY:"), "csv must suppress envelope: {stdout}");
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    let header = lines.first().expect("csv must have a header");
    assert!(header.contains("name") || header.contains("service"), "header: {header}");
    let body = lines[1..].join("\n");
    assert!(body.contains("pulse"));
    assert!(body.contains("atlas"));
}

#[test]
fn status_yaml_keeps_summary_as_comment() {
    let mock = json!([
        { "match": "docker ps", "stdout": "pulse\n", "exit": 0 }
    ]);
    let sb = Sandbox::new(mock);
    write_servers_toml(sb.home(), &["arte"]);
    write_profile(sb.home(), "arte", &[("pulse", "p:1", "ok")]);
    let out = sb.cmd().args(["status", "arte/*", "--yaml"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("# summary:"), "yaml lead-in: {stdout}");
    assert!(stdout.contains("pulse"));
}

// ---------------------------------------------------------------------------
// Default human format is unchanged (regression guard).
// ---------------------------------------------------------------------------

#[test]
fn default_human_format_unchanged_for_ps() {
    let sb = ps_sandbox();
    sb.cmd()
        .args(["ps", "arte"])
        .assert()
        .success()
        .stdout(contains("pulse"))
        .stdout(contains("atlas"))
        .stdout(contains("2 container(s)"));
}
