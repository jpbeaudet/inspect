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
use crate::verbs::cache::{aggregate_sources, get_runtime, print_source_line, GetOpts};
use crate::verbs::correlation::{status_rules, StatusRow};
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::OutputDoc;

pub fn run(args: StatusArgs) -> Result<ExitKind> {
    // Bare-namespace selectors (`inspect status arte`,
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

    // A k8s namespace is presented from its discovered profile
    // (health-aware) — the docker `plan()`/`get_runtime` pipeline
    // is SSH-bound and would fail on a hostless k8s config. Branch early.
    if let Some(ns_name) = selector.split('/').next() {
        if let Ok(resolved) = crate::config::resolver::resolve(ns_name) {
            if resolved.config.runtime_kind() == crate::exec::runtime::RuntimeKind::K8s {
                return status_k8s(&args, ns_name, &selector);
            }
        }
    }

    let (runner, nses, targets) = plan(&selector)?;

    let mut total = 0usize;
    let mut healthy = 0usize;
    let mut unhealthy = 0usize;
    let mut unknown = 0usize;

    let mut data_lines: Vec<String> = Vec::new();
    let mut services_json: Vec<Value> = Vec::new();
    let mut rows: Vec<StatusRow> = Vec::new();

    // Pull a runtime snapshot per namespace through the
    // cache orchestrator. The orchestrator decides live vs cached vs
    // stale based on TTL and the `--refresh` flag, returns a
    // [`SourceInfo`] describing the choice, and silently saves any
    // freshly-fetched snapshot to disk for the next read. The verb
    // itself never touches `docker ps` directly any more.
    let opts = GetOpts {
        force_refresh: args.refresh,
    };
    let mut runtime_by_ns: std::collections::HashMap<
        String,
        crate::profile::runtime::RuntimeSnapshot,
    > = std::collections::HashMap::new();
    // Aggregate the per-ns SourceInfo into a single line for output.
    // For the common single-namespace case this is just that namespace's
    // info; for multi-ns selectors we surface "live" if every ns was
    // refreshed, "stale" if any failed, otherwise "cached".
    let mut sources: Vec<crate::profile::runtime::SourceInfo> = Vec::new();
    let mut refresh_warnings: Vec<String> = Vec::new();
    for ns in &nses {
        match get_runtime(runner.as_ref(), ns, opts) {
            Ok((snap, info)) => {
                if info.stale {
                    if let Some(reason) = &info.reason {
                        refresh_warnings.push(format!(
                            "{}: serving cached data — {}",
                            ns.namespace, reason
                        ));
                    }
                }
                runtime_by_ns.insert(ns.namespace.clone(), snap);
                sources.push(info);
            }
            Err(_) => {
                // Cold cache + refresh failed: fall through to a
                // dry inventory-only view. Mark the source as stale
                // with a clear reason so the operator knows runtime
                // facts are missing.
                sources.push(crate::profile::runtime::SourceInfo {
                    mode: crate::profile::runtime::SourceMode::Stale,
                    runtime_age_s: None,
                    inventory_age_s: crate::profile::runtime::inventory_age(&ns.namespace)
                        .map(|d| d.as_secs()),
                    stale: true,
                    reason: Some(format!(
                        "{}: runtime refresh failed (no cache)",
                        ns.namespace
                    )),
                });
                refresh_warnings.push(format!(
                    "{}: runtime refresh failed and no cache present",
                    ns.namespace
                ));
            }
        }
    }
    let aggregated_source = aggregate_sources(&sources);
    let fmt = args.format.resolve()?;
    print_source_line(&aggregated_source, &fmt);

    for step in iter_steps(&nses, &targets) {
        let svc_name = match step.service() {
            Some(n) => n,
            None => continue, // host-level — nothing to report here
        };
        let svc_def = step.service_def();
        let mut status_str = "unknown".to_string();
        if let Some(def) = svc_def {
            // Prefer the live runtime snapshot for both
            // running-state and health. The cached profile's
            // health_status was the post-`setup` value and is
            // exactly the field that went stale after the 3rd
            // user's restart — never read it here directly.
            let rt = runtime_by_ns
                .get(&step.ns.namespace)
                .and_then(|s| s.lookup(&def.container_name));
            let live_up = rt.map(|r| r.running).unwrap_or(true);
            status_str = if !live_up {
                "down".to_string()
            } else {
                let health = rt.and_then(|r| r.health_status).or(def.health_status);
                match health {
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
        // Surface docker `container_name` as an alias when it is
        // distinct from the canonical compose service name. Always
        // emit the field (empty array when no aliases) so the JSON
        // schema is stable for agent consumers.
        let aliases: Vec<String> = match svc_def {
            Some(def) if def.container_name != def.name => vec![def.container_name.clone()],
            _ => Vec::new(),
        };
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
            "aliases": aliases,
        }));
        data_lines.push(format!(
            "{ns}/{svc_name:<20} {status_str:<10} {img}",
            ns = step.ns.namespace
        ));
    }

    // Empty-state phrasing + `state` field.
    //
    //   - "ok"                 — at least one service classified
    //   - "no_services_matched"— inventory non-empty but zero services
    //   - "empty_inventory"    — inventory empty too (host clean / down)
    //
    // The first case exists today; the latter two used to read as
    // "0 service(s): 0 healthy, 0 unhealthy, 0 unknown" — alarming
    // when the actual condition is "no service definitions configured
    // for this namespace."
    let inventory_count: usize = runtime_by_ns.values().map(|s| s.services.len()).sum();
    let state = if total > 0 {
        "ok"
    } else if inventory_count > 0 {
        "no_services_matched"
    } else {
        "empty_inventory"
    };
    let summary = match state {
        "ok" => format!(
            "{total} service(s): {healthy} healthy, {unhealthy} unhealthy, {unknown} unknown"
        ),
        "no_services_matched" => {
            let ns0 = first_namespace(&nses);
            format!(
                "no service definitions configured for {ns0} — {inventory_count} container(s) discovered but unmatched"
            )
        }
        _ => format!(
            "{total} service(s): {healthy} healthy, {unhealthy} unhealthy, {unknown} unknown"
        ),
    };
    // Surface the cached compose project list. We
    // always read from the inventory tier (not runtime) because
    // compose project membership rarely changes between deploys
    // and a fresh probe would cost a second remote round-trip per
    // namespace just to refresh a count.
    let mut compose_total = 0usize;
    let mut compose_projects_json: Vec<Value> = Vec::new();
    for ns in &nses {
        if let Some(profile) = ns.profile.as_ref() {
            for cp in &profile.compose_projects {
                compose_total += 1;
                compose_projects_json.push(json!({
                    "namespace": ns.namespace,
                    "name": cp.name,
                    "status": cp.status,
                    "compose_file": cp.compose_file,
                    "working_dir": cp.working_dir,
                    "service_count": cp.service_count,
                    "running_count": cp.running_count,
                }));
            }
        }
    }

    let mut doc = OutputDoc::new(
        summary,
        json!({
            "services": services_json,
            "state": state,
            "totals": {
                "total": total,
                "healthy": healthy,
                "unhealthy": unhealthy,
                "unknown": unknown,
            },
            "compose_projects": compose_projects_json,
        }),
    )
    .with_meta("selector", args.selector.clone())
    // Stable JSON contract — every read-verb response carries
    // `meta.source` so agents can tell live from cached without
    // parsing the SOURCE: prose line.
    .with_meta("source", aggregated_source.to_json())
    .with_quiet(args.format.quiet);
    // Chained next-actions for the no-services-matched empty
    // state. Lead with `inspect ps` (see what the host actually has)
    // then `inspect setup --force` (re-classify if a service was
    // expected). Suppressed for the populated-OK and empty-inventory
    // cases — there's nothing useful to say in either.
    if state == "no_services_matched" {
        let ns0 = first_namespace(&nses);
        doc.push_next(crate::verbs::output::NextStep::new(
            format!("inspect ps {ns0}"),
            "list the containers discovered on this namespace",
        ));
        doc.push_next(crate::verbs::output::NextStep::new(
            format!("inspect setup {ns0} --force"),
            "re-run discovery to classify services",
        ));
    }
    for n in status_rules(&rows) {
        doc.push_next(n);
    }
    // Stale-source chained hint — when the cache is degraded,
    // surface the connectivity check as the next concrete action.
    if aggregated_source.stale {
        doc.push_next(crate::verbs::output::NextStep::new(
            format!("inspect connectivity {}", first_namespace(&nses)),
            "diagnose why runtime refresh failed",
        ));
    }

    if !refresh_warnings.is_empty() {
        // Emit on stderr so JSON consumers don't pollute stdout, but
        // human operators still see the per-ns reason. Single line per
        // namespace; never per-service spam.
        for w in &refresh_warnings {
            crate::tee_eprintln!("warning: {w}");
        }
    }

    // Append a `compose_projects: N` DATA line so
    // human operators see at a glance whether the host runs any
    // compose projects (and how many) without dropping back to
    // `inspect compose ls`. The line is only emitted when the
    // count > 0 — adding "compose_projects: 0" to every status
    // call on plain container hosts would be noise.
    if compose_total > 0 {
        data_lines.push(format!("compose_projects: {compose_total}"));
    }

    crate::format::render::render_doc(&doc, &fmt, &data_lines, args.format.select_spec())
}
/// Present a k8s namespace's `status` from its discovered,
/// health-aware profile. No live docker-ps reconcile — the profile's
/// `health_status` (set at `setup` from pod readiness) IS the source; an
/// operator re-runs `inspect setup <ns> --force` (or status --refresh, a
/// future increment) to refresh. Emits the same envelope shape as docker
/// status so an agent branches identically across mediums.
fn status_k8s(args: &StatusArgs, ns: &str, selector: &str) -> Result<ExitKind> {
    let fmt = args.format.resolve()?;
    let profile = match crate::profile::cache::load_profile(ns)? {
        Some(p) => p,
        None => {
            let mut doc = OutputDoc::new(
                format!("no cached profile for '{ns}' — run `inspect setup {ns}` first"),
                json!({
                    "services": [],
                    "state": "empty_inventory",
                    "totals": {"total": 0, "healthy": 0, "unhealthy": 0, "unknown": 0},
                    "compose_projects": [],
                }),
            )
            .with_meta("selector", args.selector.clone())
            .with_meta("runtime", "k8s".to_string());
            doc.push_next(crate::verbs::output::NextStep::new(
                format!("inspect setup {ns}"),
                "discover the cluster's pods first",
            ));
            return crate::format::render::render_doc(&doc, &fmt, &[], args.format.select_spec());
        }
    };

    let svc_pat = selector.split_once('/').map(|(_, s)| s).unwrap_or("*");
    let (mut total, mut healthy, mut unhealthy, mut unknown) = (0usize, 0, 0, 0);
    let mut services_json: Vec<Value> = Vec::new();
    let mut data_lines: Vec<String> = Vec::new();
    for s in &profile.services {
        if !k8s_glob(svc_pat, &s.name) {
            continue;
        }
        let status_str = match s.health_status {
            Some(HealthStatus::Ok) => "ok",
            Some(HealthStatus::Unhealthy) => "unhealthy",
            Some(HealthStatus::Starting) => "starting",
            _ => "unknown",
        };
        total += 1;
        match status_str {
            "ok" => healthy += 1,
            "unhealthy" => unhealthy += 1,
            _ => unknown += 1,
        }
        let img = s.image.clone().unwrap_or_default();
        services_json.push(json!({
            "server": ns,
            "service": s.name,
            "status": status_str,
            "image": img,
            "aliases": [],
        }));
        data_lines.push(format!(
            "{ns}/{name:<28} {status_str:<10} {img}",
            name = s.name
        ));
    }

    let state = if total > 0 { "ok" } else { "empty_inventory" };
    let summary =
        format!("{total} pod(s): {healthy} healthy, {unhealthy} unhealthy, {unknown} unknown");
    let doc = OutputDoc::new(
        summary,
        json!({
            "services": services_json,
            "state": state,
            "totals": {"total": total, "healthy": healthy, "unhealthy": unhealthy, "unknown": unknown},
            "compose_projects": [],
        }),
    )
    .with_meta("selector", args.selector.clone())
    .with_meta("runtime", "k8s".to_string())
    .with_meta("context", profile.host.clone())
    .with_meta("source", "cached (profile) — refresh with `inspect setup --force`".to_string())
    .with_quiet(args.format.quiet);
    crate::format::render::render_doc(&doc, &fmt, &data_lines, args.format.select_spec())
}

/// Minimal pod-name glob for k8s status: `*`/empty matches all; a trailing
/// `*` is a prefix match; otherwise exact. (Selector-grammar globbing is
/// refined in the selector work; this covers the common `coredns*` shape.)
fn k8s_glob(pat: &str, name: &str) -> bool {
    if pat.is_empty() || pat == "*" {
        return true;
    }
    if let Some(prefix) = pat.strip_suffix('*') {
        return name.starts_with(prefix);
    }
    name == pat
}

/// Rewrite a service-less selector (`arte`, `prod-*`,
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

// `aggregate_sources` lives in `verbs::cache` (shared with `health`,
// `why`) — see [`crate::verbs::cache::aggregate_sources`].

fn first_namespace(nses: &[crate::verbs::dispatch::NsCtx]) -> String {
    nses.first()
        .map(|n| n.namespace.clone())
        .unwrap_or_else(|| "<ns>".to_string())
}

#[cfg(test)]
mod tests {
    use super::{expand_bare_namespace, k8s_glob};

    #[test]
    fn bare_namespace_gets_all_services_glob() {
        assert_eq!(expand_bare_namespace("arte"), "arte/*");
        assert_eq!(expand_bare_namespace("prod-*"), "prod-*/*");
        assert_eq!(expand_bare_namespace("arte~staging"), "arte~staging/*");
    }

    #[test]
    fn k7_k8s_glob_matches() {
        assert!(k8s_glob("*", "coredns-abc"));
        assert!(k8s_glob("", "anything"));
        assert!(k8s_glob("coredns*", "coredns-c4dbffb5f-lpx6g"));
        assert!(!k8s_glob("coredns*", "traefik-9bcdbbd9"));
        assert!(k8s_glob("traefik-9bcdbbd9-n9mhf", "traefik-9bcdbbd9-n9mhf"));
        assert!(!k8s_glob("traefik", "traefik-9bcdbbd9-n9mhf")); // exact, not prefix
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
