//! Remote command runner abstraction. In production it shells out via
//! [`crate::ssh::exec::run_remote`]; in tests it can be replaced by a
//! file-based mock through `INSPECT_MOCK_REMOTE_FILE`.
//!
//! The mock file is JSON, e.g.:
//! ```json
//! [
//!   { "match": "docker ps", "stdout": "...", "exit": 0 },
//!   { "match": "cat /etc/x", "stderr": "no such file", "exit": 1 }
//! ]
//! ```
//! First entry whose `match` substring appears in the command wins.

use std::path::PathBuf;

use anyhow::Result;
use serde::Deserialize;

use crate::config::namespace::ResolvedNamespace;
use crate::ssh::exec::{
    run_remote, run_remote_streaming, run_remote_streaming_capturing, RemoteOutput, RunOpts,
};
use crate::ssh::options::SshTarget;

/// Trait every verb uses to talk to a remote host. Lets the test suite
/// swap in deterministic, offline behavior.
pub trait RemoteRunner: Send + Sync {
    fn run(
        &self,
        namespace: &str,
        target: &SshTarget,
        cmd: &str,
        opts: RunOpts,
    ) -> Result<RemoteOutput>;

    /// Streaming variant (P1, v0.1.1). Default implementation buffers
    /// via [`Self::run`] and delivers lines after the command exits —
    /// correct for mocks and replay-based tests. Live runners should
    /// override to pump output as it arrives.
    ///
    /// Returns the remote exit code. `on_line` receives each output
    /// line (newline-stripped).
    fn run_streaming(
        &self,
        namespace: &str,
        target: &SshTarget,
        cmd: &str,
        opts: RunOpts,
        on_line: &mut dyn FnMut(&str),
    ) -> Result<i32> {
        let out = self.run(namespace, target, cmd, opts)?;
        for line in out.stdout.lines() {
            on_line(line);
        }
        Ok(out.exit_code)
    }

    /// B7 (v0.1.2): streaming variant that **also** captures every
    /// emitted stdout line into the returned [`RemoteOutput`]. Used by
    /// `inspect exec` so the operator gets live progress *and* the
    /// audit log gets a faithful record of what was shown. Default
    /// implementation buffers via [`Self::run`] and replays lines —
    /// correct for mocks and tests.
    fn run_streaming_capturing(
        &self,
        namespace: &str,
        target: &SshTarget,
        cmd: &str,
        opts: RunOpts,
        on_line: &mut dyn FnMut(&str),
    ) -> Result<RemoteOutput> {
        let out = self.run(namespace, target, cmd, opts)?;
        for line in out.stdout.lines() {
            on_line(line);
        }
        Ok(out)
    }
}

/// Production runner: real ssh through the inspect master socket.
pub struct LiveRunner;

impl RemoteRunner for LiveRunner {
    fn run(
        &self,
        namespace: &str,
        target: &SshTarget,
        cmd: &str,
        opts: RunOpts,
    ) -> Result<RemoteOutput> {
        run_remote(namespace, target, cmd, opts)
    }

    fn run_streaming(
        &self,
        namespace: &str,
        target: &SshTarget,
        cmd: &str,
        opts: RunOpts,
        on_line: &mut dyn FnMut(&str),
    ) -> Result<i32> {
        run_remote_streaming(namespace, target, cmd, opts, on_line)
    }

    fn run_streaming_capturing(
        &self,
        namespace: &str,
        target: &SshTarget,
        cmd: &str,
        opts: RunOpts,
        on_line: &mut dyn FnMut(&str),
    ) -> Result<RemoteOutput> {
        run_remote_streaming_capturing(namespace, target, cmd, opts, on_line)
    }
}

#[derive(Debug, Deserialize, Clone)]
struct MockEntry {
    #[serde(rename = "match")]
    match_: String,
    #[serde(default)]
    stdout: String,
    #[serde(default)]
    stderr: String,
    #[serde(default)]
    exit: i32,
    /// F9 (v0.1.3): when `true` and the caller forwarded stdin via
    /// `RunOpts.stdin`, the mock prepends the (lossily UTF-8 decoded)
    /// stdin to its `stdout`. Lets tests assert that bytes really
    /// crossed the runner boundary without needing a live ssh.
    #[serde(default)]
    echo_stdin: bool,
}

pub struct MockRunner {
    entries: Vec<MockEntry>,
}

impl MockRunner {
    pub fn from_file(path: &std::path::Path) -> Result<Self> {
        let body = std::fs::read_to_string(path)?;
        let entries: Vec<MockEntry> = serde_json::from_str(&body)?;
        Ok(Self { entries })
    }
}

impl RemoteRunner for MockRunner {
    fn run(
        &self,
        _namespace: &str,
        _target: &SshTarget,
        cmd: &str,
        opts: RunOpts,
    ) -> Result<RemoteOutput> {
        for e in &self.entries {
            if cmd.contains(&e.match_) {
                let mut stdout = e.stdout.clone();
                if e.echo_stdin {
                    if let Some(bytes) = opts.stdin.as_ref() {
                        let prefix = String::from_utf8_lossy(bytes);
                        stdout = format!("{prefix}{stdout}");
                    }
                }
                return Ok(RemoteOutput {
                    stdout,
                    stderr: e.stderr.clone(),
                    exit_code: e.exit,
                });
            }
        }
        Ok(RemoteOutput {
            stdout: String::new(),
            stderr: format!("(mock) no match for command: {cmd}"),
            exit_code: 127,
        })
    }
}

fn mock_path() -> Option<PathBuf> {
    let p = std::env::var("INSPECT_MOCK_REMOTE_FILE").ok()?;
    let p = PathBuf::from(p);
    if p.exists() {
        Some(p)
    } else {
        None
    }
}

/// Pick the runner based on environment. Returned as a fresh `Box` each
/// call; verb code can pass a `&dyn RemoteRunner` down without owning it.
pub fn current_runner() -> Box<dyn RemoteRunner> {
    if let Some(p) = mock_path() {
        match MockRunner::from_file(&p) {
            Ok(m) => return Box::new(m),
            Err(e) => eprintln!("warning: mock file '{}' unreadable: {e}", p.display()),
        }
    }
    Box::new(LiveRunner)
}

/// Resolve a namespace and turn it into an SshTarget. Cached per process
/// to avoid re-reading TOML on every fanout step.
pub fn resolve_target(namespace: &str) -> Result<(ResolvedNamespace, SshTarget)> {
    use crate::config::resolver as ns_resolver;
    let ns = ns_resolver::resolve(namespace)?;
    // F12 (v0.1.3): the resolver itself does not validate (so callers
    // can introspect partial configs); every dispatch site DOES need
    // validation, including the new env-overlay key check, so do it
    // here at the single boundary every verb crosses.
    ns.config.validate(&ns.name)?;
    let target = SshTarget::from_resolved(&ns)?;
    Ok((ns, target))
}
