//! `inspect top <k8s-ns>[/<pod>]` — pod CPU/memory usage (K13, v0.1.4).
//!
//! A Kubernetes verb: runs `kubectl top pods` (context-pinned). When
//! metrics-server is absent it degrades cleanly to `metrics_unavailable`
//! (exit 15, WA-4) with an install hint — never a raw kubectl error. For a
//! docker namespace it refuses with a pointer to `inspect status`/`health`.

use anyhow::Result;

use crate::cli::SimpleSelectorArgs;
use crate::error::ExitKind;

pub fn run(args: SimpleSelectorArgs) -> Result<ExitKind> {
    let ns_name = args.selector.split('/').next().unwrap_or("");
    let resolved = crate::config::resolver::resolve(ns_name)?;
    if resolved.config.runtime_kind() != crate::exec::runtime::RuntimeKind::K8s {
        crate::error::emit(
            "top is a Kubernetes verb (pod CPU/memory via metrics-server). For a \
             docker namespace use `inspect status` / `inspect health`.",
        );
        return Ok(ExitKind::Error);
    }
    top_k8s(&args, ns_name, &resolved.config)
}

fn top_k8s(
    args: &SimpleSelectorArgs,
    ns: &str,
    cfg: &crate::config::namespace::NamespaceConfig,
) -> Result<ExitKind> {
    // Optional pod filter: `<ns>/<pod>`.
    let pod = args.selector.split_once('/').map(|(_, r)| r.to_string());

    let mut cmd = crate::exec::kubectl::kubectl_base(cfg);
    cmd.args(["top", "pods", "--no-headers", "--request-timeout=10s"]);
    if let Some(p) = pod.as_deref() {
        cmd.arg(p);
    }
    let out = cmd.output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let f = crate::exec::kubectl::classify_kubectl_failure(&stderr, out.status.code().unwrap_or(1));
        // metrics-server absent → the K13/WA-4 degrade path (exit 15).
        crate::tee_eprintln!("top: [{}] {}", f.failure_class(), f.hint(""));
        return Ok(ExitKind::Inner(f.exit_code()));
    }

    let as_json = args.format.is_json();
    let mut rows = 0usize;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.is_empty() {
            continue;
        }
        rows += 1;
        // `kubectl top pods --no-headers`: NAME  CPU(cores)  MEMORY(bytes)
        let name = cols.first().copied().unwrap_or("");
        let cpu = cols.get(1).copied().unwrap_or("");
        let mem = cols.get(2).copied().unwrap_or("");
        if as_json {
            println!(
                "{}",
                serde_json::json!({ "server": ns, "pod": name, "cpu": cpu, "memory": mem })
            );
        } else {
            println!("{ns}/{name:<40} cpu={cpu:<8} mem={mem}");
        }
    }
    if !as_json {
        println!("SUMMARY: {rows} pod(s)");
    }
    Ok(ExitKind::Success)
}
