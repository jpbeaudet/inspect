//! `inspect resolve` — print the resolved targets for a selector.

use crate::cli::ResolveArgs;
use crate::error::ExitKind;
use crate::selector::resolve::{resolve, TargetKind};

pub fn run(args: ResolveArgs) -> anyhow::Result<ExitKind> {
    // F19 (v0.1.3): activate the FormatArgs mutex check
    // (e.g. `--select` without `--json` → exit 2).
    args.format.resolve()?;
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
        // F19 (v0.1.3): route through `print_json_value` so `--select`
        // applies to the resolved-targets array. Pre-fix this verb
        // pretty-printed via `to_string_pretty` + `println!`; the
        // `--select` filter compacts the rendered output (one line per
        // yielded value), which matches every other JSON-emitting
        // verb's filter contract.
        return crate::verbs::output::print_json_value(
            &serde_json::Value::Array(arr),
            args.format.select_spec(),
        );
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
