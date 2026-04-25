//! Integration tests for Phase 0 commands.
//!
//! Each test isolates `INSPECT_HOME` to a fresh tempdir to avoid touching
//! the real `~/.inspect/` and to allow parallel execution.

use std::path::PathBuf;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn bin() -> Command {
    Command::cargo_bin("inspect").expect("inspect binary built")
}

fn isolated_home() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().to_path_buf();
    (dir, path)
}

fn clear_inspect_env(cmd: &mut Command) {
    // Remove any inherited INSPECT_* variables to keep tests deterministic.
    for (key, _) in std::env::vars() {
        if key.starts_with("INSPECT_") {
            cmd.env_remove(key);
        }
    }
}

#[test]
fn list_empty_when_no_namespaces() {
    let (_g, home) = isolated_home();
    let mut cmd = bin();
    clear_inspect_env(&mut cmd);
    cmd.env("INSPECT_HOME", &home)
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("no namespaces configured"));
}

#[test]
fn add_then_show_then_list_roundtrip() {
    let (_g, home) = isolated_home();

    let mut add = bin();
    clear_inspect_env(&mut add);
    add.env("INSPECT_HOME", &home)
        .args([
            "add",
            "arte",
            "--host",
            "arte.example.internal",
            "--user",
            "ubuntu",
            "--key-path",
            "/tmp/fake-key",
            "--port",
            "2222",
            "--non-interactive",
        ])
        .assert()
        .success();

    // File is created with mode 0600.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(home.join("servers.toml")).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }

    let mut show = bin();
    clear_inspect_env(&mut show);
    show.env("INSPECT_HOME", &home)
        .args(["show", "arte"])
        .assert()
        .success()
        .stdout(predicate::str::contains("arte.example.internal"))
        .stdout(predicate::str::contains("ubuntu"))
        .stdout(predicate::str::contains("2222"));

    let mut list = bin();
    clear_inspect_env(&mut list);
    list.env("INSPECT_HOME", &home)
        .args(["list", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"name\":\"arte\""))
        .stdout(predicate::str::contains("\"source\":\"file\""));
}

#[test]
fn add_rejects_duplicate_without_force() {
    let (_g, home) = isolated_home();
    let common = [
        "--host",
        "h",
        "--user",
        "u",
        "--key-path",
        "/tmp/k",
        "--non-interactive",
    ];

    let mut first = bin();
    clear_inspect_env(&mut first);
    first
        .env("INSPECT_HOME", &home)
        .arg("add")
        .arg("dup")
        .args(common)
        .assert()
        .success();

    let mut second = bin();
    clear_inspect_env(&mut second);
    second
        .env("INSPECT_HOME", &home)
        .arg("add")
        .arg("dup")
        .args(common)
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));

    let mut forced = bin();
    clear_inspect_env(&mut forced);
    forced
        .env("INSPECT_HOME", &home)
        .arg("add")
        .arg("dup")
        .args(common)
        .arg("--force")
        .assert()
        .success();
}

#[test]
fn invalid_namespace_name_rejected() {
    let (_g, home) = isolated_home();
    let mut cmd = bin();
    clear_inspect_env(&mut cmd);
    cmd.env("INSPECT_HOME", &home)
        .args([
            "add",
            "Bad Name!",
            "--host",
            "h",
            "--user",
            "u",
            "--key-path",
            "/tmp/k",
            "--non-interactive",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid namespace name"));
}

#[test]
fn env_overrides_file_for_show() {
    let (_g, home) = isolated_home();

    let mut add = bin();
    clear_inspect_env(&mut add);
    add.env("INSPECT_HOME", &home)
        .args([
            "add",
            "arte",
            "--host",
            "from-file.example",
            "--user",
            "fileuser",
            "--key-path",
            "/tmp/k",
            "--non-interactive",
        ])
        .assert()
        .success();

    let mut show = bin();
    clear_inspect_env(&mut show);
    show.env("INSPECT_HOME", &home)
        .env("INSPECT_ARTE_HOST", "from-env.example")
        .args(["show", "arte"])
        .assert()
        .success()
        .stdout(predicate::str::contains("from-env.example"))
        .stdout(predicate::str::contains("env-over-file"));
}

#[test]
fn env_only_namespace_appears_in_list() {
    let (_g, home) = isolated_home();
    let mut cmd = bin();
    clear_inspect_env(&mut cmd);
    cmd.env("INSPECT_HOME", &home)
        .env("INSPECT_PROD_HOST", "prod.example")
        .env("INSPECT_PROD_USER", "ops")
        .env("INSPECT_PROD_KEY_PATH", "/tmp/k")
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("prod"))
        .stdout(predicate::str::contains("env"));
}

#[test]
fn show_redacts_inline_key() {
    let (_g, home) = isolated_home();
    let mut cmd = bin();
    clear_inspect_env(&mut cmd);
    cmd.env("INSPECT_HOME", &home)
        .env("INSPECT_SECRET_HOST", "h")
        .env("INSPECT_SECRET_USER", "u")
        .env(
            "INSPECT_SECRET_KEY_INLINE",
            "PLEASE_DO_NOT_LEAK_THIS_PRIVATE_KEY",
        )
        .args(["show", "secret"])
        .assert()
        .success()
        .stdout(predicate::str::contains("<redacted>"))
        .stdout(predicate::str::contains("PLEASE_DO_NOT_LEAK_THIS_PRIVATE_KEY").not());
}

#[test]
fn unknown_namespace_errors_with_friendly_message() {
    let (_g, home) = isolated_home();
    let mut cmd = bin();
    clear_inspect_env(&mut cmd);
    cmd.env("INSPECT_HOME", &home)
        .args(["show", "nope"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not configured"));
}

#[test]
fn remove_deletes_namespace() {
    let (_g, home) = isolated_home();
    let mut add = bin();
    clear_inspect_env(&mut add);
    add.env("INSPECT_HOME", &home)
        .args([
            "add",
            "rmme",
            "--host",
            "h",
            "--user",
            "u",
            "--key-path",
            "/tmp/k",
            "--non-interactive",
        ])
        .assert()
        .success();

    let mut rm = bin();
    clear_inspect_env(&mut rm);
    rm.env("INSPECT_HOME", &home)
        .args(["remove", "rmme", "--yes"])
        .assert()
        .success();

    let mut show = bin();
    clear_inspect_env(&mut show);
    show.env("INSPECT_HOME", &home)
        .args(["show", "rmme"])
        .assert()
        .failure();
}

#[test]
fn rejects_unsafe_permissions_on_servers_toml() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let (_g, home) = isolated_home();
        std::fs::create_dir_all(&home).unwrap();
        let path = home.join("servers.toml");
        std::fs::write(
            &path,
            "schema_version = 1\n[namespaces.bad]\nhost=\"h\"\nuser=\"u\"\nkey_path=\"/tmp/k\"\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&path, perms).unwrap();

        let mut cmd = bin();
        clear_inspect_env(&mut cmd);
        cmd.env("INSPECT_HOME", &home)
            .arg("list")
            .assert()
            .failure()
            .stderr(predicate::str::contains("unsafe permissions"));
    }
}

#[test]
fn placeholder_verb_returns_phase_message() {
    let (_g, home) = isolated_home();
    let mut cmd = bin();
    clear_inspect_env(&mut cmd);
    cmd.env("INSPECT_HOME", &home)
        .args(["why", "arte"])
        .assert()
        .failure() // exit 2 for unimplemented verbs
        .stdout(predicate::str::contains("not implemented yet"))
        .stdout(predicate::str::contains("Phase 9"));
}
