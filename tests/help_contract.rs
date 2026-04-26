//! HP-0 contract guards for the `inspect help` system.
//!
//! See `INSPECT_HELP_IMPLEMENTATION_PLAN.md` §8 for the full guard
//! catalog. HP-0 ships the four guards that exercise the dispatcher,
//! index page, topic body, and "did you mean" suggestion. HP-1..HP-6
//! add the remainder (G3..G8).

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str;

fn inspect() -> Command {
    let mut c = Command::cargo_bin("inspect").expect("inspect binary built");
    // Discovery probes / ssh subsystems must never fire under the help
    // tests — they do not touch any of those paths, but the env pin is
    // a defence-in-depth measure consistent with the other test files.
    c.env("INSPECT_NON_INTERACTIVE", "1");
    // Force direct stdout so the renderer never tries to spawn a pager
    // under the test runner (where stdout *is* a pipe but `CI` may not
    // be set in every environment).
    c.env("INSPECT_HELP_NO_PAGER", "1");
    // Disable ANSI for byte-stable assertions.
    c.env("NO_COLOR", "1");
    c
}

// G0a: bare `inspect help` prints the index, exits 0.
#[test]
fn index_page_prints_and_exits_zero() {
    inspect()
        .arg("help")
        .assert()
        .success()
        .stdout(str::contains("INSPECT — cross-server debugging"))
        .stdout(str::contains("Topics:"))
        .stdout(str::contains("quickstart"))
        .stdout(str::contains("Commands:"));
}

// G0b: index fits on a single 80x40 screen (bible §2.1).
#[test]
fn index_page_fits_one_screen() {
    let out = inspect()
        .arg("help")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).expect("help index is utf-8");
    let lines = text.lines().count();
    assert!(
        lines <= 40,
        "index page must fit one screen (≤ 40 lines); got {lines}"
    );
}

// G0c: `inspect help quickstart` renders the authored topic body.
#[test]
fn quickstart_topic_renders_body() {
    inspect()
        .args(["help", "quickstart"])
        .assert()
        .success()
        .stdout(str::contains("QUICKSTART"))
        .stdout(str::contains("EXAMPLES"))
        .stdout(str::contains("inspect connect arte"))
        .stdout(str::contains("SEE ALSO"));
}

// G0d: every topic id resolves (stub bodies are acceptable in HP-0).
#[test]
fn every_topic_id_resolves() {
    for id in [
        "quickstart",
        "selectors",
        "aliases",
        "search",
        "formats",
        "write",
        "safety",
        "fleet",
        "recipes",
        "discovery",
        "ssh",
        "examples",
    ] {
        inspect()
            .args(["help", id])
            .assert()
            .success()
            .stdout(str::contains(&id.to_uppercase() as &str));
    }
}

// G0e: unknown topic exits 1 (NoMatches), with a "did you mean" hint
// on stderr when the typo is close to a real topic.
#[test]
fn unknown_topic_returns_nomatches_with_suggestion() {
    inspect()
        .args(["help", "quickstrt"])
        .assert()
        .code(1)
        .stderr(str::contains("unknown help topic"))
        .stderr(str::contains("did you mean: quickstart?"));
}

// G0f: unknown topic with no close match still exits 1 but does not
// fabricate a suggestion (must NOT contain "did you mean").
#[test]
fn unknown_topic_far_from_any_topic_omits_suggestion() {
    inspect()
        .args(["help", "zzzzzzzz"])
        .assert()
        .code(1)
        .stderr(str::contains("unknown help topic"))
        .stderr(str::contains("did you mean").not());
}

// HP-3 G0g/G0h: `--search` is implemented. The flag must
//   * print N matches grouped by topic on stdout for a known hit,
//   * exit 0 on success and 1 on miss (NoMatches),
//   * print a single "no results" line on stderr when empty,
//   * intersect needles when multiple words are given (AND).
#[test]
fn search_finds_timeout_with_at_least_three_hits() {
    let out = inspect()
        .args(["help", "--search", "timeout"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).expect("search output is utf-8");
    // First line is "<N> match(es) for \"timeout\"" — N ≥ 3.
    let first = text.lines().next().unwrap_or_default();
    let n: usize = first
        .split_whitespace()
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    assert!(
        n >= 3,
        "expected ≥3 hits for 'timeout', first line was {first:?}\nfull output:\n{text}"
    );
    // Hits must be grouped by topic and at least the 'search' topic
    // (LogQL doc) and one cmd:* synthetic topic must appear.
    assert!(
        text.contains("\nsearch\n"),
        "expected 'search' topic header in output:\n{text}"
    );
    assert!(
        text.lines().any(|l| l.starts_with("cmd:")),
        "expected at least one cmd:* synthetic topic in output:\n{text}"
    );
}

#[test]
fn search_unknown_keyword_exits_one_with_stderr_line() {
    inspect()
        .args(["help", "--search", "xyzzynonexistent"])
        .assert()
        .code(1)
        .stderr(str::contains("no results for"));
}

#[test]
fn search_and_semantics_intersect() {
    // "fleet apply" should AND-intersect — every result must mention
    // both. We assert via output line count: it cannot exceed either
    // single-needle result count.
    let single = inspect()
        .args(["help", "--search", "apply"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let single = String::from_utf8(single).unwrap();
    let single_n: usize = single
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let both = inspect()
        .args(["help", "--search", "fleet apply"])
        .assert();
    // AND query may legitimately return zero hits if no line mentions
    // both — but if it returns hits, count must not exceed single_n.
    let both_out = both.get_output().stdout.clone();
    let both = String::from_utf8(both_out).unwrap();
    let both_n: usize = both
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    assert!(
        both_n <= single_n,
        "AND-result ({both_n}) must not exceed single-needle result ({single_n})"
    );
}

#[test]
fn json_flag_emits_valid_json() {
    // HP-4 lit this up. Full schema/keys are pinned in
    // tests/help_json_snapshot.rs; here we just guard the dispatcher
    // path: --json succeeds and emits parseable JSON with a
    // schema_version.
    let out = inspect()
        .args(["help", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(out).expect("utf-8");
    let v: serde_json::Value = serde_json::from_str(&s).expect("valid json");
    assert_eq!(v["schema_version"], 1);
}

// HP-1 G0j: `inspect help all` concatenates every topic. The dump
// must contain each topic's title (UPPERCASE) and at least one of the
// known section headers, with deterministic separators between
// topics.
#[test]
fn help_all_dumps_every_topic() {
    let out = inspect()
        .args(["help", "all"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).expect("help all is utf-8");
    for id in [
        "QUICKSTART",
        "SELECTORS",
        "ALIASES",
        "SEARCH",
        "FORMATS",
        "WRITE",
        "SAFETY",
        "FLEET",
        "RECIPES",
        "DISCOVERY",
        "SSH",
        "EXAMPLES",
    ] {
        assert!(
            text.contains(id),
            "`inspect help all` missing topic title {id:?}"
        );
    }
    // Deterministic separator between topics (11 separators for 12 topics).
    let bar = "=".repeat(72);
    assert_eq!(
        text.matches(bar.as_str()).count(),
        11,
        "expected 11 topic separators in `inspect help all`"
    );
}

// HP-1 G4: every authored topic must carry at least 3 copy-pasteable
// `$ inspect ` example lines. This is the bible §3 + plan §9 contract
// and the single biggest "don't let topics rot" guard.
#[test]
fn every_topic_has_at_least_three_examples() {
    for id in [
        "quickstart",
        "selectors",
        "aliases",
        "search",
        "formats",
        "write",
        "safety",
        "fleet",
        "recipes",
        "discovery",
        "ssh",
        "examples",
    ] {
        let out = inspect()
            .args(["help", id])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let text = String::from_utf8(out).expect("topic body is utf-8");
        let count = text
            .lines()
            .filter(|l| l.trim_start().starts_with("$ inspect "))
            .count();
        assert!(
            count >= 3,
            "topic {id:?} must carry ≥3 `$ inspect ` example lines (found {count})"
        );
    }
}

// HP-1 G3 (light): every topic referenced in a `SEE ALSO` block of
// every authored topic must resolve to a real topic id. Hand-checked
// during HP-1 authoring; this test prevents drift in HP-2+.
#[test]
fn every_see_also_reference_resolves() {
    use std::collections::HashSet;
    let known: HashSet<&str> = [
        "quickstart",
        "selectors",
        "aliases",
        "search",
        "formats",
        "write",
        "safety",
        "fleet",
        "recipes",
        "discovery",
        "ssh",
        "examples",
    ]
    .iter()
    .copied()
    .collect();

    for id in known.iter().copied() {
        let out = inspect()
            .args(["help", id])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let text = String::from_utf8(out).expect("topic body is utf-8");
        let mut in_see_also = false;
        for line in text.lines() {
            if line.trim_start().starts_with("SEE ALSO") {
                in_see_also = true;
                continue;
            }
            if !in_see_also {
                continue;
            }
            // The SEE ALSO block ends at the first blank line at the
            // end of the body; entries look like:
            //   inspect help <topic>   <description>
            let l = line.trim_start();
            if let Some(rest) = l.strip_prefix("inspect help ") {
                let referenced = rest
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_');
                assert!(
                    known.contains(referenced),
                    "topic {id:?} SEE ALSO references unknown topic {referenced:?}"
                );
            }
        }
    }
}

// G0h: clap's own auto-generated --help is also accessible. The help
// subcommand should not shadow it.
#[test]
fn clap_help_flag_still_works() {
    inspect()
        .arg("--help")
        .assert()
        .success()
        .stdout(str::contains("Usage:"));
}

// G0i: `inspect help --search foo --json` rejects mutually-exclusive
// mode flags up-front with a deterministic message.
#[test]
fn mutually_exclusive_mode_flags_rejected() {
    inspect()
        .args(["help", "--search", "x", "--json"])
        .assert()
        .code(2)
        .stderr(str::contains("mutually exclusive"));
}

// ---------------------------------------------------------------------------
// HP-2: per-verb cross-link guards.
//
// Integration tests cannot import `Cli` directly (the crate has no `[lib]`
// target), so each guard spawns the binary and inspects its stdout. The
// `SEE_ALSO_*` constants in `cli.rs` carry their own bin-internal unit
// test that pins them to `help::topics::see_also_line`.
// ---------------------------------------------------------------------------

/// Canonical list of every top-level subcommand the binary exposes.
/// Mirrors `Command` in `cli.rs`. The `every_verb_listed_here_is_real`
/// guard below asserts this stays in sync with the binary.
const TOP_LEVEL_VERBS: &[&str] = &[
    "add", "list", "remove", "test", "show",
    "connect", "disconnect", "connections", "disconnect-all",
    "setup", "discover", "profile",
    "status", "health", "logs", "grep", "cat", "ls", "find", "ps",
    "volumes", "images", "network", "ports",
    "why", "connectivity", "recipe",
    "search",
    "restart", "stop", "start", "reload",
    "cp", "edit", "rm", "mkdir", "touch", "chmod", "chown", "exec",
    "alias", "resolve",
    "audit", "revert",
    "fleet",
    "help",
];

// HP-2 sanity: every entry in `TOP_LEVEL_VERBS` must accept `--help`.
// Catches typos in the test list itself; if the binary loses a verb
// without the test list being updated, this guard fires first.
#[test]
fn every_verb_listed_here_is_real() {
    for verb in TOP_LEVEL_VERBS {
        inspect()
            .args([verb, "--help"])
            .assert()
            .success();
    }
}

// HP-2 G1: every top-level subcommand carries a non-empty
// `See also: inspect help …` footer in its `--help` output.
#[test]
fn every_top_level_subcommand_has_see_also_footer() {
    let mut missing: Vec<&str> = Vec::new();
    for verb in TOP_LEVEL_VERBS {
        let out = inspect()
            .args([verb, "--help"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let text = String::from_utf8(out).unwrap_or_default();
        if !text.contains("See also: inspect help ") {
            missing.push(verb);
        }
    }
    assert!(
        missing.is_empty(),
        "subcommands missing `See also: inspect help …` footer: {missing:?}"
    );
}

// HP-2 G2: every top-level subcommand's `--help` carries at least one
// `$ inspect ` example line (bible §HP-2 DoD: --help is self-sufficient).
#[test]
fn every_top_level_subcommand_has_inline_examples() {
    let mut missing: Vec<&str> = Vec::new();
    for verb in TOP_LEVEL_VERBS {
        let out = inspect()
            .args([verb, "--help"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let text = String::from_utf8(out).unwrap_or_default();
        let has_example = text
            .lines()
            .any(|l| l.trim_start().starts_with("$ inspect "));
        if !has_example {
            missing.push(verb);
        }
    }
    assert!(
        missing.is_empty(),
        "subcommands missing inline `$ inspect …` examples: {missing:?}"
    );
}

// HP-2 DoD pin: `inspect grep --help` ends with the exact line
// `See also: inspect help selectors, inspect help formats, inspect help examples`.
// Byte-exact on the last non-blank line — this is the single most
// load-bearing contract test in HP-2.
#[test]
fn grep_help_ends_with_pinned_see_also_line() {
    let out = inspect()
        .args(["grep", "--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).expect("grep --help is utf-8");
    let last = text
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .expect("grep --help has at least one line");
    assert_eq!(
        last,
        "See also: inspect help selectors, inspect help formats, inspect help examples",
        "grep --help footer drifted from the HP-2 contract"
    );
}
