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
    /// - `-o ForwardAgent=no -o ForwardX11=no` — defense-in-depth
    ///   against an operator's personal `ForwardAgent yes` /
    ///   `ForwardX11 yes` in `~/.ssh/config` exposing the agent or
    ///   X11 socket to the remote (CVE-2023-38408 family). Inspect
    ///   never needs either form of forwarding for its own work.
    /// - `-o BatchMode=yes` is **not** set here; that's only used in the
    ///   "check existing user mux" probe, since we want password/agent
    ///   prompts to work for actual connection attempts.
    pub fn base_args(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = Vec::with_capacity(12);
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
        // Force-disable agent and X11
        // forwarding so a stray `ForwardAgent yes` in the operator's
        // ssh_config cannot expose the local ssh-agent to the remote.
        args.push(OsString::from("-o"));
        args.push(OsString::from("ForwardAgent=no"));
        args.push(OsString::from("-o"));
        args.push(OsString::from("ForwardX11=no"));
        args
    }
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = crate::paths::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(key: Option<&str>) -> SshTarget {
        SshTarget {
            host: "example".into(),
            user: "ops".into(),
            port: 22,
            key_path: key.map(PathBuf::from),
        }
    }

    fn args_as_strings(t: &SshTarget) -> Vec<String> {
        t.base_args()
            .into_iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn s1_base_args_disable_agent_and_x11_forwarding() {
        let strings = args_as_strings(&target(None));
        // ForwardAgent=no and ForwardX11=no must always be present so
        // an operator's personal `ForwardAgent yes` in ~/.ssh/config
        // cannot leak the local ssh-agent to a target server
        // (CVE-2023-38408 family).
        assert!(
            strings.iter().any(|a| a == "ForwardAgent=no"),
            "ForwardAgent=no missing from base_args: {strings:?}"
        );
        assert!(
            strings.iter().any(|a| a == "ForwardX11=no"),
            "ForwardX11=no missing from base_args: {strings:?}"
        );
    }

    #[test]
    fn s1_forwarding_off_with_explicit_key() {
        let strings = args_as_strings(&target(Some("/tmp/k")));
        assert!(strings.iter().any(|a| a == "ForwardAgent=no"));
        assert!(strings.iter().any(|a| a == "IdentitiesOnly=yes"));
    }
}
