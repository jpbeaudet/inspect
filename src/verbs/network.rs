//! `inspect network <sel>` — list docker networks.

use anyhow::Result;

use crate::cli::SimpleSelectorArgs;
use crate::error::ExitKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::plan;
use crate::verbs::output::{Envelope, Renderer};

pub fn run(args: SimpleSelectorArgs) -> Result<ExitKind> {
    // K14 (v0.1.4): k8s "network" = Services (kubectl get svc), not docker nets.
    if let Some(ns_name) = args.selector.split('/').next() {
        if let Ok(resolved) = crate::config::resolver::resolve(ns_name) {
            if resolved.config.runtime_kind() == crate::exec::runtime::RuntimeKind::K8s {
                return network_k8s(&args, ns_name, &resolved.config);
            }
        }
    }

    let (runner, nses, _) = plan(&args.selector)?;
    let mut renderer = Renderer::new();
    let mut count = 0usize;
    for ns in &nses {
        let out = runner.run(
            &ns.namespace,
            &ns.target,
            "docker network ls --format '{{json .}}'",
            RunOpts::with_timeout(20),
        )?;
        if !out.ok() {
            if !args.format.is_json() {
                crate::tee_eprintln!(
                    "{}: docker network ls failed (exit {}): {}",
                    ns.namespace,
                    out.exit_code,
                    out.stderr.trim()
                );
            }
            continue;
        }
        for line in out.stdout.lines() {
            count += 1;
            let v: serde_json::Value = serde_json::from_str(line).unwrap_or_default();
            let name = v
                .get("Name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let driver = v
                .get("Driver")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let scope = v
                .get("Scope")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            renderer.data_line(format!(
                "{ns} | {name:<24} {driver:<12} {scope}",
                ns = ns.namespace
            ));
            renderer.push_row(
                &Envelope::new(&ns.namespace, "network", format!("network:{name}"))
                    .put("name", name)
                    .put("driver", driver)
                    .put("scope", scope)
                    .put("raw", v),
            );
        }
    }
    renderer.summary(format!("{count} network(s)"));
    let fmt = args.format.resolve()?;
    let select = args.format.select_filter()?;
    renderer.dispatch(&fmt, select)
}

/// K14 (v0.1.4): `inspect network <k8s-ns>` — Services in the namespace
/// (`kubectl get svc`): name, type, clusterIP, and ports. The k8s analogue
/// of docker networks — Services are the reachability layer.
fn network_k8s(
    args: &SimpleSelectorArgs,
    ns: &str,
    cfg: &crate::config::namespace::NamespaceConfig,
) -> Result<ExitKind> {
    let mut renderer = Renderer::new();
    let out = crate::exec::kubectl::kubectl_base(cfg)
        .args(["get", "svc", "-o", "json", "--request-timeout=10s"])
        .output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let f =
            crate::exec::kubectl::classify_kubectl_failure(&stderr, out.status.code().unwrap_or(1));
        crate::tee_eprintln!("network: [{}] {}", f.failure_class(), f.hint(""));
        return Ok(ExitKind::Inner(f.exit_code()));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    let empty = Vec::new();
    let items = v.get("items").and_then(|i| i.as_array()).unwrap_or(&empty);
    let mut count = 0usize;
    for s in items {
        count += 1;
        let name = s
            .pointer("/metadata/name")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        let typ = s
            .pointer("/spec/type")
            .and_then(|x| x.as_str())
            .unwrap_or("ClusterIP");
        let cip = s
            .pointer("/spec/clusterIP")
            .and_then(|x| x.as_str())
            .unwrap_or("-");
        let ports: Vec<String> = s
            .pointer("/spec/ports")
            .and_then(|p| p.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|p| {
                        let port = p.get("port").and_then(|x| x.as_u64()).unwrap_or(0);
                        let proto = p.get("protocol").and_then(|x| x.as_str()).unwrap_or("TCP");
                        format!("{port}/{proto}")
                    })
                    .collect()
            })
            .unwrap_or_default();
        let ports_str = ports.join(",");
        renderer.data_line(format!("{ns} | {name:<28} {typ:<12} {cip:<16} {ports_str}"));
        renderer.push_row(
            &Envelope::new(ns, "network", "service")
                .with_service(name)
                .put("type", typ.to_string())
                .put("cluster_ip", cip.to_string())
                .put("ports", ports),
        );
    }
    renderer.summary(format!("{count} service(s)"));
    let fmt = args.format.resolve()?;
    renderer.dispatch(&fmt, args.format.select_filter()?)
}
