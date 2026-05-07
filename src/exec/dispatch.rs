//! F13 (v0.1.3): transport-failure-aware dispatch wrapper.
//!
//! Verbs that ship operator-supplied commands to the remote host
//! (`run`, `exec`) call into [`dispatch_with_reauth`] instead of the
//! raw [`RemoteRunner`] trait so the stale-session retry policy lives
//! in exactly one place. The wrapper:
//!
//! 1. Calls the caller-supplied `do_dispatch` closure (which performs
//!    the actual `runner.run_streaming` / `run_streaming_capturing` /
//!    `run`).
//! 2. On `Err(e)`, classifies via [`crate::ssh::transport::classify`].
//! 3. On a `TransportStale` classification AND when reauth is allowed
//!    (per-namespace `auto_reauth = true` AND no `--no-reauth` flag),
//!    writes a one-line stderr notice, records a `connect.reauth`
//!    audit entry, calls [`RemoteRunner::reauth`], and re-runs the
//!    dispatch closure exactly once. The retry's failure (if any) is
//!    final; no exponential backoff, no second retry.
//! 4. On a failed reauth attempt, the wrapper escalates the original
//!    failure to `TransportAuthFailed` so the caller exits with code
//!    14 + the chained `inspect connect <ns>` hint.
//!
//! See `docs/RUNBOOK.md` §10 for the full contract.

use crate::safety::audit::{AuditEntry, AuditStore};
use crate::ssh::options::SshTarget;
use crate::ssh::transport::{classify, TransportClass};
use crate::verbs::runtime::RemoteRunner;

/// Reauth policy for a single dispatch site. Sourced from the
/// per-namespace config (`auto_reauth`) ANDed with the per-invocation
/// flag (`--no-reauth` inverts to `false`).
#[derive(Debug, Clone, Copy)]
pub struct ReauthPolicy {
    /// `false` disables the auto-reauth path entirely; the dispatch
    /// wrapper classifies and surfaces the failure unchanged.
    pub allow_reauth: bool,
}

/// One-shot dispatch outcome. `result` carries the final dispatch
/// result (post-retry if a retry happened); `failure_class` carries
/// the transport classification that should populate the verb's
/// `failure_class` JSON field and SUMMARY trailer; `reauth_id` is
/// `Some` when a reauth audit entry was written so the caller can
/// stamp it onto the retry's audit entry; `retried` is `true` when
/// the reauth path actually fired and re-dispatched.
pub struct DispatchOutcome<T> {
    pub result: anyhow::Result<T>,
    pub failure_class: Option<TransportClass>,
    pub reauth_id: Option<String>,
    pub retried: bool,
}

/// Run `do_dispatch` once (and, on transport-stale, again after a
/// reauth) under the F13 contract. Returns a [`DispatchOutcome`] the
/// caller threads into its audit + SUMMARY rendering.
#[allow(clippy::too_many_arguments)]
pub fn dispatch_with_reauth<T>(
    namespace: &str,
    target: &SshTarget,
    runner: &dyn RemoteRunner,
    audit_store: Option<&AuditStore>,
    original_verb: &str,
    selector: &str,
    policy: ReauthPolicy,
    mut do_dispatch: impl FnMut() -> anyhow::Result<T>,
) -> DispatchOutcome<T> {
    let first = do_dispatch();
    let err = match first {
        Ok(v) => {
            return DispatchOutcome {
                result: Ok(v),
                failure_class: None,
                reauth_id: None,
                retried: false,
            };
        }
        Err(e) => e,
    };

    let class = classify(&err.to_string());
    // Non-stale transport failures (or unclassified errors) are
    // surfaced as-is; reauth doesn't help.
    if class != Some(TransportClass::TransportStale) || !policy.allow_reauth {
        return DispatchOutcome {
            result: Err(err),
            failure_class: class,
            reauth_id: None,
            retried: false,
        };
    }

    // Stale + reauth-allowed: emit the operator notice, record the
    // reauth audit entry, attempt reauth, and retry once.
    eprintln!("note: persistent session for {namespace} expired — re-authenticating…");
    let reauth_id = audit_store.map(|store| {
        let mut e = AuditEntry::new("connect.reauth", namespace);
        e.args =
            format!("trigger=transport_stale,original_verb={original_verb},selector={selector}");
        e.exit = 0;
        let id = e.id.clone();
        // P8-C fix (v0.1.3): the reauth entry is a side-effect of the
        // operator's verb invocation, not the primary audit. Skip the
        // F18 transcript link so the verb's own audit_id (appended
        // later on retry success) wins the footer slot. The reauth
        // entry remains discoverable via `audit show <reauth_id>` /
        // `audit grep verb=connect.reauth` and chains forensically to
        // the verb via the verb-side `retry_of` field.
        let _ = store.append_without_transcript_link(&e);
        id
    });

    if let Err(reauth_err) = runner.reauth(namespace, target) {
        // Reauth itself failed — escalate to AuthFailed exit class.
        return DispatchOutcome {
            result: Err(anyhow::anyhow!(
                "auto-reauth for '{namespace}' failed: {reauth_err}"
            )),
            failure_class: Some(TransportClass::TransportAuthFailed),
            reauth_id,
            retried: false,
        };
    }

    // Retry exactly once. Whatever this returns is final.
    let second = do_dispatch();
    let retry_class = match &second {
        Ok(_) => None,
        Err(e) => classify(&e.to_string()),
    };
    DispatchOutcome {
        result: second,
        failure_class: retry_class,
        reauth_id,
        retried: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ssh::exec::{RemoteOutput, RunOpts};
    use std::sync::Mutex;

    struct MockRunner {
        sequence: Mutex<Vec<Result<String, String>>>,
        reauth_ok: bool,
        reauth_calls: Mutex<u32>,
    }

    impl RemoteRunner for MockRunner {
        fn run(&self, _: &str, _: &SshTarget, _: &str, _: RunOpts) -> anyhow::Result<RemoteOutput> {
            let mut seq = self.sequence.lock().unwrap();
            let next = seq.remove(0);
            match next {
                Ok(s) => Ok(RemoteOutput {
                    stdout: s,
                    stderr: String::new(),
                    exit_code: 0,
                }),
                Err(msg) => Err(anyhow::anyhow!(msg)),
            }
        }

        fn reauth(&self, _: &str, _: &SshTarget) -> anyhow::Result<()> {
            *self.reauth_calls.lock().unwrap() += 1;
            if self.reauth_ok {
                Ok(())
            } else {
                Err(anyhow::anyhow!("mock reauth failed"))
            }
        }
    }

    fn mk(seq: Vec<Result<&'static str, &'static str>>, reauth_ok: bool) -> MockRunner {
        MockRunner {
            sequence: Mutex::new(
                seq.into_iter()
                    .map(|r| r.map(String::from).map_err(String::from))
                    .collect(),
            ),
            reauth_ok,
            reauth_calls: Mutex::new(0),
        }
    }

    fn target() -> SshTarget {
        SshTarget {
            host: "h".into(),
            user: "u".into(),
            port: 22,
            key_path: None,
        }
    }

    fn dispatch<R: RemoteRunner>(r: &R, allow: bool) -> DispatchOutcome<String> {
        dispatch_with_reauth(
            "arte",
            &target(),
            r,
            None,
            "run",
            "arte/svc",
            ReauthPolicy {
                allow_reauth: allow,
            },
            || {
                r.run("arte", &target(), "echo", RunOpts::default())
                    .map(|o| o.stdout)
            },
        )
    }

    #[test]
    fn first_call_succeeds_no_retry() {
        let m = mk(vec![Ok("hello")], true);
        let out = dispatch(&m, true);
        assert!(out.result.is_ok());
        assert!(out.failure_class.is_none());
        assert!(!out.retried);
        assert_eq!(*m.reauth_calls.lock().unwrap(), 0);
    }

    #[test]
    fn stale_then_success_after_reauth_retries_once() {
        let m = mk(vec![Err("transport:stale"), Ok("ok")], true);
        let out = dispatch(&m, true);
        assert_eq!(out.result.as_ref().ok().map(|s| s.as_str()), Some("ok"));
        assert!(out.retried);
        assert_eq!(*m.reauth_calls.lock().unwrap(), 1);
        assert!(out.failure_class.is_none());
    }

    #[test]
    fn stale_with_failed_reauth_escalates_to_auth_failed() {
        let m = mk(vec![Err("transport:stale")], false);
        let out = dispatch(&m, true);
        assert!(out.result.is_err());
        assert_eq!(out.failure_class, Some(TransportClass::TransportAuthFailed));
        assert!(!out.retried);
        assert_eq!(*m.reauth_calls.lock().unwrap(), 1);
    }

    #[test]
    fn stale_with_no_reauth_policy_does_not_retry() {
        let m = mk(vec![Err("transport:stale")], true);
        let out = dispatch(&m, false);
        assert!(out.result.is_err());
        assert_eq!(out.failure_class, Some(TransportClass::TransportStale));
        assert!(!out.retried);
        assert_eq!(*m.reauth_calls.lock().unwrap(), 0);
    }

    #[test]
    fn unreachable_does_not_attempt_reauth() {
        let m = mk(vec![Err("transport:unreachable")], true);
        let out = dispatch(&m, true);
        assert!(out.result.is_err());
        assert_eq!(
            out.failure_class,
            Some(TransportClass::TransportUnreachable)
        );
        assert!(!out.retried);
        assert_eq!(*m.reauth_calls.lock().unwrap(), 0);
    }
}
