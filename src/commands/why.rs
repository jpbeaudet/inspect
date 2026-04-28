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
use crate::ssh::SshTarget;
use crate::verbs::cache::{aggregate_sources, get_runtime, print_source_line, GetOpts};
use crate::verbs::correlation::why_rules;
use crate::verbs::dispatch::{iter_steps, plan, NsCtx};
use crate::verbs::output::OutputDoc;
use crate::verbs::runtime::RemoteRunner;

/// F4 (v0.1.3): hard cap on the recent-logs tail. Anything larger is
/// clamped with a one-line stderr notice. Protects the operator from
/// accidentally pulling tens of thousands of lines through redaction.
pub const LOG_TAIL_CAP: u32 = 200;

pub fn run(args: WhyArgs) -> Result<ExitKind> {
    let (runner, nses, targets) = plan(&args.selector)?;
    let (runtime_by_ns, sources, refresh_warnings) =
        collect_runtime(runner.as_ref(), &nses, args.refresh);
    let aggregated_source = aggregate_sources(&sources);
    let fmt = args.format.resolve()?;
    print_source_line(&aggregated_source, &fmt);

    // F4 (v0.1.3): clamp --log-tail at LOG_TAIL_CAP with a one-line
    // stderr notice. Keeps redaction + transport bills bounded.
    let log_tail_clamped = if args.log_tail > LOG_TAIL_CAP {
        eprintln!(
            "warning: --log-tail {} clamped to {}",
            args.log_tail, LOG_TAIL_CAP
        );
        LOG_TAIL_CAP
    } else {
        args.log_tail
    };

    let mut data_lines: Vec<String> = Vec::new();
    let mut services_json: Vec<serde_json::Value> = Vec::new();
    let mut overall_failing = 0usize;
    let mut overall_total = 0usize;
    let mut emitted = 0usize;
    let mut last_root: Option<(String, String)> = None; // (server, root)
    let mut bundle_next_steps: Vec<crate::verbs::output::NextStep> = Vec::new();

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
            "recent_logs": serde_json::Value::Array(Vec::new()),
            "effective_command": serde_json::Value::Null,
            "port_reality": serde_json::Value::Array(Vec::new()),
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

        // F4 (v0.1.3): diagnostic bundle for failing target services.
        // Only fires for the target service (not transitive deps), and
        // only when its status is unhealthy/down. ≤4 remote commands
        // per service per bundle invocation.
        let target_status = status_map
            .get(&svc_name)
            .copied()
            .unwrap_or(NodeStatus::Unknown);
        if !args.no_bundle
            && matches!(target_status, NodeStatus::Unhealthy | NodeStatus::Down)
        {
            if let Some(svc) = profile.services.iter().find(|s| s.name == svc_name) {
                let bundle = collect_diagnostic_bundle(
                    runner.as_ref(),
                    ns,
                    &step.ns.target,
                    &svc.container_name,
                    log_tail_clamped,
                );
                // Patch the JSON entry we just pushed with the real
                // bundle fields so agents see populated data.
                if let Some(last) = services_json.last_mut() {
                    last["recent_logs"] = serde_json::Value::Array(
                        bundle
                            .recent_logs
                            .iter()
                            .map(|s| serde_json::Value::String(s.clone()))
                            .collect(),
                    );
                    last["effective_command"] = bundle
                        .effective_command
                        .as_ref()
                        .map(|ec| serde_json::to_value(ec).unwrap_or(serde_json::Value::Null))
                        .unwrap_or(serde_json::Value::Null);
                    last["port_reality"] =
                        serde_json::to_value(&bundle.port_reality).unwrap_or_else(|_| {
                            serde_json::Value::Array(Vec::new())
                        });
                }
                for line in bundle.render_text() {
                    data_lines.push(line);
                }
                if bundle.has_double_bind() {
                    bundle_next_steps.push(crate::verbs::output::NextStep::new(
                        format!(
                            "inspect run {ns}/{svc_name} -- 'cat /docker-entrypoint.sh'"
                        ),
                        "inspect entrypoint for flag-injection (port bound twice)",
                    ));
                }
                if bundle.logs_show_port_conflict() {
                    bundle_next_steps.push(crate::verbs::output::NextStep::new(
                        format!("inspect ports {ns}"),
                        "host port reality (logs show 'address already in use')",
                    ));
                }
            }
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
    // F4: smart NEXT hints derived from the bundle (entrypoint
    // inspection on bound-twice, host port reality on "address
    // already in use" log lines). These come *before* the generic
    // why_rules suggestions so the most actionable guidance lands
    // first.
    for step in bundle_next_steps {
        doc.push_next(step);
    }
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

// ─────────────────────────────────────────────────────────────────────────────
// F4 (v0.1.3): diagnostic bundle.
//
// For unhealthy / down / restart-looping target services, attach three
// artifacts inline so the operator (human or LLM) doesn't have to
// re-run `inspect logs`, `inspect inspect`, and `inspect ports` to
// reconstruct the picture. ≤4 remote commands per service per
// invocation; each independent so partial failure still surfaces what
// worked.
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
struct EffectiveCommand {
    entrypoint: Vec<String>,
    cmd: Vec<String>,
    /// Wrapper-script-injected flag (e.g. `-dev-listen-address=0.0.0.0:8200`)
    /// detected by reading the container's docker-entrypoint script. None
    /// when no injection pattern is found.
    wrapper_injects: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct PortRow {
    port: u16,
    /// `"bound→<host>:<port>"` when the host is listening on this
    /// port, or `"free"` otherwise.
    host: String,
    /// `"bound"`, `"bound (twice!)"`, or `"exposed"` from the
    /// container's perspective. "twice!" fires when the port is
    /// declared both in `PortBindings` *and* in an entrypoint
    /// wrapper-injected listener flag — the headline reproducer
    /// pattern from the Vault triage.
    container: String,
    /// `"config"`, `"entrypoint <flag>"`, `"config + entrypoint <flag>"`,
    /// or `"exposed"` — how this port came to be declared.
    declared_by: String,
}

#[derive(Debug, Clone, Default)]
struct DiagnosticBundle {
    recent_logs: Vec<String>,
    effective_command: Option<EffectiveCommand>,
    port_reality: Vec<PortRow>,
}

impl DiagnosticBundle {
    /// Render the three sections as indented text lines for the human
    /// `DATA:` block.
    fn render_text(&self) -> Vec<String> {
        let mut lines = Vec::new();
        lines.push("  logs:".to_string());
        if self.recent_logs.is_empty() {
            lines.push("    (no recent logs)".to_string());
        } else {
            for l in &self.recent_logs {
                lines.push(format!("    {l}"));
            }
        }
        lines.push("  effective_command:".to_string());
        match &self.effective_command {
            Some(ec) => {
                lines.push(format!("    entrypoint: {:?}", ec.entrypoint));
                lines.push(format!("    cmd: {:?}", ec.cmd));
                if let Some(w) = &ec.wrapper_injects {
                    lines.push(format!("    wrapper injects: {w}"));
                }
            }
            None => lines.push("    (unavailable)".to_string()),
        }
        lines.push("  port_reality:".to_string());
        if self.port_reality.is_empty() {
            lines.push("    (no declared ports)".to_string());
        } else {
            for p in &self.port_reality {
                lines.push(format!(
                    "    {}: declared by {}; host: {}; container: {}",
                    p.port, p.declared_by, p.host, p.container
                ));
            }
        }
        lines
    }

    fn has_double_bind(&self) -> bool {
        self.port_reality
            .iter()
            .any(|p| p.container.contains("twice"))
    }

    fn logs_show_port_conflict(&self) -> bool {
        self.recent_logs
            .iter()
            .any(|l| l.to_lowercase().contains("address already in use"))
    }
}

fn collect_diagnostic_bundle(
    runner: &dyn RemoteRunner,
    ns: &str,
    target: &SshTarget,
    container_name: &str,
    log_tail: u32,
) -> DiagnosticBundle {
    use crate::ssh::exec::RunOpts as SshRunOpts;
    let opts = SshRunOpts::with_timeout(15);
    let mut bundle = DiagnosticBundle::default();

    // ── 1/4: recent logs ────────────────────────────────────────────────
    let cmd = format!(
        "docker logs --tail {log_tail} {container_name} 2>&1 || true"
    );
    if let Ok(out) = runner.run(ns, target, &cmd, opts.clone()) {
        let body = if !out.stdout.is_empty() {
            &out.stdout
        } else {
            &out.stderr
        };
        for line in body.lines() {
            bundle.recent_logs.push(line.to_string());
        }
    }

    // ── 2/4: effective Cmd / Entrypoint / declared ports (single inspect) ─
    let cmd = format!(
        "docker inspect --format '{{{{json .Config.Cmd}}}}|{{{{json .Config.Entrypoint}}}}|{{{{json .HostConfig.PortBindings}}}}|{{{{json .Config.ExposedPorts}}}}' {container_name}"
    );
    let mut declared_by_config: BTreeSet<u16> = BTreeSet::new();
    let mut host_bindings: HashMap<u16, String> = HashMap::new();
    let mut exposed_only: BTreeSet<u16> = BTreeSet::new();
    if let Ok(out) = runner.run(ns, target, &cmd, opts.clone()) {
        if out.exit_code == 0 {
            let trimmed = out.stdout.trim();
            let parts: Vec<&str> = trimmed.split('|').collect();
            if parts.len() >= 4 {
                let cmd_v: Vec<String> =
                    serde_json::from_str(parts[0]).unwrap_or_default();
                let ep_v: Vec<String> =
                    serde_json::from_str(parts[1]).unwrap_or_default();
                if let Ok(pb) = serde_json::from_str::<serde_json::Value>(parts[2]) {
                    if let Some(map) = pb.as_object() {
                        for (k, v) in map {
                            if let Some(p) = parse_container_port(k) {
                                declared_by_config.insert(p);
                                if let Some(arr) = v.as_array() {
                                    if let Some(first) = arr.first() {
                                        let host_ip = first
                                            .get("HostIp")
                                            .and_then(|x| x.as_str())
                                            .unwrap_or("");
                                        let host_port = first
                                            .get("HostPort")
                                            .and_then(|x| x.as_str())
                                            .unwrap_or("");
                                        host_bindings
                                            .insert(p, format!("{host_ip}:{host_port}"));
                                    }
                                }
                            }
                        }
                    }
                }
                if let Ok(ep) = serde_json::from_str::<serde_json::Value>(parts[3]) {
                    if let Some(map) = ep.as_object() {
                        for k in map.keys() {
                            if let Some(p) = parse_container_port(k) {
                                if !declared_by_config.contains(&p) {
                                    exposed_only.insert(p);
                                }
                            }
                        }
                    }
                }
                bundle.effective_command = Some(EffectiveCommand {
                    cmd: cmd_v,
                    entrypoint: ep_v,
                    wrapper_injects: None,
                });
            }
        }
    }

    // ── 3/4: entrypoint script — wrapper-injection scan ─────────────────
    let cat_cmd = format!(
        "docker exec {container_name} cat /docker-entrypoint.sh 2>/dev/null \
         || docker exec {container_name} cat /entrypoint.sh 2>/dev/null \
         || true"
    );
    let mut entrypoint_injects: BTreeSet<u16> = BTreeSet::new();
    let mut wrapper_flag: Option<String> = None;
    if let Ok(out) = runner.run(ns, target, &cat_cmd, opts.clone()) {
        if !out.stdout.is_empty() {
            let prefixes = [
                "-dev-listen-address=",
                "-listen-address=",
                "-bind-address=",
                "-api-addr=",
                "--listen-address=",
            ];
            'outer: for line in out.stdout.lines() {
                for prefix in &prefixes {
                    if let Some(idx) = line.find(prefix) {
                        let rest = &line[idx..];
                        let end = rest
                            .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
                            .unwrap_or(rest.len());
                        let flag = &rest[..end];
                        wrapper_flag = Some(flag.to_string());
                        if let Some(p) = extract_port_from_addr(flag) {
                            entrypoint_injects.insert(p);
                        }
                        break 'outer;
                    }
                }
            }
        }
    }
    if let Some(ec) = bundle.effective_command.as_mut() {
        ec.wrapper_injects = wrapper_flag.clone();
    }

    // ── 4/4: host port reality ──────────────────────────────────────────
    let ss_cmd = "ss -ltn 2>/dev/null || netstat -ltn 2>/dev/null || true";
    let mut host_listening: BTreeSet<u16> = BTreeSet::new();
    if let Ok(out) = runner.run(ns, target, ss_cmd, opts.clone()) {
        for line in out.stdout.lines() {
            for tok in line.split_whitespace() {
                if let Some(colon) = tok.rfind(':') {
                    let port_str = &tok[colon + 1..];
                    if let Ok(p) = port_str.parse::<u16>() {
                        host_listening.insert(p);
                    }
                }
            }
        }
    }

    // ── Combine into port_reality rows ──────────────────────────────────
    let mut all_ports: BTreeSet<u16> = BTreeSet::new();
    all_ports.extend(declared_by_config.iter().copied());
    all_ports.extend(exposed_only.iter().copied());
    all_ports.extend(entrypoint_injects.iter().copied());
    for p in all_ports {
        let from_config = declared_by_config.contains(&p);
        let from_entry = entrypoint_injects.contains(&p);
        let declared_by = match (from_config, from_entry) {
            (true, true) => format!(
                "config + entrypoint {}",
                wrapper_flag.as_deref().unwrap_or("(injection)")
            ),
            (true, false) => "config".to_string(),
            (false, true) => format!(
                "entrypoint {}",
                wrapper_flag.as_deref().unwrap_or("(injection)")
            ),
            (false, false) => "exposed".to_string(),
        };
        let host = if host_listening.contains(&p) {
            host_bindings
                .get(&p)
                .map(|b| format!("bound→{b}"))
                .unwrap_or_else(|| "bound".to_string())
        } else {
            "free".to_string()
        };
        let container = if from_config && from_entry {
            "bound (twice!)".to_string()
        } else if from_config || from_entry {
            "bound".to_string()
        } else {
            "exposed".to_string()
        };
        bundle.port_reality.push(PortRow {
            port: p,
            host,
            container,
            declared_by,
        });
    }

    bundle
}

fn parse_container_port(spec: &str) -> Option<u16> {
    spec.split('/').next().and_then(|s| s.parse::<u16>().ok())
}

fn extract_port_from_addr(flag: &str) -> Option<u16> {
    let rhs = flag.split_once('=')?.1;
    let last = rhs.rsplit(':').next()?;
    last.trim_end_matches(|c: char| !c.is_ascii_digit())
        .parse::<u16>()
        .ok()
}
