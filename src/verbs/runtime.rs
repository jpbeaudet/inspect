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
use crate::ssh::exec::{run_remote, RemoteOutput, RunOpts};
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
        _opts: RunOpts,
    ) -> Result<RemoteOutput> {
        for e in &self.entries {
            if cmd.contains(&e.match_) {
                return Ok(RemoteOutput {
                    stdout: e.stdout.clone(),
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
    let target = SshTarget::from_resolved(&ns)?;
    Ok((ns, target))
}
