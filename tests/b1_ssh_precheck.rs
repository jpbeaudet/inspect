//! B1 (v0.1.2) integration: when SSH auth or connectivity fails during
//! `inspect setup`, we surface a single chained hint and exit 2 instead
//! of producing a half-empty profile full of swallowed warnings.
//!
//! These tests use a TCP port we know is closed (port 1, reserved
//! tcpmux) so the `ssh` binary itself returns a deterministic
//! `Connection refused`. We only assert on the user-visible shape of
//! the error message; behavior of the network stack itself is out of
//! scope.

use std::sync::{Mutex, MutexGuard, OnceLock};

use assert_cmd::Command;
use predicates::prelude::*;

fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

fn isolated_home() -> (MutexGuard<'static, ()>, std::path::PathBuf) {
    let g = env_lock();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    std::mem::forget(dir);
    (g, path)
}

fn bin() -> Command {
    Command::cargo_bin("inspect").expect("binary built")
}

fn clear_inspect_env(cmd: &mut Command) {
    cmd.env_remove("INSPECT_HOME");
    cmd.env_remove("INSPECT_SSH_EXTRA_OPTS");
    for k in [
        "INSPECT_HOST",
        "INSPECT_USER",
        "INSPECT_KEY_PATH",
        "INSPECT_KEY_PASSPHRASE_ENV",
        "INSPECT_PORT",
    ] {
        cmd.env_remove(k);
        cmd.env_remove(format!("{k}_LOOPBACK"));
        cmd.env_remove(format!("{k}_ARTE"));
    }
}

fn add_unreachable_namespace(home: &std::path::Path, ns: &str) {
    let mut c = bin();
    clear_inspect_env(&mut c);
    c.env("INSPECT_HOME", home)
        .args([
            "add",
            ns,
            "--host",
            "127.0.0.1",
            "--port",
            "1", // tcpmux, closed almost everywhere
            "--user",
            "nobody",
            "--key-path",
            "/tmp/nonexistent-key",
            "--non-interactive",
        ])
        .assert()
        .success();
}

#[test]
fn setup_against_unreachable_host_emits_chained_hint() {
    let (_g, home) = isolated_home();
    add_unreachable_namespace(&home, "ghost");
    let mut c = bin();
    clear_inspect_env(&mut c);
    c.env("INSPECT_HOME", &home)
        .env("INSPECT_SSH_CONNECT_TIMEOUT", "2")
        .args(["setup", "ghost"])
        .timeout(std::time::Duration::from_secs(20))
        .assert()
        .code(2)
        // The hint must be a single, structured paragraph — not a wall
        // of swallowed warnings.
        .stderr(
            // Either AuthFailed or Unreachable depending on platform —
            // both are acceptable B1 shapes; what matters is that the
            // user gets a chained "→ then retry: inspect setup ghost"
            // line and the help-topic pointer.
            predicate::str::contains("ghost")
                .and(predicate::str::contains("inspect setup ghost"))
                .and(predicate::str::contains("see: inspect help ssh")),
        );
}

#[test]
fn setup_against_unreachable_host_does_not_persist_partial_profile() {
    let (_g, home) = isolated_home();
    add_unreachable_namespace(&home, "ghost");
    let mut c = bin();
    clear_inspect_env(&mut c);
    let _ = c
        .env("INSPECT_HOME", &home)
        .env("INSPECT_SSH_CONNECT_TIMEOUT", "2")
        .args(["setup", "ghost"])
        .timeout(std::time::Duration::from_secs(20))
        .assert()
        .code(2);

    // No profile cache must have been written for a failed precheck:
    // ~/.inspect/profiles/ghost.json should not exist.
    let profile = home.join("profiles").join("ghost.json");
    assert!(
        !profile.exists(),
        "precheck failure must not persist a partial profile at {}",
        profile.display()
    );
}
