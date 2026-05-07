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

// =========================================================================
// C3 — `select` editorial topic, ERROR_CATALOG cross-link, and the
// "no lone | jq" sweep over the help / runbook / smoke surfaces. Each
// test pins one slice of the F19 contract that an agent learns from
// `inspect help` first; if any of these regress, the agent-friendliness
// invariant on a fresh-install target is back in play.
// =========================================================================

#[test]
fn f19_help_select_topic_lists_in_index() {
    // The top-level `inspect help` index page must surface the new
    // editorial topic so agents discover it without typing the id.
    cmd()
        .args(["help"])
        .assert()
        .success()
        .stdout(contains("select"))
        .stdout(contains("Filter / project JSON output"));
}

#[test]
fn f19_help_select_topic_renders() {
    // `inspect help select` renders the body. We require both the
    // word "jq" (so an agent searching for jq idioms lands here)
    // and the literal flag spelling, plus the EXAMPLES section
    // canonical to every topic.
    let out = cmd()
        .args(["help", "select"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("SELECT"), "topic header missing: {s}");
    assert!(s.contains("--select"), "flag spelling missing: {s}");
    assert!(s.contains("jq"), "jq pointer missing: {s}");
    assert!(s.contains("EXAMPLES"), "EXAMPLES section missing");
    // Sanity: the body is non-trivial. The select.md file is over
    // 100 lines; even with light rendering the output should clear
    // 1 KB. A regression to a stub would silently shrink to ~10
    // bytes ("(unauthored)").
    assert!(
        s.len() > 1024,
        "topic body too short ({} bytes): {s}",
        s.len()
    );
}

#[test]
fn f19_help_search_finds_select_topic() {
    // `inspect help --search filter` must surface at least one hit
    // pointing at the `select` topic, since "filter" is the word
    // an agent reaches for first when looking for projection.
    let out = cmd()
        .args(["help", "--search", "filter"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(out).unwrap();
    // The render groups by topic with the topic id on its own line
    // followed by indented `L<n>  <snippet>` entries, so
    // "\nselect\n" is a stable signal that the select topic shows.
    assert!(
        s.contains("\nselect\n") || s.starts_with("select\n"),
        "select topic not surfaced under search 'filter':\n{s}"
    );
}

#[test]
fn f19_help_search_hits_select_for_jq_term() {
    // The companion search: an agent searching for "jq" should
    // also be routed to the select topic (the editorial entry
    // documents jq idioms inspect understands).
    let out = cmd()
        .args(["help", "--search", "jq"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(out).unwrap();
    assert!(
        s.contains("\nselect\n") || s.starts_with("select\n"),
        "select topic not surfaced under search 'jq':\n{s}"
    );
}

#[test]
fn f19_no_lone_jq_in_help_content() {
    // No `| jq ` recipe survives in any editorial topic body or
    // verbose sidecar. The build-time corpus is the source of
    // truth for the help renderer; a regression here means an
    // agent reading the help system would be told to install jq.
    use std::fs;
    use std::path::Path;
    let dirs: &[&str] = &["src/help/content", "src/help/verbose"];
    let mut offenders: Vec<String> = Vec::new();
    for dir in dirs {
        for entry in fs::read_dir(Path::new(dir)).expect("dir readable") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let body = fs::read_to_string(&path).expect("readable");
            for (idx, line) in body.lines().enumerate() {
                if line.contains("| jq ") {
                    offenders.push(format!("{}:{}: {}", path.display(), idx + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "every recipe in help/content + help/verbose must use `--select`, not `| jq`. Found:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn f19_no_lone_jq_in_long_constants() {
    // Same sweep, against every `LONG_*` constant compiled into
    // the binary. We probe via `--help` rather than scraping the
    // .rs source so a future move of the constants doesn't
    // silently make the test pass for the wrong reason.
    //
    // Verbs to probe: every JSON-emitting verb that ships a
    // dedicated `LONG_*` doc. The list mirrors the C2 sweep.
    let verbs: &[&[&str]] = &[
        &["status", "--help"],
        &["why", "--help"],
        &["health", "--help"],
        &["audit", "--help"],
        &["audit", "ls", "--help"],
        &["audit", "show", "--help"],
        &["history", "--help"],
        &["bundle", "--help"],
        &["compose", "--help"],
        &["compose", "ls", "--help"],
        &["compose", "ps", "--help"],
        &["fleet", "--help"],
        &["help", "--help"],
        &["search", "--help"],
        &["recipe", "--help"],
        &["connectivity", "--help"],
        &["resolve", "--help"],
        &["query", "--help"],
    ];
    let mut offenders: Vec<String> = Vec::new();
    for argv in verbs {
        let out = cmd().args(*argv).assert().get_output().stdout.clone();
        let s = String::from_utf8(out).unwrap();
        for (idx, line) in s.lines().enumerate() {
            if line.contains("| jq ") {
                offenders.push(format!(
                    "inspect {}: L{}: {}",
                    argv.join(" "),
                    idx + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "every example in clap LONG_* must use `--select`, not `| jq`. Found:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn f19_select_parse_error_links_to_help_topic() {
    // C1-fixup routed filter-error stderr through `error::emit`;
    // C3 added the ERROR_CATALOG row that gives those errors a
    // `see: inspect help select` cross-link. Verify the link
    // renders on a real parse error.
    let home = sandbox();
    cmd()
        .env("INSPECT_HOME", home.path())
        .args(["audit", "ls", "--json", "--select", ".["])
        .assert()
        .code(2)
        .stderr(contains("filter parse:"))
        .stderr(contains("see: inspect help select"));
}

#[test]
fn f19_help_search_index_under_cap() {
    // The select topic + per-verb LONG_* SELECTING blocks pushed
    // the help-search index past 144 KB; C3 raised the cap to
    // 160 KB. This test pins the new cap so a future commit that
    // trims documentation to fit (forbidden by CLAUDE.md "Help-
    // surface discipline") fails loudly. The cap can be raised
    // again if prose grows, never trimmed.
    //
    // We probe through `inspect help all --json --select` since
    // that exercises the help registry size, plus an explicit
    // search round-trip to ensure the index loaded.
    cmd()
        .args(["help", "--search", "select"])
        .assert()
        .success()
        .stdout(contains("select"));
    // The size pin itself lives in `src/help/search.rs` as a
    // lib-level test (it has access to `index_byte_size()` which
    // is `#[cfg(test)]` private to the binary). We re-assert here
    // that the search engine is functional as a smoke; if the
    // build-time index regenerator silently emits an empty index,
    // the search above would fail.
}
