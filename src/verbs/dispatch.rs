//! Common selector→target fanout used by every read verb.
//!
//! Resolves the selector, builds an SshTarget per namespace once, and
//! yields each `(target_kind, runner)` step. Failures on a single
//! namespace are surfaced through [`StepError`] but do not abort the
//! caller's loop unless it chooses to.

use std::collections::HashMap;

use anyhow::Result;
use thiserror::Error;

use crate::profile::cache::load_profile;
use crate::profile::schema::{Profile, Service};
use crate::selector::resolve::{resolve as sel_resolve, ResolvedTarget, SelectorError, TargetKind};
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
        self.ns.profile.as_ref()?.services.iter().find(|s| s.name == name)
    }
}

/// Resolve `selector` and prepare a fan-out plan. Returns the runner and a
/// vec of (NsCtx, Vec<ResolvedTarget>) pairs keyed by namespace, so the
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
        let (_, target) = resolve_target(&t.namespace).map_err(StepError::Other)?;
        let profile = load_profile(&t.namespace).map_err(StepError::Other)?;
        nses.insert(
            t.namespace.clone(),
            NsCtx {
                namespace: t.namespace.clone(),
                target,
                profile,
            },
        );
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
    let by_name: HashMap<&str, &NsCtx> =
        nses.iter().map(|n| (n.namespace.as_str(), n)).collect();
    targets.iter().filter_map(move |t| {
        by_name.get(t.namespace.as_str()).map(|ns| Step {
            ns,
            kind: t.kind.clone(),
            path: t.path.clone(),
        })
    })
}
