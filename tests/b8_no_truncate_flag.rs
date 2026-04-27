//! B8 (v0.1.2) — Output truncation indicator must be loud and offer
//! a `--no-truncate` opt-out.
//!
//! These tests focus on the surface we can control without a real
//! remote host: the help text confirms `--no-truncate` is a documented
//! flag for `inspect run`, and the marker text in the in-process
//! sanitizer is unambiguous (full coverage lives in
//! `src/format/safe.rs::tests`).

use assert_cmd::Command;
use predicates::prelude::*;

fn bin() -> Command {
    Command::cargo_bin("inspect").expect("binary built")
}

#[test]
fn run_help_lists_no_truncate_flag() {
    bin()
        .args(["run", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--no-truncate"));
}

#[test]
fn run_help_describes_no_truncate_purpose() {
    bin()
        .args(["run", "--help"])
        .assert()
        .success()
        // The flag's purpose must mention what it disables. We don't
        // pin exact prose — clap may rewrap — but require the key
        // phrase so a future regression that drops the explanation
        // fails this test.
        .stdout(predicate::str::contains("verbatim").or(predicate::str::contains("byte cap")));
}
