//! `inspect top <k8s-ns>[/<pod>]` — pod CPU/memory usage (K13, v0.1.4).
//!
//! A Kubernetes verb: runs `kubectl top pods` (context-pinned). When
//! metrics-server is absent it degrades cleanly to `metrics_unavailable`
//! (exit 15, WA-4) with an install hint — never a raw kubectl error. For a
//! docker namespace it refuses with a pointer to `inspect status`/`health`.

use anyhow::Result;

use crate::cli::SimpleSelectorArgs;
use crate::error::ExitKind;
use crate::verbs::output::OutputDoc;

pub fn run(args: SimpleSelectorArgs) -> Result<ExitKind> {
    let Some((ns_name, cfg)) = crate::verbs::dispatch::require_k8s(
        &args.selector,
        "top is a Kubernetes verb (pod CPU/memory via metrics-server). For a \
         docker namespace use `inspect status` / `inspect health`.",
    )?
    else {
        return Ok(ExitKind::Error);
    };
    top_k8s(&args, &ns_name, &cfg)
}

fn top_k8s(
    args: &SimpleSelectorArgs,
    ns: &str,
    cfg: &crate::config::namespace::NamespaceConfig,
) -> Result<ExitKind> {
    // Optional pod filter: `<ns>/<pod>`.
    let pod = args.selector.split_once('/').map(|(_, r)| r.to_string());

    let mut cmd = crate::exec::kubectl::kubectl_base(cfg);
    cmd.args([
        "top",
        "pods",
        "--no-headers",
        crate::exec::kubectl::READ_REQUEST_TIMEOUT,
    ]);
    if let Some(p) = pod.as_deref() {
        cmd.arg(p);
    }
    let out = cmd.output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let f =
            crate::exec::kubectl::classify_kubectl_failure(&stderr, out.status.code().unwrap_or(1));
        // metrics-server absent → the K13/WA-4 degrade path (exit 15).
        crate::tee_eprintln!("top: [{}] {}", f.failure_class(), f.hint(""));
        return Ok(ExitKind::Inner(f.exit_code()));
    }

    // H4/R1: emit the standard envelope (`.data.pods[]`) + honor `--select`,
    // matching the other snapshot read verbs (ps / status / describe) — not a
    // bare per-line `json!` stream that silently ignores projection.
    let fmt = args.format.resolve()?;
    let mut pods: Vec<serde_json::Value> = Vec::new();
    let mut data_lines: Vec<String> = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.is_empty() {
            continue;
        }
        // `kubectl top pods --no-headers`: NAME  CPU(cores)  MEMORY(bytes)
        let name = cols.first().copied().unwrap_or("");
        let cpu = cols.get(1).copied().unwrap_or("");
        let mem = cols.get(2).copied().unwrap_or("");
        pods.push(serde_json::json!({
            "server": ns, "pod": name, "cpu": cpu, "memory": mem
        }));
        data_lines.push(format!("{ns}/{name:<40} cpu={cpu:<8} mem={mem}"));
    }
    let summary = format!("{} pod(s)", pods.len());
    let doc = OutputDoc::new(summary, serde_json::json!({ "pods": pods }))
        .with_meta("selector", args.selector.clone())
        .with_meta("runtime", "k8s".to_string())
        .with_quiet(args.format.quiet);
    crate::format::render::render_doc(&doc, &fmt, &data_lines, args.format.select_spec())
}
