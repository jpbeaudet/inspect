//! ControlMaster lifecycle: start, check, list, exit.
//!
//! We run the operating system's `ssh` binary so that all security policy
//! (host-key verification, `known_hosts`, ssh-agent integration, ProxyJump,
//! algorithm negotiation) stays in OpenSSH. We never re-implement any of it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};

use super::askpass::AskpassScript;
use super::options::SshTarget;
use crate::error::ConfigError;
use crate::paths;

const ENV_INTERACTIVE_PASSPHRASE: &str = "INSPECT_INTERACTIVE_PASSPHRASE";
const ENV_INTERACTIVE_PASSWORD: &str = "INSPECT_INTERACTIVE_PASSWORD";
const SSH_BIN: &str = "ssh";

/// L4 (v0.1.3): how many wrong passwords we tolerate during a single
/// `inspect connect` invocation before aborting with a chained hint
/// to `inspect help ssh`. Each wrong password costs one ssh master
/// invocation; the cap exists so a noisy keyboard or stale
/// muscle-memory does not lock the operator out repeatedly.
pub const PASSWORD_MAX_ATTEMPTS: usize = 3;

/// How `inspect connect` ultimately authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    /// User already had a mux for this host via their own `~/.ssh/config`.
    ExistingUserMux,
    /// inspect started a master; ssh-agent provided the credentials.
    Agent,
    /// inspect started a master; passphrase came from a configured env var.
    EnvPassphrase,
    /// inspect started a master; passphrase came from interactive prompt.
    InteractivePrompt,
    /// L4 (v0.1.3): inspect started a master; password came from a
    /// configured `password_env`.
    EnvPassword,
    /// L4 (v0.1.3): inspect started a master; password came from an
    /// interactive prompt (one-shot per attempt; up to 3 attempts).
    InteractivePassword,
    /// We didn't open a master because one was already running for this ns.
    AlreadyOpen,
}

impl AuthMode {
    pub fn label(self) -> &'static str {
        match self {
            AuthMode::ExistingUserMux => "existing-user-mux",
            AuthMode::Agent => "agent",
            AuthMode::EnvPassphrase => "env-passphrase",
            AuthMode::InteractivePrompt => "interactive",
            AuthMode::EnvPassword => "env-password",
            AuthMode::InteractivePassword => "interactive-password",
            AuthMode::AlreadyOpen => "already-open",
        }
    }
}

/// Result of [`start_master`].
#[derive(Debug)]
pub struct ConnectOutcome {
    pub auth_mode: AuthMode,
    pub socket: Option<PathBuf>,
    pub ttl: String,
}

/// Status of an inspect-managed master socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MasterStatus {
    Alive,
    Stale,
    Missing,
}

impl MasterStatus {
    pub fn label(self) -> &'static str {
        match self {
            MasterStatus::Alive => "alive",
            MasterStatus::Stale => "stale",
            MasterStatus::Missing => "missing",
        }
    }
}

/// Compute the inspect-managed control-socket path for a namespace.
pub fn socket_path(namespace: &str) -> PathBuf {
    paths::sockets_dir().join(format!("{namespace}.sock"))
}

/// Ensure `~/.inspect/sockets/` exists with mode 0700.
pub fn ensure_sockets_dir() -> std::result::Result<PathBuf, ConfigError> {
    paths::ensure_home()?;
    let dir = paths::sockets_dir();
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| ConfigError::Io {
            path: dir.display().to_string(),
            source: e,
        })?;
    }
    paths::set_dir_mode_0700(&dir)?;
    Ok(dir)
}

/// Probe whether `ssh -O check` says a master is alive at `socket`.
pub fn check_socket(socket: &Path, target: &SshTarget) -> MasterStatus {
    if !socket.exists() {
        return MasterStatus::Missing;
    }
    let mut cmd = Command::new(SSH_BIN);
    cmd.arg("-O")
        .arg("check")
        .arg("-S")
        .arg(socket)
        .arg("-o")
        .arg(format!("ControlPath={}", socket.display()))
        .arg("-o")
        .arg("BatchMode=yes")
        .args(target.base_args())
        .arg(&target.host)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    apply_extra_opts(&mut cmd);
    match cmd.status() {
        Ok(s) if s.success() => MasterStatus::Alive,
        _ => MasterStatus::Stale,
    }
}

/// Probe whether the user's *own* `~/.ssh/config`-driven ControlMaster is
/// already open for this target. If so, we can ride it without starting our
/// own.
fn check_user_existing_mux(target: &SshTarget) -> bool {
    let mut cmd = Command::new(SSH_BIN);
    // Deliberately omit -S / ControlPath so ssh consults user config.
    cmd.arg("-O")
        .arg("check")
        .arg("-o")
        .arg("BatchMode=yes")
        .args(target.base_args())
        .arg(&target.host)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    apply_extra_opts(&mut cmd);
    matches!(cmd.status(), Ok(s) if s.success())
}

/// Send `ssh -O exit` to a control socket and remove the socket file.
pub fn exit_master(socket: &Path, target: &SshTarget) -> Result<()> {
    if !socket.exists() {
        return Ok(());
    }
    let _ = Command::new(SSH_BIN)
        .arg("-O")
        .arg("exit")
        .arg("-S")
        .arg(socket)
        .arg("-o")
        .arg(format!("ControlPath={}", socket.display()))
        .arg("-o")
        .arg("BatchMode=yes")
        .args(target.base_args())
        .arg(&target.host)
        .envs(extra_env_pairs())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    // Best-effort cleanup; the master removes the socket itself on exit.
    if socket.exists() {
        let _ = std::fs::remove_file(socket);
    }
    Ok(())
}

/// List inspect-managed sockets in `~/.inspect/sockets/`.
pub fn list_sockets() -> Result<Vec<(String, PathBuf)>> {
    let dir = paths::sockets_dir();
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(ns) = name.strip_suffix(".sock") else {
            continue;
        };
        out.push((ns.to_string(), path));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// How `start_master` should locate the passphrase, in order of preference.
#[derive(Debug, Clone, Copy)]
pub struct AuthSelection<'a> {
    /// Name of the env var that already contains the passphrase, if the
    /// user configured one.
    pub passphrase_env: Option<&'a str>,
    /// Allow falling back to an interactive prompt on the controlling TTY.
    pub allow_interactive: bool,
    /// Skip the "is there already a user mux?" probe.
    pub skip_existing_mux_check: bool,
    /// L4 (v0.1.3): when `true`, take the password-auth branch instead
    /// of the key-auth branch (skip agent attempt, send
    /// `PreferredAuthentications=password`, use `password_env` or
    /// prompt). Set by `inspect connect` when the resolved namespace
    /// has `auth = "password"`.
    pub password_auth: bool,
    /// L4 (v0.1.3): name of the env var holding the SSH password.
    /// Falls back to interactive prompt when `None` (and
    /// `allow_interactive` is true).
    pub password_env: Option<&'a str>,
}

/// Start (or reuse) a ControlMaster for `target`.
///
/// Order of operations follows the bible:
///
/// 1. If our socket already exists and `ssh -O check` succeeds → reuse it.
/// 2. If user's own `~/.ssh/config`-driven mux exists → reuse that.
/// 3. Try a non-interactive ssh master (agent/keys without passphrase).
/// 4. If `passphrase_env` is set → askpass-from-env.
/// 5. Else if `allow_interactive` → prompt via rpassword and feed askpass.
/// 6. Otherwise return a structured error explaining what's missing.
pub fn start_master(
    namespace: &str,
    target: &SshTarget,
    ttl: &str,
    auth: AuthSelection<'_>,
) -> Result<ConnectOutcome> {
    ensure_sockets_dir().map_err(anyhow::Error::from)?;
    let socket = socket_path(namespace);

    // (1) Our socket alive?
    if matches!(check_socket(&socket, target), MasterStatus::Alive) {
        return Ok(ConnectOutcome {
            auth_mode: AuthMode::AlreadyOpen,
            socket: Some(socket),
            ttl: ttl.to_string(),
        });
    }
    // Stale socket file from a previous master crash.
    if socket.exists() {
        let _ = std::fs::remove_file(&socket);
    }

    // (2) User's own mux already up?
    if !auth.skip_existing_mux_check && check_user_existing_mux(target) {
        return Ok(ConnectOutcome {
            auth_mode: AuthMode::ExistingUserMux,
            socket: None,
            ttl: ttl.to_string(),
        });
    }

    // L4 (v0.1.3): password-auth branch — skip the agent/key attempt
    // entirely (key auth is disabled at the ssh level via
    // `PubkeyAuthentication=no` so a configured agent key cannot
    // bypass the operator's intent to authenticate by password) and
    // run up to PASSWORD_MAX_ATTEMPTS attempts against `password_env`
    // or the interactive prompt.
    if auth.password_auth {
        return start_master_password(namespace, target, ttl, &socket, &auth);
    }

    // (3) Try with BatchMode=yes (agent / keys without passphrase).
    let agent_attempt = run_master(target, ttl, &socket, &[], /*batch=*/ true);
    if agent_attempt.is_ok() {
        return Ok(ConnectOutcome {
            auth_mode: AuthMode::Agent,
            socket: Some(socket),
            ttl: ttl.to_string(),
        });
    }

    // (4) Env-var passphrase.
    if let Some(var) = auth.passphrase_env {
        let value = std::env::var(var).map_err(|_| {
            anyhow!(
                "passphrase env var '{var}' is not set in the current environment; \
                 either export it or unset 'key_passphrase_env' for namespace '{ns}'",
                ns = namespace
            )
        })?;
        if value.is_empty() {
            return Err(anyhow!("passphrase env var '{var}' is empty"));
        }
        let askpass = AskpassScript::new(var)?;
        run_master(
            target,
            ttl,
            &socket,
            &askpass.env_vars(),
            /*batch=*/ false,
        )
        .with_context(|| format!("ssh master failed using env var '{var}'"))?;
        // Best-effort: nothing to zeroize — the secret stays in the user's
        // environment until they unset it. We never copy it.
        let _ = value;
        return Ok(ConnectOutcome {
            auth_mode: AuthMode::EnvPassphrase,
            socket: Some(socket),
            ttl: ttl.to_string(),
        });
    }

    // (5) Interactive.
    if auth.allow_interactive && std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        let prompt = format!(
            "Enter passphrase for SSH key (namespace '{namespace}', host {host}): ",
            host = target.host
        );
        let mut secret = rpassword::prompt_password(&prompt)?;
        if secret.is_empty() {
            return Err(anyhow!("empty passphrase; aborting"));
        }
        // Place the secret into a private env var that the askpass helper
        // reads, then immediately wipe our local copy.
        std::env::set_var(ENV_INTERACTIVE_PASSPHRASE, &secret);
        zeroize_string(&mut secret);
        let askpass = AskpassScript::new(ENV_INTERACTIVE_PASSPHRASE)?;
        let result = run_master(
            target,
            ttl,
            &socket,
            &askpass.env_vars(),
            /*batch=*/ false,
        );
        // Always remove the env var afterward, success or failure.
        std::env::remove_var(ENV_INTERACTIVE_PASSPHRASE);
        result.context("ssh master failed using interactive passphrase")?;
        return Ok(ConnectOutcome {
            auth_mode: AuthMode::InteractivePrompt,
            socket: Some(socket),
            ttl: ttl.to_string(),
        });
    }

    Err(anyhow!(
        "could not authenticate to '{}@{}:{}': no agent identity, \
         no key_passphrase_env configured, and stdin is not a TTY",
        target.user,
        target.host,
        target.port
    ))
}

fn run_master(
    target: &SshTarget,
    ttl: &str,
    socket: &Path,
    extra_env: &[(OsString, OsString)],
    batch: bool,
) -> Result<()> {
    run_master_with_opts(target, ttl, socket, extra_env, batch, &[])
}

/// L4 (v0.1.3): like `run_master` but with caller-supplied ssh `-o`
/// options appended (e.g. `PreferredAuthentications=password` for
/// the password-auth branch). Each entry is a single `KEY=VALUE`
/// string, applied as `-o KEY=VALUE`.
fn run_master_with_opts(
    target: &SshTarget,
    ttl: &str,
    socket: &Path,
    extra_env: &[(OsString, OsString)],
    batch: bool,
    extra_ssh_opts: &[&str],
) -> Result<()> {
    let mut cmd = Command::new(SSH_BIN);
    cmd.arg("-fN") // background master, no remote command
        .arg("-o")
        .arg("ControlMaster=yes")
        .arg("-o")
        .arg(format!("ControlPath={}", socket.display()))
        .arg("-o")
        .arg(format!("ControlPersist={ttl}"))
        .arg("-o")
        .arg("ServerAliveInterval=30")
        .arg("-o")
        .arg("ServerAliveCountMax=3")
        .arg("-o")
        .arg(format!("ConnectTimeout={}", connect_timeout_secs()));
    apply_extra_opts(&mut cmd);
    for opt in extra_ssh_opts {
        cmd.arg("-o").arg(opt);
    }
    if batch {
        cmd.arg("-o").arg("BatchMode=yes");
    }
    cmd.args(target.base_args()).arg(&target.host);

    cmd.stdin(Stdio::null());
    // Surface stderr so users can see ssh's diagnostics directly.
    cmd.stderr(Stdio::inherit());
    cmd.stdout(Stdio::null());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }

    let status = cmd
        .status()
        .with_context(|| format!("failed to invoke '{}'", SSH_BIN))?;
    if !status.success() {
        return Err(anyhow!(
            "ssh exited with status {} while opening master to {}@{}:{}",
            status.code().unwrap_or(-1),
            target.user,
            target.host,
            target.port
        ));
    }
    // Wait briefly for the socket to materialize. ssh -fN backgrounds the
    // master before printing; the socket should appear synchronously, but
    // poll up to 2s to be safe.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if socket.exists() {
            #[cfg(unix)]
            {
                let _ = paths::set_file_mode_0600(socket);
            }
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Err(anyhow!(
        "ssh reported success but control socket {} did not appear within 2s",
        socket.display()
    ))
}

fn zeroize_string(s: &mut String) {
    use zeroize::Zeroize;
    s.zeroize();
}

/// Override via `INSPECT_SSH_CONNECT_TIMEOUT` (seconds). Default 15.
fn connect_timeout_secs() -> u64 {
    std::env::var("INSPECT_SSH_CONNECT_TIMEOUT")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(15)
}

/// Append any operator-supplied raw `ssh` arguments from
/// `INSPECT_SSH_EXTRA_OPTS`. Whitespace-split. Useful for site-specific
/// settings (ProxyJump, UserKnownHostsFile, etc.). Never weakens host-key
/// verification by default — the operator is responsible for what they pass.
fn apply_extra_opts(cmd: &mut Command) {
    if let Ok(extra) = std::env::var("INSPECT_SSH_EXTRA_OPTS") {
        for tok in extra.split_whitespace() {
            cmd.arg(tok);
        }
    }
}

/// `exit_master` uses `Command::envs`, which expects (k,v) pairs. We have
/// none right now (no askpass needed for `-O exit`) but keep the seam.
fn extra_env_pairs() -> Vec<(OsString, OsString)> {
    Vec::new()
}

/// L4 (v0.1.3): ssh `-o` options that switch a master attempt to
/// password authentication only. `PubkeyAuthentication=no` ensures
/// an agent-loaded key cannot pre-empt the operator's intent;
/// `NumberOfPasswordPrompts=1` makes one ssh invocation = one
/// password attempt so the per-call retry loop in
/// `start_master_password` controls the total attempt count.
const PASSWORD_AUTH_SSH_OPTS: &[&str] = &[
    "PreferredAuthentications=password",
    "PubkeyAuthentication=no",
    "NumberOfPasswordPrompts=1",
];

/// L4 (v0.1.3): password-auth branch of `start_master`. Tries
/// `password_env` first when set; otherwise prompts on the
/// controlling tty (when `allow_interactive`); retries up to
/// `PASSWORD_MAX_ATTEMPTS` times on auth failure.
fn start_master_password(
    namespace: &str,
    target: &SshTarget,
    ttl: &str,
    socket: &Path,
    auth: &AuthSelection<'_>,
) -> Result<ConnectOutcome> {
    // Path A: env-var password.
    if let Some(var) = auth.password_env {
        let value = std::env::var(var).map_err(|_| {
            anyhow!(
                "password env var '{var}' is not set in the current environment; \
                 either export it or unset 'password_env' for namespace '{ns}'",
                ns = namespace
            )
        })?;
        if value.is_empty() {
            return Err(anyhow!("password env var '{var}' is empty"));
        }
        let askpass = AskpassScript::new(var)?;
        run_master_with_opts(
            target,
            ttl,
            socket,
            &askpass.env_vars(),
            /*batch=*/ false,
            PASSWORD_AUTH_SSH_OPTS,
        )
        .with_context(|| format!("ssh master failed using password env var '{var}'"))?;
        let _ = value;
        maybe_warn_password_auth(namespace);
        return Ok(ConnectOutcome {
            auth_mode: AuthMode::EnvPassword,
            socket: Some(socket.to_path_buf()),
            ttl: ttl.to_string(),
        });
    }

    // Path B: interactive prompt with up to PASSWORD_MAX_ATTEMPTS retries.
    if !(auth.allow_interactive && std::io::IsTerminal::is_terminal(&std::io::stdin())) {
        return Err(anyhow!(
            "namespace '{namespace}' uses password auth but no \
             password_env is set and stdin is not a TTY (no way to prompt). \
             hint: export the password into an env var and add \
             `password_env = \"VAR_NAME\"` to ~/.inspect/servers.toml, or \
             rerun interactively. see: inspect help ssh"
        ));
    }

    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 1..=PASSWORD_MAX_ATTEMPTS {
        let prompt = format!(
            "Enter SSH password (namespace '{namespace}', host {host}, attempt {attempt}/{max}): ",
            host = target.host,
            max = PASSWORD_MAX_ATTEMPTS,
        );
        let mut secret = rpassword::prompt_password(&prompt)?;
        if secret.is_empty() {
            // Empty input is equivalent to giving up; do not consume an attempt slot.
            zeroize_string(&mut secret);
            return Err(anyhow!("empty password; aborting. see: inspect help ssh"));
        }
        std::env::set_var(ENV_INTERACTIVE_PASSWORD, &secret);
        zeroize_string(&mut secret);
        let askpass = AskpassScript::new(ENV_INTERACTIVE_PASSWORD)?;
        let result = run_master_with_opts(
            target,
            ttl,
            socket,
            &askpass.env_vars(),
            /*batch=*/ false,
            PASSWORD_AUTH_SSH_OPTS,
        );
        std::env::remove_var(ENV_INTERACTIVE_PASSWORD);
        match result {
            Ok(()) => {
                maybe_warn_password_auth(namespace);
                return Ok(ConnectOutcome {
                    auth_mode: AuthMode::InteractivePassword,
                    socket: Some(socket.to_path_buf()),
                    ttl: ttl.to_string(),
                });
            }
            Err(e) => {
                eprintln!(
                    "warning: ssh password attempt {attempt}/{max} failed",
                    max = PASSWORD_MAX_ATTEMPTS
                );
                last_err = Some(e);
            }
        }
    }

    Err(anyhow!(
        "ssh password auth for '{namespace}' failed after {n} attempt(s); \
         aborting. hint: verify the password against the host directly, then retry. \
         see: inspect help ssh\nlast error: {last}",
        n = PASSWORD_MAX_ATTEMPTS,
        last = last_err
            .as_ref()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "unknown".into()),
    ))
}

/// L4 (v0.1.3): per-namespace marker that records whether we have
/// already shown the "password auth is less secure" warning for this
/// namespace. `~/.inspect/.password_warned/<ns>` (touched on first
/// successful password connect, deleted by `inspect ssh add-key`
/// when the operator migrates off password auth so a re-onboarding
/// re-warns).
pub fn password_warned_path(namespace: &str) -> PathBuf {
    paths::inspect_home()
        .join(".password_warned")
        .join(namespace)
}

/// L4 (v0.1.3): emit the one-time warning on first successful
/// password connect for `<ns>`, then create the marker so subsequent
/// connects stay quiet.
fn maybe_warn_password_auth(namespace: &str) {
    let marker = password_warned_path(namespace);
    if marker.exists() {
        return;
    }
    eprintln!(
        "warning: password auth is less secure than key-based. \
         Run 'inspect ssh add-key {namespace}' to migrate."
    );
    if let Some(parent) = marker.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::File::create(&marker);
}
