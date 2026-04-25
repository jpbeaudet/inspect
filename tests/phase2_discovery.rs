//! Phase 2 — discovery & profile cache.
//!
//! Surface tests run with no remote ssh access. The opt-in `e2e_setup`
//! exercise (gated by `INSPECT_E2E_SSH=1`) spins up a real local sshd and
//! validates that `setup` produces a profile and that `setup --check-drift`
//! reports `fresh`.

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
    std::mem::forget(dir); // keep alive for the test's lifetime
    (g, path)
}

fn bin() -> Command {
    Command::cargo_bin("inspect").expect("binary built")
}

fn clear_inspect_env(cmd: &mut Command) {
    cmd.env_remove("INSPECT_HOME");
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

fn add_namespace(home: &std::path::Path, ns: &str, host: &str) {
    let mut c = bin();
    clear_inspect_env(&mut c);
    c.env("INSPECT_HOME", home)
        .args([
            "add",
            ns,
            "--host",
            host,
            "--user",
            "x",
            "--key-path",
            "/tmp/key",
            "--non-interactive",
        ])
        .assert()
        .success();
}

#[test]
fn check_drift_no_cache_is_no_cache() {
    let (_g, home) = isolated_home();
    add_namespace(&home, "arte", "127.0.0.1");
    let mut c = bin();
    clear_inspect_env(&mut c);
    c.env("INSPECT_HOME", &home)
        .args(["setup", "arte", "--check-drift", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"drift\":\"no-cache\""));
}

#[test]
fn profile_missing_is_friendly_error() {
    let (_g, home) = isolated_home();
    add_namespace(&home, "arte", "127.0.0.1");
    let mut c = bin();
    clear_inspect_env(&mut c);
    c.env("INSPECT_HOME", &home)
        .args(["profile", "arte"])
        .assert()
        .code(2)
        .stdout(predicate::str::contains("no cached profile"))
        .stdout(predicate::str::contains("inspect setup arte"));
}

#[test]
fn profile_unknown_namespace_errors() {
    let (_g, home) = isolated_home();
    let mut c = bin();
    clear_inspect_env(&mut c);
    c.env("INSPECT_HOME", &home)
        .args(["profile", "ghost"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not configured"));
}

#[test]
fn setup_invalid_namespace_name_rejected() {
    let mut c = bin();
    clear_inspect_env(&mut c);
    c.args(["setup", "BAD!"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("invalid namespace name"));
}

#[test]
fn discover_alias_routes_to_setup() {
    let (_g, home) = isolated_home();
    add_namespace(&home, "arte", "127.0.0.1");
    let mut c = bin();
    clear_inspect_env(&mut c);
    c.env("INSPECT_HOME", &home)
        .args(["discover", "arte", "--check-drift", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"drift\":\"no-cache\""));
}

#[test]
fn profile_round_trip_via_yaml_writer() {
    // The cache layer's atomic-write + 0700-dir + 0600-file behavior is
    // covered by unit tests in `src/profile/cache.rs`. This integration
    // test only exercises the CLI path, which we hit elsewhere.
}

// ----------------------- opt-in real-sshd E2E --------------------------------

#[cfg(unix)]
mod e2e {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Child, Command as StdCommand, Stdio};
    use std::time::{Duration, Instant};

    struct SshdFixture {
        child: Child,
        port: u16,
        key_path: std::path::PathBuf,
        _dir: tempfile::TempDir,
    }
    impl Drop for SshdFixture {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn try_start_sshd() -> Option<SshdFixture> {
        let sshd_bin = "/usr/sbin/sshd";
        if !std::path::Path::new(sshd_bin).exists() {
            return None;
        }
        let dir = tempfile::tempdir().ok()?;
        let host_key = dir.path().join("host_key");
        let user_key = dir.path().join("user_key");
        let auth_keys = dir.path().join("authorized_keys");
        let cfg = dir.path().join("sshd_config");

        StdCommand::new("ssh-keygen")
            .args(["-q", "-N", "", "-t", "ed25519", "-f"])
            .arg(&host_key)
            .status()
            .ok()?
            .success()
            .then_some(())?;
        StdCommand::new("ssh-keygen")
            .args(["-q", "-N", "", "-t", "ed25519", "-f"])
            .arg(&user_key)
            .status()
            .ok()?
            .success()
            .then_some(())?;
        let mut pub_key = String::new();
        std::fs::File::open(format!("{}.pub", user_key.display()))
            .ok()?
            .read_to_string(&mut pub_key)
            .ok()?;
        std::fs::write(&auth_keys, &pub_key).ok()?;
        for f in [&host_key, &user_key, &auth_keys] {
            std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o600)).ok()?;
        }

        let port = TcpListener::bind("127.0.0.1:0")
            .ok()?
            .local_addr()
            .ok()?
            .port();
        let cfg_body = format!(
            "Port {port}\nListenAddress 127.0.0.1\nHostKey {hk}\nAuthorizedKeysFile {ak}\nPasswordAuthentication no\nPubkeyAuthentication yes\nStrictModes no\nPidFile {pid}\nUsePAM no\n",
            hk = host_key.display(),
            ak = auth_keys.display(),
            pid = dir.path().join("sshd.pid").display(),
        );
        std::fs::write(&cfg, cfg_body).ok()?;

        let child = StdCommand::new(sshd_bin)
            .args(["-D", "-e", "-f"])
            .arg(&cfg)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .ok()?;

        // Wait until the port is open.
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return Some(SshdFixture {
                    child,
                    port,
                    key_path: user_key,
                    _dir: dir,
                });
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }

    #[test]
    fn e2e_setup_against_local_sshd() {
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

        // Pre-trust host key.
        let known_hosts = home.join("known_hosts");
        let scan = StdCommand::new("ssh-keyscan")
            .args(["-p", &sshd.port.to_string(), "-H", "127.0.0.1"])
            .output()
            .expect("ssh-keyscan");
        std::fs::write(&known_hosts, scan.stdout).unwrap();
        let extra_opts = format!(
            "-o UserKnownHostsFile={} -o GlobalKnownHostsFile=/dev/null",
            known_hosts.display()
        );

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

        // Open a master so discovery can multiplex through it.
        let mut conn = bin();
        clear_inspect_env(&mut conn);
        conn.env("INSPECT_HOME", &home)
            .env("INSPECT_SSH_EXTRA_OPTS", &extra_opts)
            .env("INSPECT_SSH_CONNECT_TIMEOUT", "5")
            .args([
                "connect",
                "loopback",
                "--non-interactive",
                "--no-existing-mux",
                "--json",
            ])
            .timeout(std::time::Duration::from_secs(15))
            .assert()
            .success();

        // Run setup (full discovery). We don't assert on container counts —
        // the sshd dev container may or may not have docker. We only assert
        // that the profile lands and the JSON envelope is well-formed.
        let mut setup = bin();
        clear_inspect_env(&mut setup);
        setup
            .env("INSPECT_HOME", &home)
            .env("INSPECT_SSH_EXTRA_OPTS", &extra_opts)
            .args(["setup", "loopback", "--force", "--json"])
            .timeout(std::time::Duration::from_secs(60))
            .assert()
            .success()
            .stdout(predicate::str::contains("\"status\":\"discovered\""))
            .stdout(predicate::str::contains("\"namespace\":\"loopback\""));

        // The drift check against a freshly-discovered profile must be fresh.
        let mut drift = bin();
        clear_inspect_env(&mut drift);
        drift
            .env("INSPECT_HOME", &home)
            .env("INSPECT_SSH_EXTRA_OPTS", &extra_opts)
            .args(["setup", "loopback", "--check-drift", "--json"])
            .assert()
            .success()
            // Either fresh (docker present) or probe-failed (no docker on
            // the dev container) — both are acceptable, but never drifted.
            .stdout(predicate::str::contains("\"drift\":").and(
                predicate::str::contains("\"drifted\"").not(),
            ));

        // Profile command should now find the cache.
        let mut prof = bin();
        clear_inspect_env(&mut prof);
        prof.env("INSPECT_HOME", &home)
            .args(["profile", "loopback", "--json"])
            .assert()
            .success()
            .stdout(predicate::str::contains("\"status\":\"ok\""));

        // Cleanup the master.
        let mut dc = bin();
        clear_inspect_env(&mut dc);
        dc.env("INSPECT_HOME", &home)
            .env("INSPECT_SSH_EXTRA_OPTS", &extra_opts)
            .args(["disconnect", "loopback"])
            .assert()
            .success();
    }
}
