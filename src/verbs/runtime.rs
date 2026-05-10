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
use std::sync::Mutex;

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

    /// Streaming variant. Default implementation buffers
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

    /// Re-establish the persistent master socket for
    /// `namespace`. Called by the dispatch wrapper when a transport-
    /// stale failure is detected and the verb's caller has not opted
    /// out via `--no-reauth` / `auto_reauth = false`. Live runner
    /// shells to the same code path that `inspect connect <ns>` uses;
    /// the test mock controls success/failure via the
    /// `INSPECT_MOCK_REAUTH` env var (`ok` | `fail`, default `ok`).
    /// Returning `Ok(())` means the master socket is up and the
    /// caller should retry the original verb exactly once.
    fn reauth(&self, namespace: &str, target: &SshTarget) -> Result<()> {
        let _ = (namespace, target);
        Ok(())
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

    fn reauth(&self, namespace: &str, _target: &SshTarget) -> Result<()> {
        // Delegate to the same code path that interactive
        // `inspect connect <ns>` uses. Honors askpass / agent semantics
        // so the operator gets the same passphrase prompt path as a
        // first-time connect. Non-tty + no-agent failure surfaces as
        // a plain Err which the dispatch wrapper translates into
        // `Transport::AuthFailed` for exit-code routing.
        crate::commands::connect::reauth_namespace(namespace)
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
    /// When `true` and the caller forwarded stdin via
    /// `RunOpts.stdin`, the mock prepends the (lossily UTF-8 decoded)
    /// stdin to its `stdout`. Lets tests assert that bytes really
    /// crossed the runner boundary without needing a live ssh.
    #[serde(default)]
    echo_stdin: bool,
    /// When set, the mock returns `Err(anyhow!("transport:<class>"))`
    /// from `run()` instead of an `Ok(RemoteOutput)`. Lets the     /// acceptance suite drive the dispatch-wrapper's reauth + retry
    /// path without a live ssh. Recognized values: `"stale"`,
    /// `"unreachable"`, `"auth_failed"`.
    #[serde(default)]
    transport_class: Option<String>,
    /// Consume this entry at most `max_uses` times.
    /// Once exhausted, the entry is skipped on subsequent matches so
    /// a follow-up entry (e.g. a successful retry after reauth) can
    /// take over. `None` means infinite reuse (today's behavior).
    #[serde(default)]
    max_uses: Option<u32>,
}

pub struct MockRunner {
    entries: Vec<MockEntry>,
    /// Per-entry consumption counter for `max_uses`.
    /// Indexed by `entries` position; 0 means "never matched yet".
    use_counts: Mutex<Vec<u32>>,
    /// How many reauth invocations the mock has served.
    /// Lets tests assert that auto-reauth fired exactly once (the
    /// contract is one retry per verb invocation).
    reauth_count: Mutex<u32>,
}

impl MockRunner {
    pub fn from_file(path: &std::path::Path) -> Result<Self> {
        let body = std::fs::read_to_string(path)?;
        let entries: Vec<MockEntry> = serde_json::from_str(&body)?;
        let n = entries.len();
        Ok(Self {
            entries,
            use_counts: Mutex::new(vec![0u32; n]),
            reauth_count: Mutex::new(0),
        })
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
        // Scan in declaration order; respect `max_uses`
        // so a stale-marker entry can be consumed once and the next
        // matching entry takes over on the post-reauth retry.
        let mut counts = self.use_counts.lock().unwrap();
        for (i, e) in self.entries.iter().enumerate() {
            if !cmd.contains(&e.match_) {
                continue;
            }
            if let Some(cap) = e.max_uses {
                if counts[i] >= cap {
                    continue;
                }
            }
            counts[i] += 1;
            // Synthetic transport failure: mock signals to the
            // dispatch wrapper that this should classify as a
            // transport bucket (stale / unreachable / auth_failed).
            if let Some(cls) = e.transport_class.as_deref() {
                let stderr = if e.stderr.is_empty() {
                    format!("transport:{cls}")
                } else {
                    format!("transport:{cls}\n{}", e.stderr)
                };
                return Err(anyhow::anyhow!(stderr));
            }
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
        Ok(RemoteOutput {
            stdout: String::new(),
            stderr: format!("(mock) no match for command: {cmd}"),
            exit_code: 127,
        })
    }

    fn reauth(&self, _namespace: &str, _target: &SshTarget) -> Result<()> {
        // Test-side reauth simulation. `INSPECT_MOCK_REAUTH`
        // selects success (`ok`, default) vs failure (`fail`). Counter
        // is bumped on every call so tests can assert reauth fired
        // exactly once per verb invocation.
        *self.reauth_count.lock().unwrap() += 1;
        match std::env::var("INSPECT_MOCK_REAUTH").as_deref() {
            Ok("fail") => Err(anyhow::anyhow!(
                "mock reauth failed (INSPECT_MOCK_REAUTH=fail)"
            )),
            _ => Ok(()),
        }
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
            Err(e) => crate::tee_eprintln!("warning: mock file '{}' unreadable: {e}", p.display()),
        }
    }
    Box::new(LiveRunner)
}

/// Resolve a namespace and turn it into an SshTarget. Cached per process
/// to avoid re-reading TOML on every fanout step.
pub fn resolve_target(namespace: &str) -> Result<(ResolvedNamespace, SshTarget)> {
    use crate::config::resolver as ns_resolver;
    let ns = ns_resolver::resolve(namespace)?;
    // The resolver itself does not validate (so callers
    // can introspect partial configs); every dispatch site DOES need
    // validation, including the new env-overlay key check, so do it
    // here at the single boundary every verb crosses.
    ns.config.validate(&ns.name)?;
    let target = SshTarget::from_resolved(&ns)?;
    // Every verb that resolves a namespace crosses
    // this function exactly once. Stamp the transcript context with
    // the resolved name + per-ns transcript policy here so all
    // subsequent emit calls flow into the right per-namespace file.
    crate::transcript::set_namespace(&ns.name);
    Ok((ns, target))
}
