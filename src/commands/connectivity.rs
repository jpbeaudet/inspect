//! `inspect connectivity <selector>` — connectivity matrix (bible §12.3).
//!
//! Renders the dependency edge list from the cached profile. With
//! `--probe`, each `service → dep:port` edge is verified by running
//! `bash -c '(echo > /dev/tcp/<host>/<port>) 2>/dev/null'` from the
//! source service's namespace. We intentionally avoid `nc`/`ncat`
//! because availability varies wildly across distros — `/dev/tcp` is
//! a bash builtin always available where bash is.

use std::collections::HashMap;

use anyhow::Result;

use crate::cli::ConnectivityArgs;
use crate::error::ExitKind;
use crate::profile::schema::{Profile, Service};
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan, NsCtx};
use crate::verbs::output::OutputDoc;
use crate::verbs::quote::shquote;
use crate::verbs::runtime::RemoteRunner;

pub fn run(args: ConnectivityArgs) -> Result<ExitKind> {
    let (runner, nses, targets) = plan(&args.selector)?;
    let mut data_lines: Vec<String> = Vec::new();
    let mut services_json: Vec<serde_json::Value> = Vec::new();
    let mut total_edges = 0usize;
    let mut probed_open = 0usize;
    let mut probed_closed = 0usize;
    let mut emitted_services = 0usize;
    let mut closed_edge: Option<(String, String, String)> = None;

    for step in iter_steps(&nses, &targets) {
        let svc_name = match step.service() {
            Some(n) => n.to_string(),
            None => continue,
        };
        let profile = match step.ns.profile.as_ref() {
            Some(p) => p,
            None => continue,
        };
        emitted_services += 1;
        let edges = collect_edges(profile, &svc_name);
        total_edges += edges.len();
        let probes: Vec<EdgeProbe> = if args.probe {
            edges
                .iter()
                .map(|e| {
                    let p = probe_edge(runner.as_ref(), step.ns, e);
                    match p {
                        Some(true) => probed_open += 1,
                        Some(false) => {
                            probed_closed += 1;
                            if closed_edge.is_none() {
                                closed_edge = Some((
                                    step.ns.namespace.clone(),
                                    e.from.clone(),
                                    e.to.clone(),
                                ));
                            }
                        }
                        None => {}
                    }
                    EdgeProbe { edge: e.clone(), open: p }
                })
                .collect()
        } else {
            edges
                .iter()
                .map(|e| EdgeProbe { edge: e.clone(), open: None })
                .collect()
        };

        let edges_json: Vec<serde_json::Value> = probes
            .iter()
            .map(|p| {
                serde_json::json!({
                    "from": p.edge.from,
                    "to": p.edge.to,
                    "to_host": p.edge.to_host,
                    "port": p.edge.port,
                    "proto": p.edge.proto,
                    "probed": match p.open {
                        Some(true) => "open",
                        Some(false) => "closed",
                        None => "skipped",
                    },
                })
            })
            .collect();
        services_json.push(serde_json::json!({
            "server": step.ns.namespace,
            "service": svc_name,
            "edges": edges_json,
        }));

        data_lines.push(format!("{}/{svc_name}:", step.ns.namespace));
        if probes.is_empty() {
            data_lines.push("  (no declared dependencies)".to_string());
        }
        for p in &probes {
            let port = p.edge.port.map(|n| n.to_string()).unwrap_or_else(|| "?".into());
            let badge = match p.open {
                Some(true) => "[open]   ",
                Some(false) => "[closed] ",
                None => "         ",
            };
            data_lines.push(format!(
                "  {badge} {} -> {}:{port}/{}",
                p.edge.from, p.edge.to, p.edge.proto
            ));
        }
    }

    let probe_summary = if args.probe {
        format!(", probed {probed_open} open / {probed_closed} closed")
    } else {
        String::new()
    };
    let summary = format!(
        "{emitted_services} service(s), {total_edges} edge(s){probe_summary}"
    );
    let mut doc = OutputDoc::new(
        summary,
        serde_json::json!({
            "services": services_json,
            "totals": {
                "services": emitted_services,
                "edges": total_edges,
                "open": probed_open,
                "closed": probed_closed,
                "probed": args.probe,
            },
        }),
    )
    .with_meta("selector", args.selector.clone());
    if !args.probe {
        doc.push_next(crate::verbs::output::NextStep::new(
            format!("inspect connectivity {} --probe", args.selector),
            "live-test edges with /dev/tcp probes".to_string(),
        ));
    }
    if let Some((server, _from, to)) = &closed_edge {
        doc.push_next(crate::verbs::output::NextStep::new(
            format!("inspect why {server}/{to}"),
            format!("dep '{to}' is unreachable; walk its state"),
        ));
    }

    if args.json {
        doc.print_json();
    } else {
        doc.print_human(&data_lines);
    }

    Ok(if args.probe && probed_closed > 0 {
        ExitKind::Error
    } else {
        ExitKind::Success
    })
}

#[derive(Debug, Clone)]
struct Edge {
    from: String,
    to: String,
    to_host: String,
    port: Option<u16>,
    proto: String,
}

struct EdgeProbe {
    edge: Edge,
    /// `Some(true)` open, `Some(false)` closed, `None` not probed.
    open: Option<bool>,
}

fn collect_edges(profile: &Profile, root: &str) -> Vec<Edge> {
    let by_name: HashMap<&str, &Service> =
        profile.services.iter().map(|s| (s.name.as_str(), s)).collect();
    let svc = match by_name.get(root) {
        Some(s) => *s,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for dep in &svc.depends_on {
        let dep_svc = by_name.get(dep.as_str()).copied();
        let (port, proto) = dep_svc
            .and_then(|d| d.ports.first())
            .map(|p| (Some(p.container), p.proto.clone()))
            .unwrap_or((None, "tcp".to_string()));
        out.push(Edge {
            from: root.to_string(),
            to: dep.clone(),
            // In a single namespace, services live on the same host;
            // assume the container name is reachable as a hostname (common
            // in compose / docker DNS). Falls back to localhost otherwise.
            to_host: dep.clone(),
            port,
            proto,
        });
    }
    out
}

fn probe_edge(runner: &dyn RemoteRunner, ns: &NsCtx, edge: &Edge) -> Option<bool> {
    let port = edge.port?;
    // bash builtin /dev/tcp probe — small, dependency-free, ubiquitous.
    let cmd = format!(
        "bash -c {} || true",
        shquote(&format!(
            "(echo > /dev/tcp/{}/{}) 2>/dev/null && echo open || echo closed",
            edge.to_host, port
        ))
    );
    let out = runner.run(&ns.namespace, &ns.target, &cmd, RunOpts::with_timeout(5)).ok()?;
    let stdout = out.stdout.trim();
    if stdout.contains("open") {
        Some(true)
    } else if stdout.contains("closed") {
        Some(false)
    } else {
        None
    }
}
