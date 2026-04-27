//! B6 (v0.1.2): regression test for "the CLI must never write files
//! into the user's `$PWD`".
//!
//! Field report (Phase 0 atlas snapshot run, 2026-04-27): an
//! `inspec-clifeedback.md` artifact kept reappearing at the workspace
//! root after `inspect` invocations. Exhaustive audit of `src/`,
//! `scripts/`, `docs/`, and `packaging/` shows that no inspect code
//! path actually writes any markdown file to `$PWD` — the artifact in
//! the operator's tree is a hand-written field-notes file (referenced
//! by the v0.1.1 implementation plan as a *source* document, not a CLI
//! emission). The user's editor / shell history was almost certainly
//! restoring it.
//!
//! This test exists to make that policy load-bearing rather than
//! incidental: if a future commit ever does start writing into `$PWD`,
//! this test fails loudly. It runs the CLI commands most likely to
//! regress (read-only verbs, `setup --check-drift`, `help`, `alias
//! list`, `connections`) inside a freshly-created empty directory and
//! asserts that the directory is still empty afterwards.
//!
//! Defence in depth: we also fence `INSPECT_HOME` to a separate
//! tempdir so config writes go *somewhere*, just never `$PWD`.

use std::fs;
use std::path::Path;

use assert_cmd::Command;

/// Snapshot the entries of a directory as a sorted list of file names.
/// Used to verify the directory hasn't grown across a CLI invocation.
fn list_entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("read tempdir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Build an `inspect` Command pinned to two tempdirs:
/// - `INSPECT_HOME` → `home_dir` (config / audit / cursors land here)
/// - `current_dir` → `pwd_dir` (the directory we're guarding)
///
/// The command is always non-interactive and ANSI-free so output never
/// influences a TTY-only branch that might tempt a future implementer
/// to "drop a hint file" in `$PWD`.
fn inspect(home_dir: &Path, pwd_dir: &Path) -> Command {
    let mut c = Command::cargo_bin("inspect").expect("inspect binary built");
    c.env("INSPECT_HOME", home_dir);
    c.env("INSPECT_NON_INTERACTIVE", "1");
    c.env("INSPECT_HELP_NO_PAGER", "1");
    c.env("NO_COLOR", "1");
    // Belt-and-suspenders: scrub anything that could push the binary
    // toward XDG paths under the user's real home.
    c.env_remove("XDG_CONFIG_HOME");
    c.env_remove("XDG_DATA_HOME");
    c.current_dir(pwd_dir);
    c
}

/// Run a single inspect invocation against fresh tempdirs and assert
/// the `pwd` tempdir is byte-identical empty before and after. We
/// don't care about the inspect exit code (some of these commands
/// exit non-zero by design when no namespaces are configured) — we
/// only care that the process doesn't drop files in `$PWD`.
fn assert_pwd_untouched(args: &[&str]) {
    let home = tempfile::tempdir().expect("home tempdir");
    let pwd = tempfile::tempdir().expect("pwd tempdir");

    let before = list_entries(pwd.path());
    assert!(
        before.is_empty(),
        "pwd tempdir should start empty, got: {before:?}"
    );

    let _ = inspect(home.path(), pwd.path()).args(args).assert(); // exit code intentionally ignored

    let after = list_entries(pwd.path());
    assert!(
        after.is_empty(),
        "B6 regression: `inspect {}` wrote {} file(s) into $PWD: {:?}",
        args.join(" "),
        after.len(),
        after,
    );
}

#[test]
fn help_index_does_not_touch_pwd() {
    assert_pwd_untouched(&["help"]);
}

#[test]
fn help_topic_does_not_touch_pwd() {
    assert_pwd_untouched(&["help", "selectors"]);
}

#[test]
fn list_does_not_touch_pwd() {
    // No namespaces configured → the verb still must not drop files.
    assert_pwd_untouched(&["list"]);
}

#[test]
fn connections_does_not_touch_pwd() {
    assert_pwd_untouched(&["connections"]);
}

#[test]
fn alias_list_does_not_touch_pwd() {
    assert_pwd_untouched(&["alias", "list"]);
}

#[test]
fn version_does_not_touch_pwd() {
    assert_pwd_untouched(&["--version"]);
}

#[test]
fn setup_missing_namespace_does_not_touch_pwd() {
    // `inspect setup arte --check-drift` against an empty config will
    // fail (no namespace 'arte'), but per B6 it must fail *cleanly* —
    // it must not leave a feedback.md, a stub config, or any other
    // artifact in the operator's working directory.
    assert_pwd_untouched(&["setup", "arte", "--check-drift"]);
}

#[test]
fn invalid_command_does_not_touch_pwd() {
    // clap's "unknown subcommand" error path runs before any verb
    // logic. Guard it just in case a future enhancement decides to
    // "be helpful" and write a how-to file.
    assert_pwd_untouched(&["wat-is-this-command"]);
}
