//! `inspect resolve` — print the resolved targets for a selector.

use crate::cli::ResolveArgs;
use crate::error::ExitKind;
use crate::selector::resolve::{resolve, TargetKind};

pub fn run(args: ResolveArgs) -> anyhow::Result<ExitKind> {
    let targets = resolve(&args.selector)?;

    if args.format.is_json() {
        let arr: Vec<_> = targets
            .iter()
            .map(|t| {
                let (kind, service) = match &t.kind {
                    TargetKind::Service { name } => ("service", Some(name.clone())),
                    TargetKind::Host => ("host", None),
                };
                serde_json::json!({
                    "namespace": t.namespace,
                    "kind": kind,
                    "service": service,
                    "path": t.path,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::Value::Array(arr))?
        );
        return Ok(ExitKind::Success);
    }

    println!("SUMMARY: selector resolved to {} target(s)", targets.len());
    println!("DATA:");
    for t in &targets {
        let target = match &t.kind {
            TargetKind::Service { name } => format!("service={name}"),
            TargetKind::Host => "host".to_string(),
        };
        let path = t
            .path
            .as_deref()
            .map(|p| format!(" path={p}"))
            .unwrap_or_default();
        println!("  {} -> {target}{path}", t.namespace);
    }
    println!("NEXT:    point a verb (e.g. logs, status) at the same selector");
    Ok(ExitKind::Success)
}
