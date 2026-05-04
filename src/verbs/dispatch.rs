//! Common selector→target fanout used by every read verb.
//!
//! Resolves the selector, builds an SshTarget per namespace once, and
//! yields each `(target_kind, runner)` step. Failures on a single
//! namespace are surfaced through [`StepError`] but do not abort the
//! caller's loop unless it chooses to.

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use thiserror::Error;

use crate::profile::cache::load_profile;
use crate::profile::schema::{Profile, Service};
use crate::selector::resolve::{
    chosen_namespaces_for, resolve as sel_resolve, ResolvedTarget, SelectorError, TargetKind,
};
use crate::ssh::options::SshTarget;

use super::runtime::{current_runner, resolve_target, RemoteRunner};

#[derive(Debug, Error)]
pub enum StepError {
    #[error(transparent)]
    Selector(#[from] SelectorError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Per-namespace bundle.
pub struct NsCtx {
    pub namespace: String,
    pub target: SshTarget,
    pub profile: Option<Profile>,
    /// F12 (v0.1.3): per-namespace remote env overlay, sourced from
    /// `[namespaces.<ns>.env]` in `~/.inspect/servers.toml` (merged
    /// with any env-var overrides). Empty when no overlay is
    /// configured. Verbs that dispatch operator-supplied free-form
    /// commands (`run`, `exec`) consult this map and prepend an
    /// `env KEY="VAL" ... -- ` to the remote command line via
    /// [`crate::exec::env_overlay::apply_to_cmd`]. Read verbs
    /// (`logs`, `ps`, `status`, etc.) issue inspect-internal
    /// commands and ignore this field — see F12 spec scope.
    pub env_overlay: BTreeMap<String, String>,
    /// F13 (v0.1.3): per-namespace policy on stale-session
    /// auto-reauth. Sourced from `[servers.<ns>].auto_reauth` in
    /// `~/.inspect/servers.toml`; defaults to `true` when the field
    /// is absent. `--no-reauth` on `run` / `exec` overrides this
    /// per-invocation. Verbs that wrap dispatch consult this field
    /// before invoking the reauth path.
    pub auto_reauth: bool,
}

/// A flat resolution: namespace context + a single target inside it.
pub struct Step<'a> {
    pub ns: &'a NsCtx,
    pub kind: TargetKind,
    pub path: Option<String>,
}

impl<'a> Step<'a> {
    pub fn service(&self) -> Option<&str> {
        match &self.kind {
            TargetKind::Service { name } => Some(name),
            _ => None,
        }
    }

    /// Look up the service definition in the cached profile, if any.
    pub fn service_def(&self) -> Option<&'a Service> {
        let name = self.service()?;
        self.ns
            .profile
            .as_ref()?
            .services
            .iter()
            .find(|s| s.name == name)
    }

    /// Container name for `docker logs|exec|restart|stop|start|kill`
    /// commands. Resolves to `service_def().container_name` when a
    /// profile entry exists; otherwise falls back to `service()` so
    /// the command still runs on hosts that haven't been discovered.
    /// Returns `None` for host-level steps (`arte/_`).
    ///
    /// Field pitfall §6.1 (v0.1.1 P2): the user-facing service name
    /// (`name`, possibly a compose label like `api`) is what the
    /// selector matches on, but the docker daemon only knows the real
    /// container name (`luminary-api`). Always pass `container()` to
    /// docker, never `service()`.
    pub fn container(&self) -> Option<&str> {
        match self.service_def() {
            Some(def) => Some(def.container_name.as_str()),
            None => self.service(),
        }
    }
}

/// Resolve `selector` and prepare a fan-out plan. Returns the runner and a
/// vec of `(NsCtx, Vec<ResolvedTarget>)` pairs keyed by namespace, so the
/// caller can iterate in stable namespace order.
pub type Plan = (Box<dyn RemoteRunner>, Vec<NsCtx>, Vec<ResolvedTarget>);

#[allow(clippy::result_large_err)]
pub fn plan(selector: &str) -> Result<Plan, StepError> {
    let targets = sel_resolve(selector)?;
    // Build a per-namespace context once.
    let mut nses: HashMap<String, NsCtx> = HashMap::new();
    for t in &targets {
        if nses.contains_key(&t.namespace) {
            continue;
        }
        let (resolved, target) = resolve_target(&t.namespace).map_err(StepError::Other)?;
        let profile = load_profile(&t.namespace).map_err(StepError::Other)?;
        nses.insert(
            t.namespace.clone(),
            NsCtx {
                namespace: t.namespace.clone(),
                target,
                profile,
                env_overlay: resolved.config.env.clone().unwrap_or_default(),
                auto_reauth: resolved.config.auto_reauth.unwrap_or(true),
            },
        );
    }
    // F7.5 (v0.1.3): a wildcard selector ("everything in this
    // namespace") against an empty profile resolves to zero targets,
    // but the verb still wants to talk to the namespace's host (e.g.
    // status's empty-state path needs `docker ps` so it can phrase
    // "N container(s) discovered but unmatched"). Backfill `nses`
    // from the namespaces the selector actually matched, even when
    // no service-level targets came back.
    if nses.is_empty() {
        if let Ok(ns_names) = chosen_namespaces_for(selector) {
            for ns in ns_names {
                if nses.contains_key(&ns) {
                    continue;
                }
                let (resolved, target) = resolve_target(&ns).map_err(StepError::Other)?;
                let profile = load_profile(&ns).map_err(StepError::Other)?;
                nses.insert(
                    ns.clone(),
                    NsCtx {
                        namespace: ns,
                        target,
                        profile,
                        env_overlay: resolved.config.env.clone().unwrap_or_default(),
                        auto_reauth: resolved.config.auto_reauth.unwrap_or(true),
                    },
                );
            }
        }
    }
    let mut ns_vec: Vec<NsCtx> = nses.into_values().collect();
    ns_vec.sort_by(|a, b| a.namespace.cmp(&b.namespace));
    Ok((current_runner(), ns_vec, targets))
}

/// Convenience iterator: zip the planned namespaces with their resolved
/// targets, yielding [`Step`]s in deterministic order.
pub fn iter_steps<'a>(
    nses: &'a [NsCtx],
    targets: &'a [ResolvedTarget],
) -> impl Iterator<Item = Step<'a>> + 'a {
    let by_name: HashMap<&str, &NsCtx> = nses.iter().map(|n| (n.namespace.as_str(), n)).collect();
    targets.iter().filter_map(move |t| {
        by_name.get(t.namespace.as_str()).map(|ns| Step {
            ns,
            kind: t.kind.clone(),
            path: t.path.clone(),
        })
    })
}
