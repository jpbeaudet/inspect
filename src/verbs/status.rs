//! `inspect status <sel>` — service inventory + health rollup.
//!
//! Reads from the cached profile (no remote round-trip required), then
//! optionally reconciles with a live `docker ps` per namespace to mark any
//! profile services that no longer exist as `down`.

use anyhow::Result;
use serde_json::{json, Value};

use crate::cli::StatusArgs;
use crate::error::ExitKind;
use crate::profile::schema::HealthStatus;
use crate::ssh::exec::RunOpts;
use crate::verbs::correlation::{status_rules, StatusRow};
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::OutputDoc;

pub fn run(args: StatusArgs) -> Result<ExitKind> {
    // F1 (v0.1.3): bare-namespace selectors (`inspect status arte`,
    // `inspect status prod-*`) historically resolved to a single
    // host-level step, which the loop below skips, yielding a
    // misleading "0 service(s)" report on a healthy host. The
    // selector resolver explicitly defers this re-interpretation
    // to the verb layer (see `selector::resolve::resolve_services_for_ns`
    // — the `None` arm comments that "the verb layer can still
    // re-interpret this for verbs that fan out across all services
    // (e.g. `status arte` with no service portion)"). Status is
    // exactly that verb: the user's intent is "show me everything
    // in this namespace", so we rewrite a service-less selector
    // to its `/*` form before resolution. Aliases (`@name`) are
    // expanded later by the resolver and may already carry a
    // service portion, so they pass through unchanged.
    let selector = expand_bare_namespace(&args.selector);
    let (runner, nses, targets) = plan(&selector)?;

    let mut total = 0usize;
    let mut healthy = 0usize;
    let mut unhealthy = 0usize;
    let mut unknown = 0usize;

    let mut data_lines: Vec<String> = Vec::new();
    let mut services_json: Vec<Value> = Vec::new();
    let mut rows: Vec<StatusRow> = Vec::new();

    // Optionally reconcile with live state to detect down-but-cached services.
    let mut live_running: std::collections::HashMap<String, std::collections::HashSet<String>> =
        std::collections::HashMap::new();
    for ns in &nses {
        let out = runner
            .run(
                &ns.namespace,
                &ns.target,
                "docker ps --format '{{.Names}}'",
                RunOpts::with_timeout(15),
            )
            .ok();
        if let Some(o) = out {
            if o.ok() {
                let set: std::collections::HashSet<String> =
                    o.stdout.lines().map(|s| s.to_string()).collect();
                live_running.insert(ns.namespace.clone(), set);
            }
        }
    }

    for step in iter_steps(&nses, &targets) {
        let svc_name = match step.service() {
            Some(n) => n,
            None => continue, // host-level — nothing to report here
        };
        let svc_def = step.service_def();
        let mut status_str = "unknown".to_string();
        if let Some(def) = svc_def {
            let live_up = live_running
                .get(&step.ns.namespace)
                .map(|set| set.contains(&def.name))
                .unwrap_or(true);
            status_str = if !live_up {
                "down".to_string()
            } else {
                match def.health_status {
                    Some(HealthStatus::Ok) => "ok".into(),
                    Some(HealthStatus::Unhealthy) => "unhealthy".into(),
                    Some(HealthStatus::Starting) => "starting".into(),
                    Some(HealthStatus::Unknown) | None => "unknown".into(),
                }
            };
        }
        total += 1;
        match status_str.as_str() {
            "ok" => healthy += 1,
            "unhealthy" | "down" => unhealthy += 1,
            _ => unknown += 1,
        }
        let img = svc_def.and_then(|s| s.image.clone()).unwrap_or_default();
        rows.push(StatusRow {
            server: step.ns.namespace.clone(),
            service: svc_name.to_string(),
            status: status_str.clone(),
        });
        services_json.push(json!({
            "server": step.ns.namespace,
            "service": svc_name,
            "status": status_str,
            "image": img,
        }));
        data_lines.push(format!(
            "{ns}/{svc_name:<20} {status_str:<10} {img}",
            ns = step.ns.namespace
        ));
    }

    let summary =
        format!("{total} service(s): {healthy} healthy, {unhealthy} unhealthy, {unknown} unknown");
    let mut doc = OutputDoc::new(
        summary,
        json!({
            "services": services_json,
            "totals": {
                "total": total,
                "healthy": healthy,
                "unhealthy": unhealthy,
                "unknown": unknown,
            }
        }),
    )
    .with_meta("selector", args.selector.clone());
    for n in status_rules(&rows) {
        doc.push_next(n);
    }

    let fmt = args.format.resolve()?;
    crate::format::render::render_doc(&doc, &fmt, &data_lines)?;

    Ok(ExitKind::Success)
}

/// F1 helper: rewrite a service-less selector (`arte`, `prod-*`,
/// `arte~staging`) into its all-services equivalent (`arte/*` etc.)
/// so the status loop fans out over containers + systemd units
/// instead of collapsing to a single host step.
///
/// Pass-through cases:
/// - selectors that already contain `/` (caller picked an explicit
///   service / `_` host / glob form)
/// - alias references starting with `@` (resolved later, may already
///   carry a service portion)
/// - empty input (let the parser surface its own diagnostic)
fn expand_bare_namespace(sel: &str) -> String {
    if sel.is_empty() || sel.starts_with('@') || sel.contains('/') {
        return sel.to_string();
    }
    format!("{sel}/*")
}

#[cfg(test)]
mod tests {
    use super::expand_bare_namespace;

    #[test]
    fn bare_namespace_gets_all_services_glob() {
        assert_eq!(expand_bare_namespace("arte"), "arte/*");
        assert_eq!(expand_bare_namespace("prod-*"), "prod-*/*");
        assert_eq!(expand_bare_namespace("arte~staging"), "arte~staging/*");
    }

    #[test]
    fn explicit_service_form_is_unchanged() {
        assert_eq!(expand_bare_namespace("arte/atlas"), "arte/atlas");
        assert_eq!(expand_bare_namespace("arte/*"), "arte/*");
        assert_eq!(expand_bare_namespace("arte/_"), "arte/_");
    }

    #[test]
    fn alias_and_empty_pass_through() {
        assert_eq!(expand_bare_namespace("@my-alias"), "@my-alias");
        assert_eq!(expand_bare_namespace(""), "");
    }
}
