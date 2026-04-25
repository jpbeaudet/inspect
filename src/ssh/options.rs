//! Build SSH command-line arguments from a resolved namespace.
//!
//! We delegate every security-relevant behavior — host-key verification,
//! `known_hosts`, ssh-agent, ProxyJump, algorithms — to the `ssh` binary
//! itself. The CLI builder here only constructs the argument vector.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::config::ResolvedNamespace;

/// A resolved SSH target: the values that actually go into an `ssh` invocation.
#[derive(Debug, Clone)]
pub struct SshTarget {
    pub host: String,
    pub user: String,
    pub port: u16,
    pub key_path: Option<PathBuf>,
}

impl SshTarget {
    pub fn from_resolved(r: &ResolvedNamespace) -> anyhow::Result<Self> {
        let cfg = &r.config;
        let host = cfg
            .host
            .clone()
            .ok_or_else(|| anyhow::anyhow!("namespace '{}' has no host", r.name))?;
        let user = cfg
            .user
            .clone()
            .ok_or_else(|| anyhow::anyhow!("namespace '{}' has no user", r.name))?;
        let port = cfg.port.unwrap_or(22);
        let key_path = cfg.key_path.as_deref().map(expand_tilde);
        Ok(Self {
            host,
            user,
            port,
            key_path,
        })
    }

    /// Common `ssh` arguments used by every invocation:
    ///
    /// - `-p <port>`
    /// - `-l <user>`
    /// - `-i <key_path>` if configured
    /// - `-o BatchMode=yes` is **not** set here; that's only used in the
    ///   "check existing user mux" probe, since we want password/agent
    ///   prompts to work for actual connection attempts.
    pub fn base_args(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = Vec::with_capacity(8);
        args.push(OsString::from("-p"));
        args.push(OsString::from(self.port.to_string()));
        args.push(OsString::from("-l"));
        args.push(OsString::from(&self.user));
        if let Some(key) = &self.key_path {
            args.push(OsString::from("-i"));
            args.push(OsString::from(key));
            // If a key is provided, prefer it explicitly: don't fall through
            // to all agent identities first.
            args.push(OsString::from("-o"));
            args.push(OsString::from("IdentitiesOnly=yes"));
        }
        args
    }
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}
