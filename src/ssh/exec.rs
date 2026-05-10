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

/// When ssh itself fails (master gone, host unreachable,
/// auth rejected) it exits non-zero with a recognisable stderr token.
/// `dispatch_with_reauth` only triggers on `Err(...)`, so we must
/// promote those transport failures to errors here — otherwise the
/// caller sees `Ok(RemoteOutput { exit_code: 255, .. })` and the
/// auto-reauth path never fires. Returns `Some(Err(..))` when stderr
/// matches a transport classification, `None` for genuine remote
/// command failures (which propagate as `Ok(exit_code)` unchanged).
fn transport_err(exit_code: i32, stderr: &str) -> Option<anyhow::Error> {
    if exit_code == 0 {
        return None;
    }
    if super::transport::classify(stderr).is_some() {
        // Use the raw stderr as the error message so
        // `dispatch_with_reauth`'s `classify(&err.to_string())` re-
        // matches the same tokens. Trim trailing whitespace for a
        // cleaner SUMMARY render.
        let trimmed = stderr.trim_end();
        if trimmed.is_empty() {
            return Some(anyhow!("ssh transport failure (exit {exit_code})"));
        }
        return Some(anyhow!("{trimmed}"));
    }
    None
}

/// Decide whether the captured stderr from a
/// streaming remote command should be surfaced to the operator on a
/// non-zero exit. Returns `Some(trimmed)` when there's a useful
/// command-failure diagnostic, or `None` when the failure has already
/// been surfaced by a typed Err (transport / max-sessions) or there's
/// no stderr to show.
///
/// Pre-fix, [`run_remote_streaming`] collected stderr only for
/// transport classification and silently dropped command-failure
/// stderr — leaving agents driving `inspect run` / `inspect logs` /
/// streaming compose verbs with `arte: exit N` and no path to
/// "what to fix". This helper keeps the decision in one place so the
/// `run_remote_streaming` call site stays a tight conditional and
/// the contract is unit-testable without an SSH child process.
///
/// The decision intentionally early-returns `None` for transport-
/// class and max-sessions stderr because both shapes are already
/// surfaced upstream as typed [`anyhow::Error`] values (with their
/// own operator-friendly wording). Emitting again here would
/// double-print.
fn command_failure_stderr(exit_code: i32, stderr: &str) -> Option<&str> {
    if exit_code == 0 {
        return None;
    }
    let trimmed = stderr.trim_end();
    if trimmed.is_empty() {
        return None;
    }
    if super::concurrency::looks_like_max_sessions(trimmed) {
        return None;
    }
    if super::transport::classify(trimmed).is_some() {
        return None;
    }
    Some(trimmed)
}

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
    /// Bytes to forward as the remote command's stdin.
    /// `None` (default) means stdin is `/dev/null`, matching pre-v0.1.3
    /// behavior. `Some(bytes)` pipes those bytes byte-for-byte to the
    /// remote command's stdin and closes the channel on EOF, so
    /// commands that read until EOF (`sh`, `psql`, `cat`, `tee`)
    /// terminate normally.
    pub stdin: Option<Vec<u8>>,
    /// Force PTY allocation (`ssh -tt`). Two effects:
    /// (1) remote stdio flips from block-buffered to line-buffered, so
    /// `docker logs -f` / `tail -f` / `journalctl -fu` deliver lines
    /// in real time instead of in 4 KB bursts; (2) local Ctrl-C
    /// (SIGINT) propagates through the PTY layer to the remote
    /// process, so `--stream` invocations actually kill the remote
    /// command instead of leaving it orphaned. Off by default for
    /// non-streaming runs because PTY allocation can change command
    /// behaviour (CRLF endings, color output, prompt suppression).
    pub tty: bool,
}

impl RunOpts {
    pub fn with_timeout(secs: u64) -> Self {
        // Field pitfall §4.1: operators on high-latency fleets
        // (cross-region, weak SSH, sluggish daemons) need the option
        // to globally raise the per-host timeout without us hand-
        // editing every call site. `INSPECT_HOST_TIMEOUT_SECS=N` acts
        // as a *floor*: it never lowers a caller's explicit ask
        // (e.g. recipe steps that intentionally use a 10s deadline)
        // but it raises shorter defaults so a `discover` against a
        // 90-second-handshake host doesn't time out at the default 30s.
        let final_secs = match std::env::var("INSPECT_HOST_TIMEOUT_SECS") {
            Ok(v) => v.parse::<u64>().ok().map(|n| n.max(secs)).unwrap_or(secs),
            Err(_) => secs,
        };
        Self {
            timeout: Some(Duration::from_secs(final_secs)),
            stdin: None,
            tty: false,
        }
    }

    /// Forward `bytes` to the remote command's stdin.
    /// Builder-style for ergonomic call sites.
    pub fn with_stdin(mut self, bytes: Vec<u8>) -> Self {
        self.stdin = Some(bytes);
        self
    }

    /// Force PTY allocation (`ssh -tt`) on this run.
    /// Required for `--stream` so the remote process line-buffers and
    /// SIGINT propagates back through the PTY. Builder-style.
    pub fn with_tty(mut self, tty: bool) -> Self {
        self.tty = tty;
        self
    }
}

/// Run a command on `target` for `namespace`. Requires that an inspect-managed
/// master socket already be open *or* that the user's own ControlMaster is
/// up — we run with `BatchMode=yes` and never prompt.
pub fn run_remote(
    namespace: &str,
    target: &SshTarget,
    cmd: &str,
    mut opts: RunOpts,
) -> Result<RemoteOutput> {
    // Per-host MaxSessions throttle (audit §3.3). Acquired *before*
    // we spawn ssh so we never overshoot the server-side cap. Released
    // automatically when `_session` drops at end of function.
    let _session =
        super::concurrency::acquire(&target.host).context("acquiring SSH session slot")?;

    let socket = socket_path(namespace);
    let use_socket = match check_socket(&socket, target) {
        MasterStatus::Alive => true,
        MasterStatus::Stale | MasterStatus::Missing => {
            // No live ControlMaster — either the
            // socket file exists but the master process is gone
            // (Stale: codespace restart, OOM, ControlPersist expiry)
            // or the master was never opened / a prior reauth
            // cleaned up the socket (Missing). In both cases we
            // must NOT fall through to a fresh `ssh -o BatchMode=yes`
            // dispatch: that path silently fails with "Permission
            // denied (publickey)" for any encrypted-key namespace
            // because BatchMode forbids passphrase prompts, and the
            // classifier would then mis-route the recovery to
            // AuthFailed (terminal) instead of Stale (auto-reauth).
            // Short-circuit with a stale-transport stderr token so
            // `dispatch_with_reauth` detects it and triggers
            // `runner.reauth()` — which walks the full auth ladder
            // (agent → env → keychain → interactive prompt).
            return Err(anyhow!(
                "control socket connect({}): Connection refused (master gone)",
                socket.display()
            ));
        }
    };

    let mut ssh = Command::new(SSH_BIN);
    if use_socket {
        ssh.arg("-S")
            .arg(&socket)
            .arg("-o")
            .arg(format!("ControlPath={}", socket.display()));
    } else {
        // G4 (v0.1.3): when we are NOT attaching to an inspect-owned
        // ControlMaster socket, force `ControlMaster=no` so an
        // operator's personal `ControlMaster auto` in ~/.ssh/config
        // cannot promote this short-lived dispatch into a backgrounded
        // master that outlives the parent and detaches stdio.
        ssh.arg("-o").arg("ControlMaster=no");
    }
    if opts.tty {
        // -tt forces PTY allocation even when local
        // stdin is not a terminal (the runner spawns ssh with
        // Stdio::null/piped, never a tty). The PTY makes the remote
        // process line-buffer and propagates SIGINT through the tty
        // layer when the local ssh receives Ctrl-C.
        ssh.arg("-tt");
    }
    ssh.arg("-o").arg("BatchMode=yes").args(target.base_args());
    apply_extra_opts(&mut ssh);
    let stdin_bytes = opts.stdin.take();
    ssh.arg(&target.host)
        .arg("--")
        .arg(cmd)
        .stdin(if stdin_bytes.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let timeout = opts.timeout.unwrap_or_else(|| Duration::from_secs(30));
    let mut child = ssh
        .spawn()
        .with_context(|| format!("spawning '{SSH_BIN}'"))?;
    spawn_stdin_writer(&mut child, stdin_bytes);

    // Wait with a soft timeout. We can't easily kill ssh without breaking
    // multiplexed channel state, but we CAN observe and surface a useful
    // error rather than blocking forever.
    let start = std::time::Instant::now();
    loop {
        // Cancellation (audit §2.2): SIGINT/SIGTERM trips the global
        // flag; reap the child so we don't leak ssh processes.
        if crate::exec::cancel::is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!("cancelled by signal"));
        }
        if let Some(status) = child.try_wait().context("waiting on ssh")? {
            let mut stdout = String::new();
            let mut stderr = String::new();
            // Field pitfall §7.2: read raw bytes and lossily decode
            // them as UTF-8. The previous `read_to_string` would *fail*
            // the entire SSH read when the remote stream contained any
            // byte that wasn't valid UTF-8 (Latin-1, Shift-JIS, GBK,
            // a stray binary chunk in a log file, etc.). Operators saw
            // "stream did not contain valid UTF-8" instead of the
            // actual log line. Lossy decoding converts unknown bytes
            // to U+FFFD so the line is still readable; downstream
            // sanitization (`format::safe`) strips control bytes.
            if let Some(mut o) = child.stdout.take() {
                use std::io::Read;
                let mut buf = Vec::new();
                let _ = o.read_to_end(&mut buf);
                stdout = String::from_utf8_lossy(&buf).into_owned();
            }
            if let Some(mut e) = child.stderr.take() {
                use std::io::Read;
                let mut buf = Vec::new();
                let _ = e.read_to_end(&mut buf);
                stderr = String::from_utf8_lossy(&buf).into_owned();
            }
            let exit_code = status.code().unwrap_or(-1);
            // Audit §3.3: turn the cryptic OpenSSH "administratively
            // prohibited" into a one-line operator-friendly error so
            // the user knows to either lower fan-out or raise the
            // server's `MaxSessions`.
            if exit_code != 0 && super::concurrency::looks_like_max_sessions(&stderr) {
                return Err(anyhow!(
                    "SSH MaxSessions hit on '{}': server refused new channel \
                     (lower INSPECT_MAX_SESSIONS_PER_HOST below the server's \
                     MaxSessions, or raise the server's limit)",
                    target.host
                ));
            }
            // Promote ssh-transport failures (master gone,
            // unreachable, auth rejected) to `Err` so the dispatch
            // wrapper classifies and (for stale) auto-reauths.
            if let Some(e) = transport_err(exit_code, &stderr) {
                return Err(e);
            }
            return Ok(RemoteOutput {
                stdout,
                stderr,
                exit_code,
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

/// If `bytes` is `Some`, take the spawned child's stdin
/// handle and write the bytes from a background thread, then drop the
/// handle (which closes the pipe and signals EOF to the remote
/// command). Done off-thread so the caller can keep draining stdout
/// without deadlocking when the input is larger than the pipe buffer
/// (Linux: 64 KiB by default).
fn spawn_stdin_writer(child: &mut std::process::Child, bytes: Option<Vec<u8>>) {
    let Some(buf) = bytes else { return };
    let Some(mut stdin) = child.stdin.take() else {
        return;
    };
    std::thread::spawn(move || {
        use std::io::Write;
        // Best-effort: a remote that closes its stdin early (broken
        // pipe) is the partial-stdin case. Surfaced as a non-zero exit
        // by the remote command itself; we don't poison the run here.
        let _ = stdin.write_all(&buf);
        let _ = stdin.flush();
        drop(stdin);
    });
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

/// Outcome of one cancellation-poll iteration in
/// the streaming SSH executor.
#[derive(Debug, PartialEq, Eq)]
enum CancelAction {
    /// No new signal since the last poll; continue reading.
    None,
    /// First Ctrl-C since dispatch (or > 1s since the previous one):
    /// forward `\x03` through the SSH stdin pipe so the remote PTY's
    /// terminal driver delivers SIGINT to the remote process group.
    /// Then the streaming loop resets the global cancellation flag and
    /// continues reading — the remote is expected to exit on its own;
    /// we surface its real exit code instead of the conventional 130.
    ForwardIntr,
    /// Either the second Ctrl-C arrived within 1 second of the first
    /// (the operator wants to be sure the remote dies), or the dispatch
    /// has no PTY at all (no way to deliver SIGINT through the stdin
    /// pipe). Kill the local SSH child — the channel close triggers
    /// SIGHUP on the remote process group via the sshd-side PTY
    /// teardown — and return exit 130.
    Escalate,
}

/// Returns the right cancel action given:
/// - the current `cancel::signal_count()`
/// - the count we last handled (call site's `last_handled` cell)
/// - the `Instant` of the most recently forwarded `\x03` (call site's
///   `last_intr_at` cell)
/// - whether a PTY was allocated AND the stdin pipe is still alive
///
/// Updates `last_handled` and `last_intr_at` in place when it returns
/// `ForwardIntr`. Returns `Escalate` for the no-PTY / second-within-1s
/// cases. Returns `None` when no new signal has arrived.
fn classify_cancel(
    last_handled: &mut u32,
    last_intr_at: &mut Option<std::time::Instant>,
    have_pty: bool,
) -> CancelAction {
    let cur = crate::exec::cancel::signal_count();
    if cur <= *last_handled {
        return CancelAction::None;
    }
    let now = std::time::Instant::now();
    let recent = matches!(
        *last_intr_at,
        Some(t) if now.duration_since(t) < Duration::from_secs(1)
    );
    if !have_pty || recent {
        // Bump the handled marker so a third signal doesn't re-trigger.
        *last_handled = cur;
        CancelAction::Escalate
    } else {
        *last_intr_at = Some(now);
        *last_handled = cur;
        CancelAction::ForwardIntr
    }
}

/// Streaming variant of [`run_remote`]. Spawns ssh just
/// like the buffered runner, but pumps stdout line-by-line into
/// `on_line` so callers can render output as it arrives instead of
/// waiting for the remote command to exit.
///
/// Returns the remote process's exit code on completion. Honors
/// [`crate::exec::cancel::is_cancelled`]: when SIGINT/SIGTERM trips
/// the global flag, the child is killed and `Ok(130)` is returned
/// (the conventional "killed by SIGINT" exit code).
///
/// `opts.timeout` is treated as an *upper bound* on the lifetime of
/// the entire streaming call. Callers using `--follow` should pass a
/// generous timeout (hours) since the user is expected to Ctrl-C.
pub fn run_remote_streaming<F: FnMut(&str)>(
    namespace: &str,
    target: &SshTarget,
    cmd: &str,
    mut opts: RunOpts,
    mut on_line: F,
) -> Result<i32> {
    use std::io::{BufRead, BufReader, Read};
    use std::sync::{Arc, Mutex};
    use std::thread;

    let _session =
        super::concurrency::acquire(&target.host).context("acquiring SSH session slot")?;

    let socket = socket_path(namespace);
    let use_socket = match check_socket(&socket, target) {
        MasterStatus::Alive => true,
        MasterStatus::Stale | MasterStatus::Missing => {
            // See `run_remote` for rationale.
            return Err(anyhow!(
                "control socket connect({}): Connection refused (master gone)",
                socket.display()
            ));
        }
    };

    let mut ssh = Command::new(SSH_BIN);
    if use_socket {
        ssh.arg("-S")
            .arg(&socket)
            .arg("-o")
            .arg(format!("ControlPath={}", socket.display()));
    } else {
        // G4 (v0.1.3): force `ControlMaster=no` on the direct-ssh
        // path. See `run_remote` above for rationale.
        ssh.arg("-o").arg("ControlMaster=no");
    }
    if opts.tty {
        // -tt forces a PTY for the remote command. See
        // the same hook in `run_remote` above for rationale.
        ssh.arg("-tt");
    }
    ssh.arg("-o").arg("BatchMode=yes").args(target.base_args());
    apply_extra_opts(&mut ssh);
    let stdin_bytes = opts.stdin.take();
    // When PTY is allocated, keep stdin as a
    // pipe (instead of /dev/null) even when the caller has no payload
    // to forward. The streaming loop holds the write end so that on
    // the first SIGINT we can write `\x03` (ASCII ETX, the default
    // INTR character) into the remote PTY's terminal driver — which
    // delivers SIGINT to the remote process group. Without the pipe,
    // a single Ctrl-C just kills the local ssh, which channel-closes
    // the remote (sending SIGHUP) — coarser than necessary.
    let want_intr_pipe = opts.tty && stdin_bytes.is_none();
    ssh.arg(&target.host)
        .arg("--")
        .arg(cmd)
        .stdin(if stdin_bytes.is_some() || want_intr_pipe {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let timeout = opts.timeout.unwrap_or_else(|| Duration::from_secs(30));
    let mut child = ssh
        .spawn()
        .with_context(|| format!("spawning '{SSH_BIN}'"))?;
    let mut intr_pipe: Option<std::process::ChildStdin> = if want_intr_pipe {
        child.stdin.take()
    } else {
        None
    };
    spawn_stdin_writer(&mut child, stdin_bytes);

    // Drain stderr in a background thread so a chatty remote can't
    // block on its stderr pipe filling up. We capture it for the
    // MaxSessions diagnostic at exit time.
    let stderr_buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let stderr_handle = if let Some(mut e) = child.stderr.take() {
        let buf = Arc::clone(&stderr_buf);
        Some(thread::spawn(move || {
            let mut local = Vec::new();
            let _ = e.read_to_end(&mut local);
            if let Ok(mut g) = buf.lock() {
                g.extend_from_slice(&local);
            }
        }))
    } else {
        None
    };

    // Read stdout line-by-line. We use raw byte reads + lossy UTF-8
    // decode (mirroring the buffered path) so non-UTF-8 log bytes
    // don't poison the stream.
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("ssh: failed to capture stdout"))?;
    let mut reader = BufReader::new(stdout);
    let start = std::time::Instant::now();
    let mut line_bytes: Vec<u8> = Vec::with_capacity(4096);
    // Track signal-count progress so a NEW
    // Ctrl-C is detected even after we cleared the cancel flag from
    // a previous forwarded SIGINT.
    let mut last_handled_signal: u32 = crate::exec::cancel::signal_count();
    let mut last_intr_at: Option<std::time::Instant> = None;
    let have_pty = intr_pipe.is_some();

    let exit_code: i32 = loop {
        match classify_cancel(&mut last_handled_signal, &mut last_intr_at, have_pty) {
            CancelAction::None => {}
            CancelAction::ForwardIntr => {
                // First SIGINT: write \x03 into the remote PTY's
                // terminal driver. The remote terminal driver
                // recognises ETX as INTR and delivers SIGINT to the
                // remote process group. Reset our cancel flag so the
                // verb can surface the remote's real exit code rather
                // than 130 (matches the spec: "exit code is the
                // docker-logs exit code").
                if let Some(ref mut sh) = intr_pipe {
                    use std::io::Write;
                    let _ = sh.write_all(b"\x03");
                    let _ = sh.flush();
                }
                crate::exec::cancel::reset_cancel_flag();
            }
            CancelAction::Escalate => {
                // Either no PTY OR second-Ctrl-C-within-1s: drop the
                // intr pipe (closes stdin → remote sees EOF), then
                // kill the local ssh which channel-closes the remote
                // → sshd sends SIGHUP to the remote process group via
                // the PTY teardown.
                drop(intr_pipe.take());
                let _ = child.kill();
                let _ = child.wait();
                break 130;
            }
        }
        line_bytes.clear();
        match reader.read_until(b'\n', &mut line_bytes) {
            Ok(0) => {
                // EOF: drain the child.
                let status = child.wait().context("waiting on ssh")?;
                break status.code().unwrap_or(-1);
            }
            Ok(_) => {
                // Strip trailing CR/LF before lossy-decoding, so
                // `on_line` doesn't see them.
                while matches!(line_bytes.last(), Some(b'\n') | Some(b'\r')) {
                    line_bytes.pop();
                }
                let s = String::from_utf8_lossy(&line_bytes);
                on_line(&s);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                // Signal arrived during the syscall (SIGINT without
                // SA_RESTART). Loop back so classify_cancel notices.
                continue;
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(anyhow!("ssh stdout read failed: {e}"));
            }
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!(
                "remote stream timed out after {}s on '{}': {}",
                timeout.as_secs(),
                namespace,
                truncate(cmd, 120)
            ));
        }
    };

    if let Some(h) = stderr_handle {
        let _ = h.join();
    }

    if exit_code != 0 {
        let stderr = String::from_utf8_lossy(&stderr_buf.lock().unwrap()).into_owned();
        if super::concurrency::looks_like_max_sessions(&stderr) {
            return Err(anyhow!(
                "SSH MaxSessions hit on '{}': server refused new channel \
                 (lower INSPECT_MAX_SESSIONS_PER_HOST below the server's \
                 MaxSessions, or raise the server's limit)",
                target.host
            ));
        }
        // Promote transport failures so the dispatch wrapper
        // classifies and (on stale) auto-reauths. Genuine remote
        // command failures fall through to `Ok(exit_code)`.
        if let Some(e) = transport_err(exit_code, &stderr) {
            return Err(e);
        }
        // For genuine command failures (not
        // transport, not max-sessions), surface the captured stderr
        // to the operator. Pre-fix, this stderr was collected only
        // for the upstream classifications and then silently
        // discarded — so an agent driving `inspect run` saw
        // `arte: exit N` with no path to "what to fix" without a
        // side channel. `tee_eprintln!` also feeds the         // transcript so the diagnostic survives in post-mortems.
        // See `command_failure_stderr` doc-comment for the contract.
        if let Some(diag) = command_failure_stderr(exit_code, &stderr) {
            crate::tee_eprintln!("{diag}");
        }
    }

    Ok(exit_code)
}

/// B7 (v0.1.2) capturing streaming variant. Pumps remote stdout
/// line-by-line through `on_line` for live display **and** captures
/// every emitted line into the returned [`RemoteOutput`] so callers
/// (notably `inspect exec`, which writes the audit log) keep a
/// faithful record of what was shown to the operator.
///
/// stderr is collected into the returned `RemoteOutput.stderr` field,
/// not streamed: most failure-message detection (no-shell containers,
/// MaxSessions, SSH multiplexing errors) reads the whole stderr blob
/// at once after the child exits, and streaming it would interleave
/// noise into the operator's transcript. The MaxSessions diagnostic
/// is preserved.
pub fn run_remote_streaming_capturing<F: FnMut(&str)>(
    namespace: &str,
    target: &SshTarget,
    cmd: &str,
    mut opts: RunOpts,
    mut on_line: F,
) -> Result<RemoteOutput> {
    use std::io::{BufRead, BufReader, Read};
    use std::sync::{Arc, Mutex};
    use std::thread;

    let _session =
        super::concurrency::acquire(&target.host).context("acquiring SSH session slot")?;

    let socket = socket_path(namespace);
    let use_socket = match check_socket(&socket, target) {
        MasterStatus::Alive => true,
        MasterStatus::Stale | MasterStatus::Missing => {
            // See `run_remote` for rationale.
            return Err(anyhow!(
                "control socket connect({}): Connection refused (master gone)",
                socket.display()
            ));
        }
    };

    let mut ssh = Command::new(SSH_BIN);
    if use_socket {
        ssh.arg("-S")
            .arg(&socket)
            .arg("-o")
            .arg(format!("ControlPath={}", socket.display()));
    } else {
        // G4 (v0.1.3): force `ControlMaster=no` on the direct-ssh
        // path. See `run_remote` above for rationale.
        ssh.arg("-o").arg("ControlMaster=no");
    }
    if opts.tty {
        // -tt for streaming-capturing dispatches too,
        // matching `run_remote` and `run_remote_streaming`.
        ssh.arg("-tt");
    }
    ssh.arg("-o").arg("BatchMode=yes").args(target.base_args());
    apply_extra_opts(&mut ssh);
    let stdin_bytes = opts.stdin.take();
    let want_intr_pipe = opts.tty && stdin_bytes.is_none();
    ssh.arg(&target.host)
        .arg("--")
        .arg(cmd)
        .stdin(if stdin_bytes.is_some() || want_intr_pipe {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let timeout = opts.timeout.unwrap_or_else(|| Duration::from_secs(30));
    let mut child = ssh
        .spawn()
        .with_context(|| format!("spawning '{SSH_BIN}'"))?;
    let mut intr_pipe: Option<std::process::ChildStdin> = if want_intr_pipe {
        child.stdin.take()
    } else {
        None
    };
    spawn_stdin_writer(&mut child, stdin_bytes);

    let stderr_buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let stderr_handle = if let Some(mut e) = child.stderr.take() {
        let buf = Arc::clone(&stderr_buf);
        Some(thread::spawn(move || {
            let mut local = Vec::new();
            let _ = e.read_to_end(&mut local);
            if let Ok(mut g) = buf.lock() {
                g.extend_from_slice(&local);
            }
        }))
    } else {
        None
    };

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("ssh: failed to capture stdout"))?;
    let mut reader = BufReader::new(stdout);
    let start = std::time::Instant::now();
    let mut line_bytes: Vec<u8> = Vec::with_capacity(4096);
    let mut captured_stdout = String::new();
    let mut last_handled_signal: u32 = crate::exec::cancel::signal_count();
    let mut last_intr_at: Option<std::time::Instant> = None;
    let have_pty = intr_pipe.is_some();

    let exit_code: i32 = loop {
        match classify_cancel(&mut last_handled_signal, &mut last_intr_at, have_pty) {
            CancelAction::None => {}
            CancelAction::ForwardIntr => {
                if let Some(ref mut sh) = intr_pipe {
                    use std::io::Write;
                    let _ = sh.write_all(b"\x03");
                    let _ = sh.flush();
                }
                crate::exec::cancel::reset_cancel_flag();
            }
            CancelAction::Escalate => {
                drop(intr_pipe.take());
                let _ = child.kill();
                let _ = child.wait();
                break 130;
            }
        }
        line_bytes.clear();
        match reader.read_until(b'\n', &mut line_bytes) {
            Ok(0) => {
                let status = child.wait().context("waiting on ssh")?;
                break status.code().unwrap_or(-1);
            }
            Ok(_) => {
                while matches!(line_bytes.last(), Some(b'\n') | Some(b'\r')) {
                    line_bytes.pop();
                }
                let s = String::from_utf8_lossy(&line_bytes);
                captured_stdout.push_str(&s);
                captured_stdout.push('\n');
                on_line(&s);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(anyhow!("ssh stdout read failed: {e}"));
            }
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!(
                "remote stream timed out after {}s on '{}': {}",
                timeout.as_secs(),
                namespace,
                truncate(cmd, 120)
            ));
        }
    };

    if let Some(h) = stderr_handle {
        let _ = h.join();
    }
    let stderr = String::from_utf8_lossy(&stderr_buf.lock().unwrap()).into_owned();

    if exit_code != 0 && super::concurrency::looks_like_max_sessions(&stderr) {
        return Err(anyhow!(
            "SSH MaxSessions hit on '{}': server refused new channel \
             (lower INSPECT_MAX_SESSIONS_PER_HOST below the server's \
             MaxSessions, or raise the server's limit)",
            target.host
        ));
    }
    // Promote ssh-transport failures so the dispatch wrapper
    // classifies and (for stale) auto-reauths. Genuine remote command
    // failures fall through to `Ok(RemoteOutput { exit_code, .. })`.
    if let Some(e) = transport_err(exit_code, &stderr) {
        return Err(e);
    }

    Ok(RemoteOutput {
        stdout: captured_stdout,
        stderr,
        exit_code,
    })
}

#[cfg(test)]
mod p8d_tests {
    use super::command_failure_stderr;

    #[test]
    fn p8d_no_emit_on_zero_exit() {
        // Stderr-on-success is just chatter (warnings, deprecation
        // notices, progress). Don't surface it — the operator chose
        // not to use --stream / --show-output.
        assert_eq!(command_failure_stderr(0, "warning: chatty success\n"), None);
    }

    #[test]
    fn p8d_no_emit_on_empty_stderr() {
        // No diagnostic to surface; the verb's own
        // `{label}: exit {code}` line is the only signal.
        assert_eq!(command_failure_stderr(2, ""), None);
        assert_eq!(command_failure_stderr(2, "   \n"), None);
        assert_eq!(command_failure_stderr(127, "\n\n"), None);
    }

    #[test]
    fn p8d_emit_on_real_command_failure() {
        // The exact shape the user's smoke hit: docker exit 2 with
        // a daemon error message that pre-fix was silently dropped.
        let stderr = "docker: Error response from daemon: image not found.\n";
        let out = command_failure_stderr(2, stderr);
        assert_eq!(
            out,
            Some("docker: Error response from daemon: image not found.")
        );
    }

    #[test]
    fn p8d_emit_strips_trailing_whitespace() {
        // `tee_eprintln!` appends a newline; emitting the trimmed
        // form avoids a double-newline when stderr ends with `\n`.
        assert_eq!(command_failure_stderr(1, "boom\n"), Some("boom"));
        assert_eq!(command_failure_stderr(1, "boom\r\n\n"), Some("boom"));
    }

    #[test]
    fn p8d_no_emit_on_max_sessions_stderr() {
        // The MaxSessions branch in `run_remote_streaming` already
        // surfaces a typed Err with operator-friendly wording. Don't
        // double-print here.
        let stderr =
            "channel 0: open failed: administratively prohibited: open failed (MaxSessions)";
        assert_eq!(command_failure_stderr(255, stderr), None);
    }

    #[test]
    fn p8d_no_emit_on_transport_class_stderr() {
        // Transport-classified stderr is wrapped into an anyhow Err
        // by `transport_err` and surfaced via the verb's error path.
        // Use the synthetic test marker the mock runner already
        // emits — keeps the test independent of OpenSSH wording.
        assert_eq!(command_failure_stderr(255, "transport:stale"), None);
        assert_eq!(command_failure_stderr(255, "transport:unreachable"), None);
        assert_eq!(command_failure_stderr(255, "transport:auth_failed"), None);
    }

    #[test]
    fn p8d_emit_on_command_failure_that_mentions_innocuous_keywords() {
        // Defense-in-depth: a remote command's *legitimate* stderr
        // might contain words that look transport-y but aren't (e.g.
        // a user-authored command that prints "permission denied:
        // /tmp/foo" from a chmod failure). The classifiers anchor on
        // ssh-specific phrasings, so we should still emit.
        let stderr = "chmod: cannot access '/etc/foo': Permission denied";
        // Verify our classifiers don't false-positive on this:
        assert!(!super::super::concurrency::looks_like_max_sessions(stderr));
        assert!(super::super::transport::classify(stderr).is_none());
        // And therefore we DO emit it as a command-failure diagnostic.
        assert_eq!(command_failure_stderr(1, stderr), Some(stderr));
    }
}
