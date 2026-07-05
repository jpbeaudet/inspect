//! `inspect describe <k8s-ns>/<pod>` — deep pod dump reshaped into the JSON
//! envelope (K11, v0.1.4).
//!
//! kubectl `describe` is text-only (no `-o json`) — its richest view (spec +
//! status + conditions) can't be projected or piped. inspect reshapes
//! `kubectl get pod -o json` + the object's Events into the standard envelope,
//! so an agent can `--select` any field. A Kubernetes verb.

use anyhow::Result;

use crate::cli::SimpleSelectorArgs;
use crate::error::ExitKind;
use crate::verbs::output::OutputDoc;

pub fn run(args: SimpleSelectorArgs) -> Result<ExitKind> {
    let ns_name = args.selector.split('/').next().unwrap_or("");
    let resolved = crate::config::resolver::resolve(ns_name)?;
    if resolved.config.runtime_kind() != crate::exec::runtime::RuntimeKind::K8s {
        crate::error::emit(
            "describe is a Kubernetes verb. For docker use `inspect ps` / `inspect why`.",
        );
        return Ok(ExitKind::Error);
    }
    describe_k8s(&args, ns_name, &resolved.config)
}

fn describe_k8s(
    args: &SimpleSelectorArgs,
    ns: &str,
    cfg: &crate::config::namespace::NamespaceConfig,
) -> Result<ExitKind> {
    let fmt = args.format.resolve()?;
    let pod = args
        .selector
        .split_once('/')
        .map(|(_, r)| r.split(':').next().unwrap_or("").to_string())
        .unwrap_or_default();
    if pod.is_empty() {
        crate::tee_eprintln!("describe: specify a pod — `inspect describe {ns}/<pod>`");
        return Ok(ExitKind::Error);
    }

    let out = crate::exec::kubectl::kubectl_base(cfg)
        .args(["get", "pod", &pod, "-o", "json", "--request-timeout=10s"])
        .output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let f =
            crate::exec::kubectl::classify_kubectl_failure(&stderr, out.status.code().unwrap_or(1));
        crate::tee_eprintln!("describe: [{}] {}", f.failure_class(), f.hint(""));
        return Ok(ExitKind::Inner(f.exit_code()));
    }
    let obj: serde_json::Value =
        serde_json::from_slice(&out.stdout).unwrap_or(serde_json::Value::Null);

    // Human summary of the load-bearing fields (the rest is in --json).
    let phase = obj
        .pointer("/status/phase")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown");
    let node = obj
        .pointer("/spec/nodeName")
        .and_then(|v| v.as_str())
        .unwrap_or("<unscheduled>");
    let pod_ip = obj
        .pointer("/status/podIP")
        .and_then(|v| v.as_str())
        .unwrap_or("-");
    let mut data_lines = vec![
        format!("pod:    {pod}"),
        format!("phase:  {phase}"),
        format!("node:   {node}"),
        format!("podIP:  {pod_ip}"),
    ];
    if let Some(containers) = obj.pointer("/spec/containers").and_then(|v| v.as_array()) {
        for c in containers {
            let name = c.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let image = c.get("image").and_then(|v| v.as_str()).unwrap_or("?");
            data_lines.push(format!("  container {name}: {image}"));
        }
    }
    if let Some(conds) = obj.pointer("/status/conditions").and_then(|v| v.as_array()) {
        for cd in conds {
            let t = cd.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let s = cd.get("status").and_then(|v| v.as_str()).unwrap_or("");
            data_lines.push(format!("  condition {t}={s}"));
        }
    }

    let summary = format!("describe {ns}/{pod} (phase {phase}, node {node})");
    let doc = OutputDoc::new(summary, serde_json::json!({ "pod": obj }))
        .with_meta("selector", args.selector.clone())
        .with_meta("runtime", "k8s".to_string())
        .with_quiet(args.format.quiet);
    crate::format::render::render_doc(&doc, &fmt, &data_lines, args.format.select_spec())
}
