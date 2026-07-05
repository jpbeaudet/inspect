//! `inspect volumes <sel>` — list docker volumes.

use anyhow::Result;

use crate::cli::SimpleSelectorArgs;
use crate::error::ExitKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::plan;
use crate::verbs::output::{Envelope, Renderer};

pub fn run(args: SimpleSelectorArgs) -> Result<ExitKind> {
    // K14 (v0.1.4): k8s "volumes" = PersistentVolumeClaims (kubectl get pvc).
    if let Some(ns_name) = args.selector.split('/').next() {
        if let Ok(resolved) = crate::config::resolver::resolve(ns_name) {
            if resolved.config.runtime_kind() == crate::exec::runtime::RuntimeKind::K8s {
                return volumes_k8s(&args, ns_name, &resolved.config);
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
            "docker volume ls --format '{{json .}}'",
            RunOpts::with_timeout(20),
        )?;
        if !out.ok() {
            if !args.format.is_json() {
                crate::tee_eprintln!(
                    "{}: docker volume ls failed (exit {}): {}",
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
            renderer.data_line(format!("{ns} | {name:<32} {driver}", ns = ns.namespace));
            renderer.push_row(
                &Envelope::new(&ns.namespace, "volume", format!("volume:{name}"))
                    .put("name", name)
                    .put("driver", driver)
                    .put("raw", v),
            );
        }
    }
    renderer.summary(format!("{count} volume(s)"));
    let fmt = args.format.resolve()?;
    let select = args.format.select_filter()?;
    renderer.dispatch(&fmt, select)
}

/// K14 (v0.1.4): `inspect volumes <k8s-ns>` — PersistentVolumeClaims
/// (`kubectl get pvc`): name, status, capacity, storageClass.
fn volumes_k8s(
    args: &SimpleSelectorArgs,
    ns: &str,
    cfg: &crate::config::namespace::NamespaceConfig,
) -> Result<ExitKind> {
    let mut renderer = Renderer::new();
    let out = crate::exec::kubectl::kubectl_base(cfg)
        .args(["get", "pvc", "-o", "json", "--request-timeout=10s"])
        .output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let f =
            crate::exec::kubectl::classify_kubectl_failure(&stderr, out.status.code().unwrap_or(1));
        crate::tee_eprintln!("volumes: [{}] {}", f.failure_class(), f.hint(""));
        return Ok(ExitKind::Inner(f.exit_code()));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    let empty = Vec::new();
    let items = v.get("items").and_then(|i| i.as_array()).unwrap_or(&empty);
    let mut count = 0usize;
    for pvc in items {
        count += 1;
        let name = pvc.pointer("/metadata/name").and_then(|x| x.as_str()).unwrap_or("");
        let status = pvc.pointer("/status/phase").and_then(|x| x.as_str()).unwrap_or("-");
        let cap = pvc
            .pointer("/status/capacity/storage")
            .and_then(|x| x.as_str())
            .unwrap_or("-");
        let sc = pvc
            .pointer("/spec/storageClassName")
            .and_then(|x| x.as_str())
            .unwrap_or("-");
        renderer.data_line(format!("{ns} | {name:<28} {status:<10} {cap:<8} {sc}"));
        renderer.push_row(
            &Envelope::new(ns, "volume", "pvc")
                .with_service(name)
                .put("status", status.to_string())
                .put("capacity", cap.to_string())
                .put("storage_class", sc.to_string()),
        );
    }
    renderer.summary(format!("{count} PVC(s)"));
    let fmt = args.format.resolve()?;
    renderer.dispatch(&fmt, args.format.select_filter()?)
}
