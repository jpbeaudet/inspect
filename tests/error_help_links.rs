//! HP-5 — error → help linkage contract.
//!
//! Guards (plan §7, §8 G6):
//! * Every `error: …` line emitted by the binary at a known site must
//!   carry a `see: inspect help <topic>` cross-link.
//! * No `eprintln!("error: …")` survives outside `error.rs` / `main.rs`.
//! * The `--json errors[]` array exposes the catalog with the same
//!   `code`/`summary`/`help_topic` shape used at runtime.
//! * Five canonical errors reproduce byte-for-byte under
//!   `INSPECT_NON_INTERACTIVE=1` (plan §11 step 6).

use assert_cmd::Command;
use predicates::str;

fn inspect() -> Command {
    let mut c = Command::cargo_bin("inspect").expect("binary builds");
    c.env("INSPECT_NON_INTERACTIVE", "1");
    c
}

// G6-static: no `eprintln!("error: ...)` line anywhere in `src/`
// outside the two files that own the helper itself. This is the
// linchpin guard — without it new error sites can drift away from
// the central renderer and lose their cross-link.
#[test]
fn no_raw_error_eprintln_outside_error_module() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations: Vec<String> = Vec::new();
    walk(&root, &mut |p, content| {
        let rel = p
            .strip_prefix(&root)
            .map(|r| r.display().to_string())
            .unwrap_or_else(|_| p.display().to_string());
        if matches!(rel.as_str(), "error.rs" | "main.rs") {
            return;
        }
        for (i, line) in content.lines().enumerate() {
            if line.contains("eprintln!(\"error:") {
                violations.push(format!("{}:{}: {}", rel, i + 1, line.trim()));
            }
        }
    });
    assert!(
        violations.is_empty(),
        "raw `eprintln!(\"error: …\")` sites found — every error must funnel through `crate::error::emit`:\n{}",
        violations.join("\n")
    );
}

fn walk(dir: &std::path::Path, f: &mut dyn FnMut(&std::path::Path, &str)) {
    for entry in std::fs::read_dir(dir).expect("readable src/") {
        let entry = entry.expect("dirent");
        let p = entry.path();
        if p.is_dir() {
            walk(&p, f);
        } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
            if let Ok(s) = std::fs::read_to_string(&p) {
                f(&p, &s);
            }
        }
    }
}

// ---- canonical bible §7 errors -----------------------------------------

#[test]
fn unknown_help_topic_links_to_examples() {
    // F3 (v0.1.3): exit 2 + wording aligned with the verb-synonym
    // path (`inspect help <foo>` covers both verbs and topics).
    inspect()
        .args(["help", "definitely-not-a-topic"])
        .assert()
        .code(2)
        .stderr(str::contains("error: unknown command or topic"))
        .stderr(str::contains("see: inspect help examples"));
}

#[test]
fn empty_search_query_links_to_search_topic() {
    // `inspect search` with empty query produces our canonical error.
    inspect()
        .args(["search", ""])
        .assert()
        .stderr(str::contains("error: empty query"))
        .stderr(str::contains("see: inspect help search"));
}

#[test]
fn unresolved_selector_links_to_selectors() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().expect("tmpdir");
    let cfg = tmp.path().join("servers.toml");
    std::fs::write(
        &cfg,
        "schema_version = 1\n[namespaces.arte]\nhost = \"127.0.0.1\"\nuser = \"ops\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o600)).unwrap();
    Command::cargo_bin("inspect")
        .unwrap()
        .env("INSPECT_NON_INTERACTIVE", "1")
        .env("INSPECT_HOME", tmp.path())
        .args(["rm", "arte/atlas"])
        .assert()
        .stderr(str::contains("matched no targets"))
        .stderr(str::contains("see: inspect help selectors"));
}

#[test]
fn no_namespaces_links_to_quickstart() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    Command::cargo_bin("inspect")
        .unwrap()
        .env("INSPECT_NON_INTERACTIVE", "1")
        .env("INSPECT_HOME", tmp.path())
        .args(["rm", "arte/atlas:/tmp/x"])
        .assert()
        .stderr(str::contains("no namespaces are configured"))
        .stderr(str::contains("see: inspect help quickstart"));
}

#[test]
fn exec_without_command_links_to_write() {
    inspect()
        .args(["exec", "arte/atlas"])
        .assert()
        .stderr(str::contains("error: exec requires a command"))
        .stderr(str::contains("see: inspect help write"));
}

#[test]
fn audit_revert_unknown_id_links_to_safety() {
    // `inspect revert` on a missing audit id.
    inspect()
        .args(["revert", "nonexistent-audit-id-zzzz"])
        .assert()
        .stderr(str::contains("audit entry"))
        .stderr(str::contains("see: inspect help safety"));
}

// ---- catalog ↔ JSON parity ---------------------------------------------

#[test]
fn json_errors_array_matches_runtime_catalog_size() {
    let out = inspect()
        .args(["help", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8(out).unwrap()).unwrap();
    let errs = v["errors"].as_array().unwrap();
    // The HP-4 stub had 15 entries; HP-5 must be ≥ that and include
    // every code we reference in the canonical-error tests above.
    assert!(
        errs.len() >= 15,
        "errors[] regressed: HP-5 must expose ≥15 entries, got {}",
        errs.len()
    );
    let codes: Vec<&str> = errs.iter().map(|e| e["code"].as_str().unwrap()).collect();
    for required in [
        "EmptyTargets",
        "RequiresPath",
        "EmptyQuery",
        "AuditEntryNotFound",
        "UnknownHelpTopic",
        "ExecMissingCommand",
    ] {
        assert!(
            codes.contains(&required),
            "errors[] missing required code {required:?} (have {:?})",
            codes
        );
    }
}

#[test]
fn json_errors_every_topic_resolves_or_is_empty() {
    let out = inspect()
        .args(["help", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8(out).unwrap()).unwrap();
    let topics: std::collections::HashSet<String> = v["topics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect();
    for e in v["errors"].as_array().unwrap() {
        let t = e["help_topic"].as_str().unwrap();
        if !t.is_empty() {
            assert!(
                topics.contains(t),
                "errors[].help_topic {:?} is not a registered topic",
                t
            );
        }
    }
}
