//! F19 (C2/4): integration tests for the `--select` /
//! `--select-raw` / `--select-slurp` flags wired onto every JSON-
//! emitting verb.
//!
//! C1 (`tests/jaq_query_v013.rs`) already covers the `query::*`
//! engine end-to-end through the standalone `inspect query` verb.
//! These tests focus on the *plumbing* C2 added on top: that the
//! flags are accepted on every JSON-emitting verb, route through
//! the right chokepoints (`OutputDoc::print_json`, `JsonOut::write`,
//! `Renderer::dispatch`, the bespoke `fleet`/`help`/`setup`
//! emitters), and propagate the same exit-code contract every-
//! where.
//!
//! Tests that need a clean `~/.inspect/` use `tempfile::tempdir()`
//! together with `INSPECT_HOME=…` so they do not depend on the
//! developer's real profile or audit log. Tests that need network
//! access against a real SSH host are deliberately not in this
//! file — the flag contract is verifiable against locally-emitted
//! envelopes (audit ls, history list, help all) without burning a
//! remote round-trip.

use assert_cmd::Command;
use predicates::str::contains;
use tempfile::TempDir;

fn cmd() -> Command {
    let mut c = Command::cargo_bin("inspect").expect("binary builds");
    c.env("INSPECT_NON_INTERACTIVE", "1");
    c
}

/// Sandbox INSPECT_HOME inside a tempdir so tests don't touch the
/// developer's real audit log / config / history.
fn sandbox() -> TempDir {
    tempfile::tempdir().expect("tmpdir")
}

// =========================================================================
// OutputDoc envelope chokepoint — `audit ls --json` is the canonical
// `print_json_value` consumer (envelope shape `{schema_version, summary,
// data:{entries:[…]}, next, meta}`).
// =========================================================================

#[test]
fn f19_select_envelope_summary_field() {
    let home = sandbox();
    cmd()
        .env("INSPECT_HOME", home.path())
        .args(["audit", "ls", "--json", "--select", ".summary"])
        .assert()
        .success()
        .stdout(contains("audit entry/entries"));
}

#[test]
fn f19_select_envelope_raw_unquotes_string() {
    let home = sandbox();
    let out = cmd()
        .env("INSPECT_HOME", home.path())
        .args([
            "audit",
            "ls",
            "--json",
            "--select",
            ".meta.order",
            "--select-raw",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(out).unwrap();
    // `.meta.order` is the string "newest_first"; `--select-raw`
    // emits it without surrounding quotes.
    assert_eq!(s.trim(), "newest_first");
}

#[test]
fn f19_select_envelope_array_length() {
    let home = sandbox();
    cmd()
        .env("INSPECT_HOME", home.path())
        .args([
            "audit",
            "ls",
            "--json",
            "--select",
            ".data.entries | length",
        ])
        .assert()
        .success()
        .stdout("0\n");
}

#[test]
fn f19_select_envelope_data_path() {
    // `.data.entries[0].id // empty` yields nothing for an empty
    // audit log, which exercises the zero-results → exit 1
    // contract on the envelope chokepoint.
    let home = sandbox();
    cmd()
        .env("INSPECT_HOME", home.path())
        .args([
            "audit",
            "ls",
            "--json",
            "--select",
            ".data.entries[0].id // empty",
        ])
        .assert()
        .code(1)
        .stdout("");
}

// =========================================================================
// Bespoke emitter — `help all --json` was rewritten in C2 to route
// through `print_json_value` so `--select` works on the help registry.
// =========================================================================

#[test]
fn f19_select_help_topic_ids() {
    cmd()
        .args(["help", "all", "--json", "--select", ".topics[].id"])
        .assert()
        .success()
        .stdout(contains("\"quickstart\""));
}

#[test]
fn f19_select_help_topic_ids_raw() {
    let out = cmd()
        .args([
            "help",
            "all",
            "--json",
            "--select",
            ".topics[].id",
            "--select-raw",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(out).unwrap();
    // Multiple topic ids, one per line, none quoted.
    assert!(s.lines().count() >= 3, "expected multiple topic lines");
    assert!(
        s.lines().any(|l| l == "quickstart"),
        "expected unquoted 'quickstart' line in:\n{s}"
    );
}

#[test]
fn f19_select_help_slurp_wraps_envelope() {
    // Slurp mode collapses the input stream into an array. `help all
    // --json` is a single envelope, so the slurp array has one
    // element — `.[0].topics | length` reads the topic count out of
    // it. This is the canonical "slurp on a single-envelope verb"
    // shape for agents that wrote a slurp recipe expecting a stream
    // and now get one envelope.
    cmd()
        .args([
            "help",
            "all",
            "--json",
            "--select",
            ".[0].topics | length",
            "--select-slurp",
        ])
        .assert()
        .success();
}

// =========================================================================
// Validation / error-class coverage — flag wiring + clap mutex.
// =========================================================================

#[test]
fn f19_select_requires_json_format() {
    // `audit ls --select '.x'` (no --json) — runtime mutex check
    // in `FormatArgs::resolve()` triggers exit 2 with the canonical
    // error: prefix.
    let home = sandbox();
    cmd()
        .env("INSPECT_HOME", home.path())
        .args(["audit", "ls", "--select", ".summary"])
        .assert()
        .code(2)
        .stderr(contains("error: --select requires --json or --jsonl"));
}

#[test]
fn f19_select_with_csv_errors() {
    // `--csv --select` falls into the same mutex check; `--csv` is
    // not JSON-class, so `--select` is invalid.
    let home = sandbox();
    cmd()
        .env("INSPECT_HOME", home.path())
        .args(["audit", "ls", "--csv", "--select", ".summary"])
        .assert()
        .code(2)
        .stderr(contains("--select requires --json or --jsonl"));
}

#[test]
fn f19_select_quiet_conflict_via_clap() {
    // `--quiet` is `conflicts_with_all = ["json", "jsonl"]` at clap
    // level. `--json --quiet` therefore fails at parse time — exit 2
    // with the canonical clap usage error.
    let home = sandbox();
    cmd()
        .env("INSPECT_HOME", home.path())
        .args(["audit", "ls", "--json", "--quiet", "--select", ".summary"])
        .assert()
        .code(2)
        .stderr(contains("'--json' cannot be used with '--quiet'"));
}

#[test]
fn f19_select_parse_error_exit_2() {
    let home = sandbox();
    cmd()
        .env("INSPECT_HOME", home.path())
        .args(["audit", "ls", "--json", "--select", ".["])
        .assert()
        .code(2)
        .stderr(contains("error: filter parse:"));
}

#[test]
fn f19_select_runtime_error_exit_1() {
    let home = sandbox();
    cmd()
        .env("INSPECT_HOME", home.path())
        .args(["audit", "ls", "--json", "--select", "1 + \"x\""])
        .assert()
        .code(1)
        .stderr(contains("error: filter runtime:"));
}

#[test]
fn f19_select_zero_results_exit_1() {
    // `.data.entries[0].id // empty` yields nothing on an empty
    // audit log; envelope chokepoint returns NoMatches → exit 1.
    let home = sandbox();
    cmd()
        .env("INSPECT_HOME", home.path())
        .args([
            "audit",
            "ls",
            "--json",
            "--select",
            ".data.entries[0].id // empty",
        ])
        .assert()
        .code(1)
        .stdout("");
}

#[test]
fn f19_select_raw_non_string_errors() {
    // `.meta.count` is a number; with `--select-raw` we error
    // exit 1 + the canonical "filter --raw" label.
    let home = sandbox();
    cmd()
        .env("INSPECT_HOME", home.path())
        .args([
            "audit",
            "ls",
            "--json",
            "--select",
            ".meta.count",
            "--select-raw",
        ])
        .assert()
        .code(1)
        .stderr(contains("error: filter --raw:"));
}

#[test]
fn f19_select_raw_requires_select() {
    // `--select-raw` alone (no `--select`) — clap's
    // `requires = "select"` rejects at parse time, exit 2.
    let home = sandbox();
    cmd()
        .env("INSPECT_HOME", home.path())
        .args(["audit", "ls", "--json", "--select-raw"])
        .assert()
        .code(2)
        .stderr(contains("--select-raw"))
        .stderr(contains("--select"));
}

#[test]
fn f19_select_slurp_requires_select() {
    let home = sandbox();
    cmd()
        .env("INSPECT_HOME", home.path())
        .args(["audit", "ls", "--json", "--select-slurp"])
        .assert()
        .code(2)
        .stderr(contains("--select-slurp"))
        .stderr(contains("--select"));
}

// =========================================================================
// Help-surface discoverability — `--select` must appear in every
// JSON-emitting verb's `--help`. Pre-fix, FormatArgs flag docstrings
// didn't exist; agents would burn turns guessing the flag.
// =========================================================================

#[test]
fn f19_help_status_mentions_select() {
    cmd()
        .args(["status", "--help"])
        .assert()
        .stdout(contains("--select"));
}

#[test]
fn f19_help_audit_ls_mentions_select() {
    cmd()
        .args(["audit", "ls", "--help"])
        .assert()
        .stdout(contains("--select"));
}

#[test]
fn f19_help_fleet_mentions_select() {
    // FleetArgs has its own select fields (not via FormatArgs flatten).
    cmd()
        .args(["fleet", "--help"])
        .assert()
        .stdout(contains("--select"));
}

#[test]
fn f19_help_help_mentions_select() {
    // HelpArgs likewise carries select directly.
    cmd()
        .args(["help", "--help"])
        .assert()
        .stdout(contains("--select"));
}
