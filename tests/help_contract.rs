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

// G0g: the placeholder modes (`--search`, `--json`, `help all`) are
// reserved with a clear, non-success error so callers know they exist
// and are scheduled. This guards against accidentally shipping a
// half-implemented mode in HP-0.
#[test]
fn search_flag_is_reserved_until_hp3() {
    inspect()
        .args(["help", "--search", "timeout"])
        .assert()
        .code(2)
        .stderr(str::contains("HP-3"));
}

#[test]
fn json_flag_is_reserved_until_hp4() {
    inspect()
        .args(["help", "--json"])
        .assert()
        .code(2)
        .stderr(str::contains("HP-4"));
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
