//! Write verbs (bible §8). Every verb routes through [`crate::safety`] so
//! the dry-run / `--apply` / audit-log / interlock contract is enforced
//! consistently. A verb implementation focuses on:
//!
//! 1. building the remote command(s) for each resolved target,
//! 2. producing a preview block for dry-run,
//! 3. recording an [`crate::safety::AuditEntry`] on apply.

pub mod chmod;
pub mod chown;
pub mod edit;
pub mod exec;
pub mod lifecycle; // restart / stop / start / reload
pub mod scale; // k8s scale (K15)
pub mod delete; // k8s delete pod (K18)
pub mod rollout; // k8s rollout undo (K17)
pub mod mkdir;
pub mod rm;
pub mod touch;

pub(crate) mod atomic;

/// K20 (v0.1.4): immutable-pod ops (edit / cp / chmod / chown / mkdir / touch /
/// rm) REFUSE on a k8s namespace with a chained idiom hint — pod filesystems
/// are ephemeral and in-pod fs mutation is an anti-pattern. Returns
/// `Some(ExitKind::Error)` when the target namespace is k8s (the caller returns
/// it); `None` for docker (proceed normally). A raw kubectl/OCI error never
/// reaches the agent — the refusal is loud, specific, and actionable.
pub fn refuse_if_k8s_immutable(target: &str, verb: &str) -> Option<crate::error::ExitKind> {
    let ns = target.split('/').next().unwrap_or("");
    let resolved = crate::config::resolver::resolve(ns).ok()?;
    if resolved.config.runtime_kind() != crate::exec::runtime::RuntimeKind::K8s {
        return None;
    }
    let hint = match verb {
        "edit" => "pods are immutable — edit the ConfigMap/Secret that backs this \
                   workload, then `inspect restart <ns>/<deploy>` (rollout restart) to \
                   pick it up.",
        "cp" | "put" | "get" => "pod filesystems are ephemeral — use a ConfigMap / \
                   Secret / Volume, or raw `kubectl cp` for the rare legit case (needs \
                   `tar` in the container).",
        _ => "in-pod filesystem mutation does not survive a pod restart (anti-pattern). \
              Edit the ConfigMap/Secret and `inspect restart <ns>/<deploy>`; for live \
              debugging use `inspect exec <ns>/<pod> --apply -- <cmd>`.",
    };
    crate::error::emit(format!("{verb} is not supported for k8s pods — {hint}"));
    Some(crate::error::ExitKind::Error)
}
