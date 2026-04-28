//! `inspect why <selector>` — service diagnostic walk (bible §12.2).
//!
//! Walks the dependency graph from the cached profile, runs a single
//! `docker ps` per namespace to learn live state, and labels each
//! transitive dependency with a status. The "likely root cause" is the
//! deepest dependency in failing state that has no failing dependency
//! beneath it.

use std::collections::{BTreeSet, HashMap};

use anyhow::Result;

use crate::cli::WhyArgs;
use crate::error::ExitKind;
use crate::profile::runtime::{RuntimeSnapshot, SourceInfo};
use crate::profile::schema::{HealthStatus, Profile, Service};
use crate::verbs::cache::{aggregate_sources, get_runtime, print_source_line, GetOpts};
use crate::verbs::correlation::why_rules;
use crate::verbs::dispatch::{iter_steps, plan, NsCtx};
use crate::verbs::output::OutputDoc;
use crate::verbs::runtime::RemoteRunner;

pub fn run(args: WhyArgs) -> Result<ExitKind> {
    let (runner, nses, targets) = plan(&args.selector)?;
    let (runtime_by_ns, sources, refresh_warnings) =
        collect_runtime(runner.as_ref(), &nses, args.refresh);
    let aggregated_source = aggregate_sources(&sources);
    let fmt = args.format.resolve()?;
    print_source_line(&aggregated_source, &fmt);

    let mut data_lines: Vec<String> = Vec::new();
    let mut services_json: Vec<serde_json::Value> = Vec::new();
    let mut overall_failing = 0usize;
    let mut overall_total = 0usize;
    let mut emitted = 0usize;
    let mut last_root: Option<(String, String)> = None; // (server, root)

    for step in iter_steps(&nses, &targets) {
        let svc_name = match step.service() {
            Some(n) => n.to_string(),
            None => continue,
        };
        let profile = match step.ns.profile.as_ref() {
            Some(p) => p,
            None => continue,
        };
        emitted += 1;
        let ns = &step.ns.namespace;
        let runtime = runtime_by_ns.get(ns);
        let walk = walk_deps(profile, &svc_name);
        let status_map: HashMap<String, NodeStatus> = walk
            .nodes
            .iter()
            .map(|n| (n.clone(), node_status(profile, runtime, n)))
            .collect();
        let root = pick_root_cause(&walk, &status_map);
        if let Some(r) = &root {
            last_root = Some((ns.clone(), r.clone()));
        }

        overall_total += walk.nodes.len();
        overall_failing += status_map
            .values()
            .filter(|s| matches!(s, NodeStatus::Down | NodeStatus::Unhealthy))
            .count();

        let nodes_json: Vec<serde_json::Value> = walk
            .order
            .iter()
            .map(|name| {
                let st = status_map.get(name).copied().unwrap_or(NodeStatus::Unknown);
                let depth = walk.depth.get(name).copied().unwrap_or(0);
                serde_json::json!({
                    "name": name,
                    "status": st.as_str(),
                    "depth": depth,
                    "depends_on": walk.edges.get(name).cloned().unwrap_or_default(),
                })
            })
            .collect();
        services_json.push(serde_json::json!({
            "server": ns,
            "service": svc_name,
            "self_status": status_map.get(&svc_name).copied().unwrap_or(NodeStatus::Unknown).as_str(),
            "root_cause": root.clone(),
            "nodes": nodes_json,
        }));

        data_lines.push(format!("{ns}/{svc_name}:"));
        for name in &walk.order {
            let depth = walk.depth.get(name).copied().unwrap_or(0);
            let st = status_map.get(name).copied().unwrap_or(NodeStatus::Unknown);
            let mark = if Some(name) == root.as_ref() {
                "  <- likely root cause"
            } else {
                ""
            };
            let indent = "  ".repeat(depth + 1);
            data_lines.push(format!("{indent}{name}: {}{mark}", st.as_str()));
        }
    }

    let summary = if emitted == 0 {
        "no services matched".to_string()
    } else {
        format!("{emitted} service(s) walked; {overall_failing} failing dep(s) of {overall_total}")
    };
    let mut doc = OutputDoc::new(
        summary,
        serde_json::json!({
            "services": services_json,
            "totals": {
                "walked": emitted,
                "failing": overall_failing,
                "total_nodes": overall_total,
            }
        }),
    )
    .with_meta("selector", args.selector.clone())
    .with_meta("source", aggregated_source.to_json());
    if let Some((server, root)) = &last_root {
        for n in why_rules(server, Some(root.as_str())) {
            doc.push_next(n);
        }
    } else if emitted > 0 {
        let server = nses
            .first()
            .map(|n| n.namespace.as_str())
            .unwrap_or("<server>");
        for n in why_rules(server, None) {
            doc.push_next(n);
        }
    }
    if aggregated_source.stale {
        doc.push_next(crate::verbs::output::NextStep::new(
            format!(
                "inspect connectivity {}",
                nses.first()
                    .map(|n| n.namespace.clone())
                    .unwrap_or_else(|| "<ns>".to_string())
            ),
            "diagnose why runtime refresh failed",
        ));
    }
    if !refresh_warnings.is_empty() {
        for w in &refresh_warnings {
            eprintln!("warning: {w}");
        }
    }

    crate::format::render::render_doc(&doc, &fmt, &data_lines)?;

    Ok(if overall_failing > 0 {
        ExitKind::Error
    } else {
        ExitKind::Success
    })
}

/// F8 (v0.1.3): per-namespace runtime snapshot collection through the
/// cache orchestrator. Returns
///   (snapshots-by-ns, per-ns SourceInfo entries, refresh warnings)
/// — the same shape `status` and `health` use. Cold cache + failed
/// refresh becomes a `Stale` source entry with no snapshot for that
/// namespace; downstream `node_status` then falls back to the
/// inventory tier's `health_status`.
fn collect_runtime(
    runner: &dyn RemoteRunner,
    nses: &[NsCtx],
    refresh: bool,
) -> (HashMap<String, RuntimeSnapshot>, Vec<SourceInfo>, Vec<String>) {
    use crate::profile::runtime::{inventory_age, SourceMode};
    let opts = GetOpts {
        force_refresh: refresh,
    };
    let mut by_ns = HashMap::new();
    let mut sources = Vec::new();
    let mut warnings = Vec::new();
    for ns in nses {
        match get_runtime(runner, ns, opts) {
            Ok((snap, info)) => {
                if info.stale {
                    if let Some(reason) = &info.reason {
                        warnings.push(format!(
                            "{}: serving cached data — {}",
                            ns.namespace, reason
                        ));
                    }
                }
                by_ns.insert(ns.namespace.clone(), snap);
                sources.push(info);
            }
            Err(_) => {
                sources.push(SourceInfo {
                    mode: SourceMode::Stale,
                    runtime_age_s: None,
                    inventory_age_s: inventory_age(&ns.namespace).map(|d| d.as_secs()),
                    stale: true,
                    reason: Some(format!("{}: runtime refresh failed (no cache)", ns.namespace)),
                });
                warnings.push(format!(
                    "{}: runtime refresh failed and no cache present",
                    ns.namespace
                ));
            }
        }
    }
    (by_ns, sources, warnings)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeStatus {
    Ok,
    Unhealthy,
    Down,
    Unknown,
    Missing,
}

impl NodeStatus {
    fn as_str(self) -> &'static str {
        match self {
            NodeStatus::Ok => "ok",
            NodeStatus::Unhealthy => "unhealthy",
            NodeStatus::Down => "down",
            NodeStatus::Unknown => "unknown",
            NodeStatus::Missing => "missing",
        }
    }
}

/// F8: prefer the runtime snapshot for both running-state and health.
/// Falls back to the inventory tier's `health_status` only when the
/// snapshot lacks a row for the container — the bug pattern from the
/// 3rd field user (where post-restart `health_status` was stuck on
/// the pre-restart value) is *exactly* what this fixes.
fn node_status(profile: &Profile, runtime: Option<&RuntimeSnapshot>, name: &str) -> NodeStatus {
    let svc: Option<&Service> = profile.services.iter().find(|s| s.name == name);
    let svc = match svc {
        Some(s) => s,
        None => return NodeStatus::Missing,
    };
    let rt = runtime.and_then(|s| s.lookup(&svc.container_name));
    if let Some(r) = rt {
        if !r.running {
            return NodeStatus::Down;
        }
        return match r.health_status.or(svc.health_status) {
            Some(HealthStatus::Ok) => NodeStatus::Ok,
            Some(HealthStatus::Unhealthy) => NodeStatus::Unhealthy,
            Some(HealthStatus::Starting) => NodeStatus::Unknown,
            Some(HealthStatus::Unknown) | None => NodeStatus::Unknown,
        };
    }
    // No runtime row for this container — degraded mode. Fall back
    // to inventory tier and treat it as unknown if missing.
    match svc.health_status {
        Some(HealthStatus::Ok) => NodeStatus::Ok,
        Some(HealthStatus::Unhealthy) => NodeStatus::Unhealthy,
        Some(HealthStatus::Starting) => NodeStatus::Unknown,
        Some(HealthStatus::Unknown) | None => NodeStatus::Unknown,
    }
}

struct Walk {
    /// Pre-order walk of the dependency tree (target service first, then
    /// dependencies depth-first). Each entry is unique.
    order: Vec<String>,
    /// All names in the walk, set form.
    nodes: BTreeSet<String>,
    /// Adjacency list, pruned to nodes that exist in the profile.
    edges: HashMap<String, Vec<String>>,
    /// Distance from the root (target service), used for indentation.
    depth: HashMap<String, usize>,
}

fn walk_deps(profile: &Profile, root: &str) -> Walk {
    let by_name: HashMap<&str, &Service> = profile
        .services
        .iter()
        .map(|s| (s.name.as_str(), s))
        .collect();

    let mut order = Vec::new();
    let mut nodes = BTreeSet::new();
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    let mut depth: HashMap<String, usize> = HashMap::new();

    fn visit<'a>(
        name: &str,
        d: usize,
        by_name: &HashMap<&'a str, &'a Service>,
        order: &mut Vec<String>,
        nodes: &mut BTreeSet<String>,
        edges: &mut HashMap<String, Vec<String>>,
        depth: &mut HashMap<String, usize>,
    ) {
        if !nodes.insert(name.to_string()) {
            return;
        }
        order.push(name.to_string());
        depth.insert(name.to_string(), d);
        let deps = by_name
            .get(name)
            .map(|s| s.depends_on.clone())
            .unwrap_or_default();
        edges.insert(name.to_string(), deps.clone());
        for dep in deps {
            visit(&dep, d + 1, by_name, order, nodes, edges, depth);
        }
    }

    visit(
        root, 0, &by_name, &mut order, &mut nodes, &mut edges, &mut depth,
    );
    Walk {
        order,
        nodes,
        edges,
        depth,
    }
}

/// The "deepest failing leaf": pick the failing node whose own
/// dependencies are all healthy (or whose deps are missing from the
/// profile). Falls back to the deepest failing node if no leaf exists.
fn pick_root_cause(walk: &Walk, status: &HashMap<String, NodeStatus>) -> Option<String> {
    let failing = |s: NodeStatus| matches!(s, NodeStatus::Down | NodeStatus::Unhealthy);
    let mut best: Option<(usize, String)> = None;
    for name in &walk.order {
        let s = status.get(name).copied().unwrap_or(NodeStatus::Unknown);
        if !failing(s) {
            continue;
        }
        let deps = walk.edges.get(name).cloned().unwrap_or_default();
        let any_failing_below = deps
            .iter()
            .any(|d| status.get(d).map(|s2| failing(*s2)).unwrap_or(false));
        if any_failing_below {
            continue;
        }
        let depth = walk.depth.get(name).copied().unwrap_or(0);
        if best.as_ref().map(|(d, _)| depth > *d).unwrap_or(true) {
            best = Some((depth, name.clone()));
        }
    }
    best.map(|(_, n)| n)
}
