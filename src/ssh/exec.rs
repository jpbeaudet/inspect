//! Run a command on a remote target through the persistent SSH master.
//!
//! Phase 2 uses this for discovery probes; later phases use it for every
//! read/write verb. The helper:
//!
//! 1. Ensures a master is up (reuse if alive, start otherwise).
//! 2. Invokes `ssh -S <socket> ... <host> -- <cmd>` with `BatchMode=yes`
//!    so we never block on auth here — auth happened at master-open time.
//! 3. Captures stdout/stderr/status.
//!
//! All security policy (host-key verification, ProxyJump) stays in OpenSSH.

use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};

use super::master::{check_socket, socket_path, MasterStatus};
use super::options::SshTarget;

const SSH_BIN: &str = "ssh";

/// Result of a remote command execution.
#[derive(Debug, Clone)]
pub struct RemoteOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

impl RemoteOutput {
    pub fn ok(&self) -> bool {
        self.exit_code == 0
    }
}

/// Options for a single remote run.
#[derive(Debug, Clone, Default)]
pub struct RunOpts {
    /// Maximum time to wait for the remote command. Defaults to 30s.
    pub timeout: Option<Duration>,
}

impl RunOpts {
    pub fn with_timeout(secs: u64) -> Self {
        Self {
            timeout: Some(Duration::from_secs(secs)),
        }
    }
}

/// Run a command on `target` for `namespace`. Requires that an inspect-managed
/// master socket already be open *or* that the user's own ControlMaster is
/// up — we run with `BatchMode=yes` and never prompt.
pub fn run_remote(
    namespace: &str,
    target: &SshTarget,
    cmd: &str,
    opts: RunOpts,
) -> Result<RemoteOutput> {
    let socket = socket_path(namespace);
    let use_socket = matches!(check_socket(&socket, target), MasterStatus::Alive);

    let mut ssh = Command::new(SSH_BIN);
    if use_socket {
        ssh.arg("-S")
            .arg(&socket)
            .arg("-o")
            .arg(format!("ControlPath={}", socket.display()));
    }
    ssh.arg("-o")
        .arg("BatchMode=yes")
        .args(target.base_args());
    apply_extra_opts(&mut ssh);
    ssh.arg(&target.host)
        .arg("--")
        .arg(cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let timeout = opts.timeout.unwrap_or_else(|| Duration::from_secs(30));
    let mut child = ssh
        .spawn()
        .with_context(|| format!("spawning '{SSH_BIN}'"))?;

    // Wait with a soft timeout. We can't easily kill ssh without breaking
    // multiplexed channel state, but we CAN observe and surface a useful
    // error rather than blocking forever.
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().context("waiting on ssh")? {
            let mut stdout = String::new();
            let mut stderr = String::new();
            if let Some(mut o) = child.stdout.take() {
                use std::io::Read;
                o.read_to_string(&mut stdout).ok();
            }
            if let Some(mut e) = child.stderr.take() {
                use std::io::Read;
                e.read_to_string(&mut stderr).ok();
            }
            return Ok(RemoteOutput {
                stdout,
                stderr,
                exit_code: status.code().unwrap_or(-1),
            });
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!(
                "remote command timed out after {}s on '{}': {}",
                timeout.as_secs(),
                namespace,
                truncate(cmd, 120)
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn apply_extra_opts(cmd: &mut Command) {
    if let Ok(extra) = std::env::var("INSPECT_SSH_EXTRA_OPTS") {
        for tok in extra.split_whitespace() {
            cmd.arg(tok);
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut out = s[..max].to_string();
        out.push_str("...");
        out
    }
}
