//! Fast pre-flight SSH connectivity check (B1, v0.1.2).
//!
//! Runs `ssh -o BatchMode=yes -o ConnectTimeout=10 <target> true` and
//! classifies the failure mode from stderr. The goal is to fail fast,
//! once, with a single chained hint instead of letting the user see a
//! pile of swallowed `Permission denied (publickey)` warnings inside
//! the discovery output.
//!
//! When an existing ControlMaster socket is alive this call is a near
//! no-op (~50ms) since `BatchMode` does not interfere with the mux.

use std::process::Command;

use crate::ssh::SshTarget;

/// Reason a precheck failed. Drives which chained hint we emit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrecheckOutcome {
    /// SSH succeeded — proceed with discovery.
    Ok,
    /// `Permission denied (publickey,...)` or agent/identity refusal.
    /// The session probably expired or the agent is empty.
    AuthFailed { stderr: String },
    /// Host key changed / host key verification failed.
    HostKeyChanged { stderr: String },
    /// `Connection refused`, `No route to host`, or `Connection timed out`.
    Unreachable { stderr: String },
    /// Anything else (DNS failure, ssh exit != 255, etc.).
    Other { stderr: String, exit_code: i32 },
}

/// Run the precheck. We always set `BatchMode=yes` so a missing agent
/// fails fast instead of prompting at the controlling tty.
///
/// `ConnectTimeout=10` matches the rest of the codebase. We set a small
/// process-level timeout via `wait_timeout`-style polling? No — ssh's
/// own `ConnectTimeout` is enough; if ssh itself wedges (very rare), we
/// give it a hard `Duration` budget below.
pub fn run(target: &SshTarget) -> PrecheckOutcome {
    let connect_timeout = std::env::var("INSPECT_SSH_CONNECT_TIMEOUT")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(10);

    let mut cmd = Command::new("ssh");
    cmd.arg("-o").arg("BatchMode=yes");
    cmd.arg("-o")
        .arg(format!("ConnectTimeout={connect_timeout}"));
    cmd.args(target.base_args());
    // Honor operator-supplied raw ssh args (ProxyJump, UserKnownHostsFile,
    // ...). Same env var the master/exec layers use.
    if let Ok(extra) = std::env::var("INSPECT_SSH_EXTRA_OPTS") {
        for tok in extra.split_whitespace() {
            cmd.arg(tok);
        }
    }
    cmd.arg(&target.host);
    cmd.arg("true");

    // Inherit stdin from null implicitly; capture stdout/stderr.
    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => {
            return PrecheckOutcome::Other {
                stderr: format!("failed to spawn ssh: {e}"),
                exit_code: -1,
            };
        }
    };

    if output.status.success() {
        return PrecheckOutcome::Ok;
    }

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let code = output.status.code().unwrap_or(-1);
    classify(&stderr, code)
}

/// Inspect ssh stderr and classify the failure. Patterns are matched in
/// most-specific-first order.
fn classify(stderr: &str, exit_code: i32) -> PrecheckOutcome {
    let lower = stderr.to_ascii_lowercase();

    // Host-key changes are a security-sensitive case; check first so we
    // don't swallow them under the generic "auth failed" bucket.
    if lower.contains("host key verification failed")
        || lower.contains("remote host identification has changed")
        || lower.contains("offending")
    {
        return PrecheckOutcome::HostKeyChanged {
            stderr: stderr.to_string(),
        };
    }

    if lower.contains("permission denied")
        || lower.contains("could not open a connection to your authentication agent")
        || lower.contains("no such identity")
        || lower.contains("too many authentication failures")
    {
        return PrecheckOutcome::AuthFailed {
            stderr: stderr.to_string(),
        };
    }

    if lower.contains("connection refused")
        || lower.contains("no route to host")
        || lower.contains("connection timed out")
        || lower.contains("network is unreachable")
        || lower.contains("could not resolve hostname")
        || lower.contains("name or service not known")
    {
        return PrecheckOutcome::Unreachable {
            stderr: stderr.to_string(),
        };
    }

    PrecheckOutcome::Other {
        stderr: stderr.to_string(),
        exit_code,
    }
}

/// Build the chained multi-line hint for an auth failure (B1 spec).
///
/// We deliberately append `→ run:` lines instead of trying to recover
/// automatically: each step has a security implication (does the user
/// want to load *this* key? does the user want a new mux?) and asking
/// the human is the right call.
pub fn auth_failed_hint(namespace: &str, target: &SshTarget) -> String {
    let key_hint = match &target.key_path {
        Some(p) => format!("ssh-add {}", p.display()),
        None => "ssh-add <your-key>".to_string(),
    };
    format!(
        "SSH auth failed for {ns}. Your session may have expired.\n  \
         → run: {key_hint}\n  \
         → run: inspect connect {ns}\n  \
         → then retry: inspect setup {ns}",
        ns = namespace,
        key_hint = key_hint,
    )
}

/// Build a hint for a host-key change. Security-sensitive: never auto-fix.
pub fn host_key_changed_hint(namespace: &str, target: &SshTarget) -> String {
    format!(
        "SSH host key for {ns} ({host}) has changed. This may be a legitimate \
         re-provisioning OR a man-in-the-middle attempt.\n  \
         → verify: confirm the new fingerprint with the host operator out-of-band\n  \
         → if legitimate: ssh-keygen -R {host}\n  \
         → then retry: inspect setup {ns}",
        ns = namespace,
        host = target.host,
    )
}

/// Build a hint for unreachable hosts.
pub fn unreachable_hint(namespace: &str, target: &SshTarget) -> String {
    format!(
        "SSH could not reach {host}:{port} for {ns}.\n  \
         → check: ping {host}\n  \
         → check: VPN / firewall / DNS\n  \
         → then retry: inspect setup {ns}",
        ns = namespace,
        host = target.host,
        port = target.port,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t() -> SshTarget {
        SshTarget {
            host: "h".into(),
            user: "u".into(),
            port: 22,
            key_path: None,
        }
    }

    #[test]
    fn classify_publickey_denied_is_auth_failed() {
        let s = "u@h: Permission denied (publickey).";
        assert!(matches!(
            classify(s, 255),
            PrecheckOutcome::AuthFailed { .. }
        ));
    }

    #[test]
    fn classify_dead_agent_is_auth_failed() {
        let s = "Could not open a connection to your authentication agent.\n\
                 u@h: Permission denied (publickey).";
        assert!(matches!(
            classify(s, 255),
            PrecheckOutcome::AuthFailed { .. }
        ));
    }

    #[test]
    fn classify_host_key_change_wins_over_auth() {
        // Some sshd versions print both; host-key change must dominate.
        let s = "@@@@@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @@@@@\n\
                 Host key verification failed.\n\
                 Permission denied (publickey).";
        assert!(matches!(
            classify(s, 255),
            PrecheckOutcome::HostKeyChanged { .. }
        ));
    }

    #[test]
    fn classify_connection_refused_is_unreachable() {
        let s = "ssh: connect to host h port 22: Connection refused";
        assert!(matches!(
            classify(s, 255),
            PrecheckOutcome::Unreachable { .. }
        ));
    }

    #[test]
    fn classify_dns_failure_is_unreachable() {
        let s = "ssh: Could not resolve hostname h: Name or service not known";
        assert!(matches!(
            classify(s, 255),
            PrecheckOutcome::Unreachable { .. }
        ));
    }

    #[test]
    fn classify_unknown_is_other() {
        let s = "kex_exchange_identification: read: Connection reset by peer";
        match classify(s, 255) {
            PrecheckOutcome::Other { exit_code, .. } => assert_eq!(exit_code, 255),
            x => panic!("expected Other, got {x:?}"),
        }
    }

    #[test]
    fn auth_hint_includes_namespace_and_key() {
        let mut tgt = t();
        tgt.key_path = Some("/home/u/.ssh/id_ed25519".into());
        let h = auth_failed_hint("arte", &tgt);
        assert!(h.starts_with("SSH auth failed for arte."));
        assert!(h.contains("ssh-add /home/u/.ssh/id_ed25519"));
        assert!(h.contains("inspect connect arte"));
        assert!(h.contains("inspect setup arte"));
    }

    #[test]
    fn auth_hint_without_key_uses_placeholder() {
        let h = auth_failed_hint("arte", &t());
        assert!(h.contains("ssh-add <your-key>"));
    }

    #[test]
    fn host_key_hint_mentions_keygen_r() {
        let h = host_key_changed_hint("arte", &t());
        assert!(h.contains("ssh-keygen -R h"));
        assert!(h.contains("man-in-the-middle"));
    }

    #[test]
    fn unreachable_hint_mentions_host_and_port() {
        let h = unreachable_hint("arte", &t());
        assert!(h.contains("h:22"));
        assert!(h.contains("ping h"));
    }
}
