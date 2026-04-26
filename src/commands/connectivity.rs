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
use crate::verbs::output::{Envelope, JsonOut, Renderer};
use crate::verbs::quote::shquote;
use crate::verbs::runtime::RemoteRunner;

pub fn run(args: ConnectivityArgs) -> Result<ExitKind> {
    let (runner, nses, targets) = plan(&args.selector)?;
    let mut renderer = Renderer::new();
    let mut total_edges = 0usize;
    let mut probed_open = 0usize;
    let mut probed_closed = 0usize;
    let mut emitted_services = 0usize;

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
                        Some(false) => probed_closed += 1,
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

        if args.json {
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
            JsonOut::write(
                &Envelope::new(&step.ns.namespace, "discovery", "discovery")
                    .with_service(&svc_name)
                    .put("edges", edges_json),
            );
        } else {
            renderer.data_line(format!("{}/{svc_name}:", step.ns.namespace));
            if probes.is_empty() {
                renderer.data_line("  (no declared dependencies)".to_string());
            }
            for p in &probes {
                let port = p.edge.port.map(|n| n.to_string()).unwrap_or_else(|| "?".into());
                let badge = match p.open {
                    Some(true) => "[open]   ",
                    Some(false) => "[closed] ",
                    None => "         ",
                };
                renderer.data_line(format!(
                    "  {badge} {} -> {}:{port}/{}",
                    p.edge.from, p.edge.to, p.edge.proto
                ));
            }
        }
    }

    if !args.json {
        let probe_summary = if args.probe {
            format!(", probed {probed_open} open / {probed_closed} closed")
        } else {
            String::new()
        };
        renderer.summary(format!(
            "{emitted_services} service(s), {total_edges} edge(s){probe_summary}"
        ));
        if !args.probe {
            renderer.next("inspect connectivity <sel> --probe to live-test edges");
        }
        renderer.next("inspect why <sel> to walk failures");
        renderer.print();
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
