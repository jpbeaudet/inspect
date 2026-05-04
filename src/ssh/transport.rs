//! F13 (v0.1.3): SSH transport-failure classifier.
//!
//! Distinguishes four buckets at the SSH-executor boundary:
//!
//! - `Stale` — master socket gone / `ControlPersist` expired / "broken
//!   pipe" / "connection closed by remote host" before any command
//!   bytes were read. Auto-reauth retries this once.
//! - `Unreachable` — host genuinely unreachable (DNS fail, connection
//!   refused, network down). Re-auth does not help.
//! - `AuthFailed` — re-auth attempt itself failed (wrong passphrase,
//!   key revoked). Never silently retried.
//! - (None) — caller saw a remote command failure (`Command::Failed`),
//!   not a transport failure. Today's behavior unchanged.
//!
//! The classifier is a pure function over a stderr / error-message
//! string so every dispatch site can use the same rule. It also
//! recognizes the synthetic `transport:<class>` prefix that the test
//! mock emits, so acceptance tests can drive the reauth path
//! deterministically without a live ssh.
//!
//! See `docs/RUNBOOK.md` §10 for the dispatch contract.

use serde::Serialize;
/// Transport-level failure class. Maps directly to a dedicated exit
/// code and a `failure_class` JSON field on the verb response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::enum_variant_names)]
pub enum TransportClass {
    /// Master socket / `ControlPersist` expired between verb
    /// invocations. Re-auth + retry once.
    TransportStale,
    /// Host genuinely unreachable (DNS, route, refused). No retry.
    TransportUnreachable,
    /// Auth attempt (initial or re-auth) failed. Never retried.
    TransportAuthFailed,
}

impl TransportClass {
    /// JSON `failure_class` field value.
    pub fn as_str(self) -> &'static str {
        match self {
            TransportClass::TransportStale => "transport_stale",
            TransportClass::TransportUnreachable => "transport_unreachable",
            TransportClass::TransportAuthFailed => "transport_auth_failed",
        }
    }

    /// Dedicated process exit code:
    /// - 12 = stale (master/socket expired)
    /// - 13 = unreachable (DNS / refused / no route)
    /// - 14 = auth-failed (wrong passphrase, key revoked, etc.)
    ///
    /// All three are out of the way of conventional `1` / `2` /
    /// `126` / `127` and do not collide with remote command exit
    /// codes (which pass through unchanged for `Command::Failed`).
    pub fn exit_code(self) -> u8 {
        match self {
            TransportClass::TransportStale => 12,
            TransportClass::TransportUnreachable => 13,
            TransportClass::TransportAuthFailed => 14,
        }
    }

    /// One-line SUMMARY trailer suffix inserted after the per-verb
    /// `N ok, M failed` text. Includes a chained operator hint so
    /// shell wrappers and human readers both see the recovery path.
    pub fn summary_hint(self, ns: &str) -> String {
        match self {
            TransportClass::TransportStale => format!(
                "ssh_error: stale connection — run 'inspect disconnect {ns} && inspect connect {ns}' or pass --reauth"
            ),
            TransportClass::TransportUnreachable => format!(
                "ssh_error: unreachable — run 'inspect connectivity {ns}' to diagnose"
            ),
            TransportClass::TransportAuthFailed => format!(
                "ssh_error: auth failed — run 'inspect connect {ns}' interactively to debug"
            ),
        }
    }
}

/// Classify an error / stderr message into a transport bucket. Returns
/// `None` for messages that do not look like transport failures (the
/// caller treats those as `Command::Failed` / generic errors).
///
/// Detection is conservative — we only escalate when we have a clear
/// match against an OpenSSH stderr pattern or the synthetic test
/// marker. Unknown failures stay `None` so the operator gets the raw
/// error rather than a misleading reauth attempt.
pub fn classify(message: &str) -> Option<TransportClass> {
    let m = message.to_ascii_lowercase();

    // Synthetic test marker emitted by the mock runner. Lets the F13
    // acceptance suite drive the classifier deterministically without
    // a live ssh process. Match the exact prefix to avoid false
    // positives on operator-typed strings.
    if let Some(rest) = m.strip_prefix("transport:") {
        let kind = rest.trim();
        return match kind {
            "stale" => Some(TransportClass::TransportStale),
            "unreachable" => Some(TransportClass::TransportUnreachable),
            "auth_failed" | "auth-failed" | "authfailed" => {
                Some(TransportClass::TransportAuthFailed)
            }
            _ => None,
        };
    }

    // AuthFailed must be checked BEFORE Stale because openssh
    // sometimes emits both "permission denied" and "connection
    // closed" in the same buffer; auth failure is the more specific
    // diagnosis.
    if m.contains("permission denied (publickey")
        || m.contains("permission denied (password")
        || m.contains("permission denied,please try again")
        || m.contains("too many authentication failures")
        || m.contains("no supported authentication methods available")
        || m.contains("host key verification failed")
        || m.contains("unprotected private key")
        || m.contains("bad passphrase")
    {
        return Some(TransportClass::TransportAuthFailed);
    }

    // Stale: master socket gone / persistent channel torn down.
    // Checked BEFORE the generic Unreachable patterns because the
    // canonical "master process exited but socket file still exists"
    // case emits `Control socket connect(/path): Connection refused`,
    // which contains both the stale-specific token AND the generic
    // "connection refused" Unreachable token. The control-socket
    // line is a local Unix-socket failure, not a remote-host one,
    // so it must win — otherwise auto-reauth never fires after a
    // codespace restart / `inspect disconnect` / ControlPersist
    // expiry, and the operator sees an unhelpful "unreachable" hint
    // for a connection that just needs to be re-established.
    if m.contains("control socket connect")
        || m.contains("controlpath does not exist")
        || m.contains("mux_client_request_session: session request failed")
        || m.contains("mux_client_request_session")
        || m.contains("mux_client_hello_exchange")
        || m.contains("multiplex: master gone")
        || m.contains("control master exited")
        || m.contains("broken pipe")
        || m.contains("connection closed by")
        || m.contains("connection reset by peer")
    {
        return Some(TransportClass::TransportStale);
    }

    // Unreachable: name resolution + transport-layer connect failures
    // that no amount of re-auth would fix.
    if m.contains("could not resolve hostname")
        || m.contains("name or service not known")
        || m.contains("temporary failure in name resolution")
        || m.contains("network is unreachable")
        || m.contains("no route to host")
        || m.contains("connection refused")
        || m.contains("connection timed out")
        || m.contains("operation timed out")
    {
        return Some(TransportClass::TransportUnreachable);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_marker_stale() {
        assert_eq!(
            classify("transport:stale"),
            Some(TransportClass::TransportStale)
        );
        assert_eq!(
            classify("Transport:STALE"),
            Some(TransportClass::TransportStale)
        );
    }

    #[test]
    fn synthetic_marker_unreachable() {
        assert_eq!(
            classify("transport:unreachable"),
            Some(TransportClass::TransportUnreachable)
        );
    }

    #[test]
    fn synthetic_marker_auth_failed() {
        assert_eq!(
            classify("transport:auth_failed"),
            Some(TransportClass::TransportAuthFailed)
        );
        assert_eq!(
            classify("transport:auth-failed"),
            Some(TransportClass::TransportAuthFailed)
        );
    }

    #[test]
    fn openssh_broken_pipe_is_stale() {
        let stderr = "packet_write_wait: Connection to 1.2.3.4 port 22: Broken pipe";
        assert_eq!(classify(stderr), Some(TransportClass::TransportStale));
    }

    #[test]
    fn openssh_connection_closed_is_stale() {
        let stderr = "Connection closed by 1.2.3.4 port 22";
        assert_eq!(classify(stderr), Some(TransportClass::TransportStale));
    }

    #[test]
    fn openssh_control_socket_gone_is_stale() {
        let stderr = "control socket connect(/tmp/sock): No such file or directory";
        assert_eq!(classify(stderr), Some(TransportClass::TransportStale));
    }

    /// Field-captured: when the master process exits but the socket
    /// file is still on disk, openssh emits BOTH "control socket
    /// connect" (stale token) AND "Connection refused" (unreachable
    /// token) in the same buffer. Stale must win — otherwise
    /// auto-reauth never fires and the operator sees an "unreachable"
    /// hint for a connection that just needs to be re-established
    /// (e.g. after a codespace restart or ControlPersist expiry).
    /// Captured 2026-05 against arte (OVH) during v0.1.3 smoke.
    #[test]
    fn openssh_dead_master_with_socket_file_is_stale_not_unreachable() {
        let stderr = "Control socket connect(/home/codespace/.inspect/sockets/arte.sock): \
                      Connection refused";
        assert_eq!(classify(stderr), Some(TransportClass::TransportStale));
    }

    #[test]
    fn openssh_mux_session_request_failed_is_stale() {
        let stderr = "mux_client_request_session: session request failed: \
                      Session open refused by peer";
        assert_eq!(classify(stderr), Some(TransportClass::TransportStale));
    }

    #[test]
    fn openssh_dns_failure_is_unreachable() {
        let stderr =
            "ssh: Could not resolve hostname bogus.example.invalid: Name or service not known";
        assert_eq!(classify(stderr), Some(TransportClass::TransportUnreachable));
    }

    #[test]
    fn openssh_connection_refused_is_unreachable() {
        let stderr = "ssh: connect to host 1.2.3.4 port 22: Connection refused";
        assert_eq!(classify(stderr), Some(TransportClass::TransportUnreachable));
    }

    #[test]
    fn openssh_publickey_denied_is_auth_failed() {
        let stderr = "Permission denied (publickey,password).";
        assert_eq!(classify(stderr), Some(TransportClass::TransportAuthFailed));
    }

    #[test]
    fn openssh_too_many_auth_is_auth_failed() {
        let stderr = "Received disconnect: Too many authentication failures";
        assert_eq!(classify(stderr), Some(TransportClass::TransportAuthFailed));
    }

    #[test]
    fn plain_command_failure_is_none() {
        assert_eq!(classify("docker: command not found"), None);
        assert_eq!(classify("exit code 1"), None);
        assert_eq!(classify(""), None);
    }

    #[test]
    fn auth_takes_precedence_over_stale_in_mixed_buffer() {
        let stderr = "Permission denied (publickey).\nConnection closed by 1.2.3.4 port 22";
        assert_eq!(classify(stderr), Some(TransportClass::TransportAuthFailed));
    }

    #[test]
    fn exit_codes_match_spec() {
        assert_eq!(TransportClass::TransportStale.exit_code(), 12);
        assert_eq!(TransportClass::TransportUnreachable.exit_code(), 13);
        assert_eq!(TransportClass::TransportAuthFailed.exit_code(), 14);
    }

    #[test]
    fn as_str_matches_json_contract() {
        assert_eq!(TransportClass::TransportStale.as_str(), "transport_stale");
        assert_eq!(
            TransportClass::TransportUnreachable.as_str(),
            "transport_unreachable"
        );
        assert_eq!(
            TransportClass::TransportAuthFailed.as_str(),
            "transport_auth_failed"
        );
    }

    #[test]
    fn summary_hint_has_chained_recovery() {
        let s = TransportClass::TransportStale.summary_hint("arte");
        assert!(s.contains("ssh_error: stale connection"));
        assert!(s.contains("inspect disconnect arte"));
        assert!(s.contains("inspect connect arte"));
    }
}
