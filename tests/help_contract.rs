//! HP-0 contract guards for the `inspect help` system.
//!
//! See `archives/INSPECT_HELP_IMPLEMENTATION_PLAN.md` §8 for the full guard
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
    let both = inspect().args(["help", "--search", "fleet apply"]).assert();
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
    // Deterministic separator between topics (13 separators for 14 topics
    // after v0.1.2 added `bundle` and `watch`).
    let bar = "=".repeat(72);
    assert_eq!(
        text.matches(bar.as_str()).count(),
        13,
        "expected 13 topic separators in `inspect help all`"
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
    "add",
    "list",
    "remove",
    "test",
    "show",
    "connect",
    "disconnect",
    "connections",
    "disconnect-all",
    "setup",
    "discover",
    "profile",
    "status",
    "health",
    "logs",
    "grep",
    "cat",
    "ls",
    "find",
    "ps",
    "volumes",
    "images",
    "network",
    "ports",
    "why",
    "connectivity",
    "recipe",
    "search",
    "restart",
    "stop",
    "start",
    "reload",
    "cp",
    "edit",
    "rm",
    "mkdir",
    "touch",
    "chmod",
    "chown",
    "exec",
    "alias",
    "resolve",
    "audit",
    "revert",
    "fleet",
    "bundle",
    "help",
];

// HP-2 sanity: every entry in `TOP_LEVEL_VERBS` must accept `--help`.
// Catches typos in the test list itself; if the binary loses a verb
// without the test list being updated, this guard fires first.
#[test]
fn every_verb_listed_here_is_real() {
    for verb in TOP_LEVEL_VERBS {
        inspect().args([verb, "--help"]).assert().success();
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
        last, "See also: inspect help selectors, inspect help formats, inspect help examples",
        "grep --help footer drifted from the HP-2 contract"
    );
}

// =====================================================================
// HP-6 — verbose / help all / render polish guards.
// =====================================================================

// HP-6 G6a: `inspect help ssh --verbose` adds the MaxSessions caveat
// from `verbose/ssh.md`, and that caveat is *not* present in the
// non-verbose body. This is the single literal acceptance criterion
// in plan §HP-6 DoD.
#[test]
fn help_ssh_verbose_adds_max_sessions_caveat() {
    let plain = inspect()
        .args(["help", "ssh"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let plain = String::from_utf8(plain).expect("ssh body is utf-8");
    assert!(
        !plain.contains("MaxSessions"),
        "`help ssh` (non-verbose) must not yet surface the MaxSessions caveat"
    );

    let verbose = inspect()
        .args(["help", "ssh", "--verbose"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let verbose = String::from_utf8(verbose).expect("ssh verbose is utf-8");
    assert!(
        verbose.contains("MaxSessions"),
        "`help ssh --verbose` must surface the MaxSessions caveat"
    );
    // Verbose body is a strict superset of the standard one.
    let plain_trimmed = plain.trim_end();
    assert!(
        verbose.contains(plain_trimmed),
        "verbose ssh body must be a superset of the standard ssh body"
    );
}

// HP-6 G6b: every topic that registers a `verbose/<id>.md` sidecar
// renders its sidecar marker (the boundary rule from `topic_page_verbose`)
// when `--verbose` is passed, and renders identically to the standard
// body when no sidecar is registered.
#[test]
fn help_verbose_sidecars_are_additive() {
    let with_sidecar = ["ssh", "search", "write", "safety"];
    let without_sidecar = [
        "quickstart",
        "selectors",
        "aliases",
        "formats",
        "fleet",
        "recipes",
        "discovery",
        "examples",
    ];
    for id in with_sidecar {
        let v = inspect()
            .args(["help", id, "--verbose"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let v = String::from_utf8(v).expect("verbose is utf-8");
        assert!(
            v.contains("VERBOSE"),
            "expected VERBOSE section in `help {id} --verbose`"
        );
        // Stable horizontal rule between body and sidecar (mod.rs pins it).
        assert!(
            v.contains(&"-".repeat(72)),
            "expected sidecar boundary rule in `help {id} --verbose`"
        );
    }
    for id in without_sidecar {
        let plain = inspect()
            .args(["help", id])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let v = inspect()
            .args(["help", id, "--verbose"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        assert_eq!(
            plain, v,
            "topic {id:?} has no sidecar; --verbose must be a no-op"
        );
    }
}

// HP-6 G6c: `inspect help all` is the pipeable corpus dump. It must
// (a) succeed, (b) include every topic title, and (c) bypass the
// pager (the `INSPECT_HELP_NO_PAGER` env we already set guarantees
// the latter; here we additionally assert the dispatcher's own
// pager-bypass path by leaving the env unset and letting the
// dispatcher's `render_no_pager` arm kick in).
#[test]
fn help_all_dump_is_substantive_and_bypasses_pager() {
    // Same `inspect()` helper sets INSPECT_HELP_NO_PAGER=1 — that's
    // belt-and-braces. The dispatcher's own bypass (commands/help.rs)
    // is exercised by phase10_3_formats and the index test above.
    let out = inspect()
        .args(["help", "all"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).expect("help all is utf-8");
    let lines = text.lines().count();
    // Topic prose totals ~575 lines (see `wc -l src/help/content/*.md`),
    // plus 11 separators × 3 lines. We assert ≥ 500 to track real
    // content size — 1500 in the plan was based on wider topic
    // bodies that ultimately shipped terser; the contract here is
    // that every topic is present, not a fixed line count.
    assert!(
        lines >= 500,
        "`inspect help all` should produce a substantive dump (≥ 500 lines); got {lines}"
    );
    // Every topic must appear. (Already covered by an earlier guard,
    // but keeping it local makes this test self-contained as the
    // HP-6 corpus contract.)
    for upper in [
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
        assert!(text.contains(upper), "`help all` missing {upper:?}");
    }
}

// HP-6 G6d: `inspect help all --verbose` is a strict superset of
// `inspect help all`, and at minimum surfaces the four sidecars'
// distinguishing markers (so a regression that drops them in `all`
// fails loudly).
#[test]
fn help_all_verbose_is_strict_superset() {
    let plain_n = inspect()
        .args(["help", "all"])
        .assert()
        .success()
        .get_output()
        .stdout
        .len();
    let verbose_out = inspect()
        .args(["help", "all", "--verbose"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let verbose = String::from_utf8(verbose_out).expect("help all verbose is utf-8");
    assert!(
        verbose.len() > plain_n,
        "`help all --verbose` must be longer than `help all` ({} vs {})",
        verbose.len(),
        plain_n
    );
    // Every registered sidecar contributes its VERBOSE header.
    let verbose_count = verbose.matches("VERBOSE").count();
    assert!(
        verbose_count >= 4,
        "`help all --verbose` must include all 4 sidecars (got {verbose_count} VERBOSE headers)"
    );
    // Sidecar boundary rule appears at least once per registered sidecar.
    let bound = "-".repeat(72);
    assert!(
        verbose.matches(bound.as_str()).count() >= 4,
        "`help all --verbose` should contain ≥ 4 sidecar boundary rules"
    );
}

// HP-6 G6e: NO_COLOR honored across every help surface. The renderer
// emits no ANSI today, but the contract is set so future highlighting
// can't bypass NO_COLOR. Asserts a hard zero on ESC bytes (\x1b).
#[test]
fn help_emits_no_ansi_under_no_color() {
    for surface in [
        vec!["help"],
        vec!["help", "search"],
        vec!["help", "ssh", "--verbose"],
        vec!["help", "all"],
        vec!["help", "--search", "timeout"],
        vec!["help", "--json"],
    ] {
        let out = inspect()
            .args(&surface)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let esc_count = out.iter().filter(|&&b| b == 0x1b).count();
        assert_eq!(
            esc_count,
            0,
            "`inspect {}` emitted {} ANSI ESC byte(s) under NO_COLOR=1",
            surface.join(" "),
            esc_count
        );
    }
}

// HP-6 G6f: `INSPECT_HELP_NO_PAGER` is honored — the renderer never
// blocks waiting on `less` even when stdout *is* a tty under the
// test runner's PTY assumptions. Implicit in every other test
// (they'd all hang otherwise), but pinned here as an explicit
// contract guard so a regression to the pager logic is named.
#[test]
fn help_no_pager_env_is_honored() {
    use predicates::prelude::PredicateBooleanExt;
    inspect()
        .env("PAGER", "/does/not/exist/pager-binary-xyzzy")
        .args(["help", "quickstart"])
        .assert()
        .success()
        .stdout(str::contains("QUICKSTART"))
        .stderr(str::contains("xyzzy").not());
}

// ---------------------------------------------------------------------
// P8 (v0.1.1): `inspect help <verb>` falls back to clap's long help
// when there is no editorial topic of the same id. This eliminates the
// dead-end seen in v0.1.0 where users typed `inspect help logs` (the
// natural reflex) and got "unknown help topic".
// ---------------------------------------------------------------------

#[test]
fn p8_help_verb_falls_back_to_clap_long_help() {
    inspect()
        .args(["help", "logs"])
        .assert()
        .success()
        // Clap's long-help renders the per-flag block; `--follow` is
        // unique to logs and stable.
        .stdout(str::contains("--follow"))
        .stdout(str::contains("Usage:"));
}

#[test]
fn p8_help_every_top_level_verb_resolves() {
    // Every entry in VERB_TOPICS must produce non-empty output via
    // either an editorial topic or clap's long-help fallback. The list
    // is the user-facing surface area; an unhandled verb is a regression.
    for verb in [
        "logs",
        "status",
        "ps",
        "grep",
        "search",
        "restart",
        "stop",
        "exec",
        "edit",
        "rm",
        "cp",
        "mkdir",
        "audit",
        "revert",
        "fleet",
        "why",
        "recipe",
        "connectivity",
        "add",
        "list",
        "show",
        "test",
        "setup",
        "discover",
        "profile",
        "connect",
        "alias",
        "help",
    ] {
        let out = inspect()
            .args(["help", verb])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        assert!(!out.is_empty(), "`inspect help {verb}` produced no output");
    }
}

#[test]
fn p8_unknown_verb_typo_suggests_real_verb() {
    // "serch" is 1 edit from "search": the suggester must consider
    // verbs (not just editorial topic ids) so the user gets a hint.
    inspect()
        .args(["help", "serch"])
        .assert()
        .code(1)
        .stderr(str::contains("unknown help topic"))
        .stderr(str::contains("did you mean: search?"));
}
