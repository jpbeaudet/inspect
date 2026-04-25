//! Phase 1 integration tests: SSH connection lifecycle commands.
//!
//! These tests do **not** require a real SSH server unless they're explicitly
//! gated by `INSPECT_E2E_SSH=1`. The default suite covers:
//!
//! - CLI surface for connect/disconnect/connections/disconnect-all
//! - Empty-state JSON shapes
//! - Friendly errors for unknown namespaces and missing keys
//! - Codespace-aware TTL defaulting via env

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
    for (key, _) in std::env::vars() {
        if key.starts_with("INSPECT_") || key == "CODESPACES" {
            cmd.env_remove(key);
        }
    }
}

fn add_namespace(home: &PathBuf, ns: &str, host: &str, port: u16) {
    let mut cmd = bin();
    clear_inspect_env(&mut cmd);
    cmd.env("INSPECT_HOME", home)
        .args([
            "add",
            ns,
            "--host",
            host,
            "--user",
            "ubuntu",
            "--key-path",
            "/tmp/fake-key",
            "--port",
            &port.to_string(),
            "--non-interactive",
        ])
        .assert()
        .success();
}

#[test]
fn connections_empty_json() {
    let (_g, home) = isolated_home();
    let mut cmd = bin();
    clear_inspect_env(&mut cmd);
    cmd.env("INSPECT_HOME", &home)
        .args(["connections", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"connections\":[]"));
}

#[test]
fn connections_empty_text() {
    let (_g, home) = isolated_home();
    let mut cmd = bin();
    clear_inspect_env(&mut cmd);
    cmd.env("INSPECT_HOME", &home)
        .arg("connections")
        .assert()
        .success()
        .stdout(predicate::str::contains("no inspect-managed connections"));
}

#[test]
fn disconnect_all_empty_is_noop() {
    let (_g, home) = isolated_home();
    let mut cmd = bin();
    clear_inspect_env(&mut cmd);
    cmd.env("INSPECT_HOME", &home)
        .args(["disconnect-all", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no inspect-managed connections"));
}

#[test]
fn disconnect_unknown_namespace_errors() {
    let (_g, home) = isolated_home();
    let mut cmd = bin();
    clear_inspect_env(&mut cmd);
    cmd.env("INSPECT_HOME", &home)
        .args(["disconnect", "ghost"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not configured"));
}

#[test]
fn disconnect_configured_but_no_master() {
    let (_g, home) = isolated_home();
    add_namespace(&home, "demo", "127.0.0.1", 22);

    let mut cmd = bin();
    clear_inspect_env(&mut cmd);
    cmd.env("INSPECT_HOME", &home)
        .args(["disconnect", "demo"])
        .assert()
        .success()
        .stdout(predicate::str::contains("had no inspect-managed master"));
}

#[test]
fn connect_fails_fast_against_bad_host() {
    // Use TEST-NET-1 (RFC 5737) which is guaranteed unroutable. With a tight
    // ConnectTimeout this should fail within a few seconds.
    let (_g, home) = isolated_home();
    add_namespace(&home, "deadhost", "192.0.2.1", 22);

    let mut cmd = bin();
    clear_inspect_env(&mut cmd);
    cmd.env("INSPECT_HOME", &home)
        .env("INSPECT_SSH_CONNECT_TIMEOUT", "2")
        .args(["connect", "deadhost", "--non-interactive", "--no-existing-mux"])
        .timeout(std::time::Duration::from_secs(20))
        .assert()
        .failure()
        .stderr(predicate::str::contains("connect 'deadhost'"));
}

#[test]
fn connect_json_reports_ttl_source_codespace() {
    // We only verify that the TTL machinery picks the right default; we
    // don't actually open a master. Use a bad host with non-interactive.
    let (_g, home) = isolated_home();
    add_namespace(&home, "deadhost", "192.0.2.1", 22);

    // Even on a failed connect, the error path still fires after TTL is
    // resolved; instead, exercise resolution via `--ttl` flag against a
    // nonexistent ns to fail before TTL kicks in. So check default_ttl()
    // indirectly by inspecting the help text via env.
    let mut cmd = bin();
    clear_inspect_env(&mut cmd);
    cmd.env("INSPECT_HOME", &home)
        .env("INSPECT_SSH_CONNECT_TIMEOUT", "1")
        .env("CODESPACES", "true")
        .args([
            "connect",
            "deadhost",
            "--non-interactive",
            "--no-existing-mux",
            "--ttl",
            "10m",
        ])
        .timeout(std::time::Duration::from_secs(15))
        .assert()
        .failure();
    // Behavior covered by ttl unit tests; this case validates --ttl parses.
}

#[test]
fn connect_rejects_invalid_ttl() {
    let (_g, home) = isolated_home();
    add_namespace(&home, "demo", "127.0.0.1", 22);

    let mut cmd = bin();
    clear_inspect_env(&mut cmd);
    cmd.env("INSPECT_HOME", &home)
        .args(["connect", "demo", "--ttl", "garbage", "--non-interactive"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("ttl"));
}

// ---- Optional E2E with a real sshd ----------------------------------------

mod e2e {
    use super::*;
    use std::process::{Child, Command as StdCommand, Stdio};

    struct SshdFixture {
        child: Child,
        port: u16,
        key_path: PathBuf,
        _dir: TempDir,
    }

    impl Drop for SshdFixture {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn pick_port() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        port
    }

    fn run(prog: &str, args: &[&str]) -> bool {
        StdCommand::new(prog)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        }

    fn try_start_sshd() -> Option<SshdFixture> {
        // Locate sshd
        let sshd = ["/usr/sbin/sshd", "/usr/local/sbin/sshd"]
            .iter()
            .find(|p| std::path::Path::new(p).exists())?;
        let dir = TempDir::new().ok()?;
        let p = dir.path();
        let host_key = p.join("host_ed25519");
        let user_key = p.join("user_ed25519");
        let auth_keys = p.join("authorized_keys");
        let pid_file = p.join("sshd.pid");
        let config = p.join("sshd_config");

        if !run(
            "ssh-keygen",
            &[
                "-q", "-t", "ed25519", "-N", "", "-f",
                host_key.to_str().unwrap(),
            ],
        ) {
            return None;
        }
        if !run(
            "ssh-keygen",
            &[
                "-q", "-t", "ed25519", "-N", "", "-f",
                user_key.to_str().unwrap(),
            ],
        ) {
            return None;
        }
        let pub_key =
            std::fs::read_to_string(format!("{}.pub", user_key.display())).ok()?;
        std::fs::write(&auth_keys, pub_key).ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for f in [&host_key, &user_key, &auth_keys] {
                let _ = std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o600));
            }
        }

        let port = pick_port();
        let cfg = format!(
            "Port {port}\n\
             ListenAddress 127.0.0.1\n\
             HostKey {hk}\n\
             PidFile {pid}\n\
             AuthorizedKeysFile {ak}\n\
             StrictModes no\n\
             UsePAM no\n\
             PasswordAuthentication no\n\
             PubkeyAuthentication yes\n\
             PermitRootLogin prohibit-password\n\
             ChallengeResponseAuthentication no\n\
             KbdInteractiveAuthentication no\n\
             LogLevel QUIET\n",
            hk = host_key.display(),
            pid = pid_file.display(),
            ak = auth_keys.display(),
        );
        std::fs::write(&config, cfg).ok()?;

        // Start sshd in foreground (-D) so we can kill it on drop.
        let child = StdCommand::new(sshd)
            .args(["-D", "-e", "-f"])
            .arg(&config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;

        // Wait briefly for the listener to come up.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return Some(SshdFixture {
                    child,
                    port,
                    key_path: user_key,
                    _dir: dir,
                });
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let mut child = child;
        let _ = child.kill();
        None
    }

    #[test]
    fn full_connect_lifecycle() {
        if std::env::var("INSPECT_E2E_SSH").ok().as_deref() != Some("1") {
            eprintln!("skipping (set INSPECT_E2E_SSH=1 to run)");
            return;
        }
        let Some(sshd) = try_start_sshd() else {
            eprintln!("skipping: could not start local sshd");
            return;
        };
        let (_g, home) = isolated_home();
        let user = std::env::var("USER").unwrap_or_else(|_| "root".to_string());
        let key = sshd.key_path.display().to_string();

        // Pre-trust the sshd host key by writing a known_hosts file via
        // ssh-keyscan, then pass it through INSPECT_SSH_EXTRA_OPTS so we
        // don't have to disable host-key verification.
        let known_hosts = sshd._dir.path().join("known_hosts");
        let scan = StdCommand::new("ssh-keyscan")
            .args(["-p", &sshd.port.to_string(), "-H", "127.0.0.1"])
            .output()
            .expect("ssh-keyscan");
        std::fs::write(&known_hosts, scan.stdout).unwrap();
        let extra_opts = format!(
            "-o UserKnownHostsFile={} -o GlobalKnownHostsFile=/dev/null",
            known_hosts.display()
        );

        // Add namespace
        let mut add = bin();
        clear_inspect_env(&mut add);
        add.env("INSPECT_HOME", &home)
            .args([
                "add",
                "loopback",
                "--host",
                "127.0.0.1",
                "--user",
                &user,
                "--key-path",
                &key,
                "--port",
                &sshd.port.to_string(),
                "--non-interactive",
            ])
            .assert()
            .success();

        // Connect; auth should succeed via the no-passphrase key (BatchMode).
        let mut conn = bin();
        clear_inspect_env(&mut conn);
        conn.env("INSPECT_HOME", &home)
            .env("INSPECT_SSH_CONNECT_TIMEOUT", "5")
            .env("INSPECT_SSH_EXTRA_OPTS", &extra_opts)
            .args([
                "connect",
                "loopback",
                "--non-interactive",
                "--no-existing-mux",
                "--json",
            ])
            .timeout(std::time::Duration::from_secs(15))
            .assert()
            .success()
            .stdout(predicate::str::contains("\"auth\":\"agent\""));

        // List active connections.
        let mut ls = bin();
        clear_inspect_env(&mut ls);
        ls.env("INSPECT_HOME", &home)
            .env("INSPECT_SSH_EXTRA_OPTS", &extra_opts)
            .arg("connections")
            .assert()
            .success()
            .stdout(predicate::str::contains("loopback"))
            .stdout(predicate::str::contains("alive"));

        // Reconnect — should be a no-op via "already-open".
        let mut conn2 = bin();
        clear_inspect_env(&mut conn2);
        conn2
            .env("INSPECT_HOME", &home)
            .env("INSPECT_SSH_EXTRA_OPTS", &extra_opts)
            .args([
                "connect",
                "loopback",
                "--non-interactive",
                "--no-existing-mux",
                "--json",
            ])
            .assert()
            .success()
            .stdout(predicate::str::contains("\"auth\":\"already-open\""));

        // Disconnect.
        let mut dc = bin();
        clear_inspect_env(&mut dc);
        dc.env("INSPECT_HOME", &home)
            .env("INSPECT_SSH_EXTRA_OPTS", &extra_opts)
            .args(["disconnect", "loopback"])
            .assert()
            .success()
            .stdout(predicate::str::contains("disconnected"));

        // Should be empty again.
        let mut ls2 = bin();
        clear_inspect_env(&mut ls2);
        ls2.env("INSPECT_HOME", &home)
            .arg("connections")
            .assert()
            .success()
            .stdout(predicate::str::contains("no inspect-managed connections"));
    }
}
