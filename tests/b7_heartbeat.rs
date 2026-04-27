//! B7 (v0.1.2) — `inspect exec` learns `--heartbeat` / `--no-heartbeat`
//! and streams output line-by-line so long-running remote commands no
//! longer look wedged.
//!
//! These tests cover the surface we can verify deterministically without
//! a live remote: that the flags are documented in `--help`. Streaming
//! and heartbeat behavior is exercised by `tests/phase2_discovery.rs`'s
//! `e2e_setup_against_local_sshd` and the existing exec/run integration
//! tests, which now flow through `run_streaming_capturing`.

use assert_cmd::Command;
use predicates::prelude::*;

fn bin() -> Command {
    Command::cargo_bin("inspect").expect("binary built")
}

#[test]
fn exec_help_lists_heartbeat_flag() {
    bin()
        .args(["exec", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--heartbeat"))
        .stdout(predicate::str::contains("--no-heartbeat"));
}

#[test]
fn exec_help_describes_heartbeat_purpose() {
    bin()
        .args(["exec", "--help"])
        .assert()
        .success()
        // Cite the marker phrase the heartbeat itself emits so a
        // future regression that drops the explanation fails this.
        .stdout(predicate::str::contains("still running"));
}

#[test]
fn exec_help_heartbeat_and_no_heartbeat_are_mutually_exclusive() {
    // Clap rejects this combo at parse time; we just need the friendly
    // "cannot be used with" message to appear.
    bin()
        .args([
            "exec",
            "arte/svc",
            "--heartbeat",
            "5",
            "--no-heartbeat",
            "--",
            "true",
        ])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("cannot be used with")
                .or(predicate::str::contains("conflict")),
        );
}
