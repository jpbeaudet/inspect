//! `inspect events <k8s-ns>[/<pod>]` — cluster/object events, newest-first
//! (K12, v0.1.4).
//!
//! A Kubernetes verb. `kubectl get events` is notoriously NOT chronologically
//! ordered by default (the #1 events complaint, research w3-P7); inspect
//! always sorts **newest-first** and, when a pod is given, auto-scopes with a
//! field-selector so the operator never writes the incantation. Feeds
//! `inspect why`.

use anyhow::Result;

use crate::cli::SimpleSelectorArgs;
use crate::error::ExitKind;

pub fn run(args: SimpleSelectorArgs) -> Result<ExitKind> {
    let ns_name = args.selector.split('/').next().unwrap_or("");
    let resolved = crate::config::resolver::resolve(ns_name)?;
    if resolved.config.runtime_kind() != crate::exec::runtime::RuntimeKind::K8s {
        crate::error::emit(
            "events is a Kubernetes verb. For docker use `inspect logs` / `inspect why`.",
        );
        return Ok(ExitKind::Error);
    }
    events_k8s(&args, ns_name, &resolved.config)
}

fn events_k8s(
    args: &SimpleSelectorArgs,
    ns: &str,
    cfg: &crate::config::namespace::NamespaceConfig,
) -> Result<ExitKind> {
    let pod = args.selector.split_once('/').map(|(_, r)| r.to_string());

    let mut cmd = crate::exec::kubectl::kubectl_base(cfg);
    cmd.args([
        "get",
        "events",
        "--sort-by=.lastTimestamp",
        "-o",
        "json",
        "--request-timeout=10s",
    ]);
    if let Some(p) = pod.as_deref() {
        cmd.args(["--field-selector", &format!("involvedObject.name={p}")]);
    }
    let out = cmd.output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let f =
            crate::exec::kubectl::classify_kubectl_failure(&stderr, out.status.code().unwrap_or(1));
        crate::tee_eprintln!("events: [{}] {}", f.failure_class(), f.hint(""));
        return Ok(ExitKind::Inner(f.exit_code()));
    }

    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or(serde_json::Value::Null);
    let empty = Vec::new();
    let items = v.get("items").and_then(|i| i.as_array()).unwrap_or(&empty);
    let as_json = args.format.is_json();
    let mut warnings = 0usize;

    // `--sort-by=.lastTimestamp` is oldest-first; reverse for newest-first.
    for e in items.iter().rev() {
        let typ = e.get("type").and_then(|x| x.as_str()).unwrap_or("Normal");
        let reason = e.get("reason").and_then(|x| x.as_str()).unwrap_or("");
        let msg = e.get("message").and_then(|x| x.as_str()).unwrap_or("");
        let obj = e
            .pointer("/involvedObject/name")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        let last = e
            .get("lastTimestamp")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        if typ == "Warning" {
            warnings += 1;
        }
        if as_json {
            println!(
                "{}",
                serde_json::json!({
                    "server": ns, "type": typ, "reason": reason,
                    "object": obj, "message": msg, "last_timestamp": last,
                })
            );
        } else {
            println!("[{typ:<7}] {reason:<22} {obj:<32} {msg}");
        }
    }

    if !as_json {
        println!(
            "SUMMARY: {} event(s), {warnings} warning(s) (newest-first). \
             Note: events expire (~1h retention).",
            items.len()
        );
    }
    Ok(ExitKind::Success)
}
