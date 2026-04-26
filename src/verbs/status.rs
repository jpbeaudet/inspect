//! `inspect status <sel>` — service inventory + health rollup.
//!
//! Reads from the cached profile (no remote round-trip required), then
//! optionally reconciles with a live `docker ps` per namespace to mark any
//! profile services that no longer exist as `down`.

use anyhow::Result;
use serde_json::{json, Value};

use crate::cli::StatusArgs;
use crate::error::ExitKind;
use crate::profile::schema::HealthStatus;
use crate::ssh::exec::RunOpts;
use crate::verbs::correlation::{status_rules, StatusRow};
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::OutputDoc;

pub fn run(args: StatusArgs) -> Result<ExitKind> {
    let (runner, nses, targets) = plan(&args.selector)?;

    let mut total = 0usize;
    let mut healthy = 0usize;
    let mut unhealthy = 0usize;
    let mut unknown = 0usize;

    let mut data_lines: Vec<String> = Vec::new();
    let mut services_json: Vec<Value> = Vec::new();
    let mut rows: Vec<StatusRow> = Vec::new();

    // Optionally reconcile with live state to detect down-but-cached services.
    let mut live_running: std::collections::HashMap<String, std::collections::HashSet<String>> =
        std::collections::HashMap::new();
    for ns in &nses {
        let out = runner
            .run(
                &ns.namespace,
                &ns.target,
                "docker ps --format '{{.Names}}'",
                RunOpts::with_timeout(15),
            )
            .ok();
        if let Some(o) = out {
            if o.ok() {
                let set: std::collections::HashSet<String> =
                    o.stdout.lines().map(|s| s.to_string()).collect();
                live_running.insert(ns.namespace.clone(), set);
            }
        }
    }

    for step in iter_steps(&nses, &targets) {
        let svc_name = match step.service() {
            Some(n) => n,
            None => continue, // host-level — nothing to report here
        };
        let svc_def = step.service_def();
        let mut status_str = "unknown".to_string();
        if let Some(def) = svc_def {
            let live_up = live_running
                .get(&step.ns.namespace)
                .map(|set| set.contains(&def.name))
                .unwrap_or(true);
            status_str = if !live_up {
                "down".to_string()
            } else {
                match def.health_status {
                    Some(HealthStatus::Ok) => "ok".into(),
                    Some(HealthStatus::Unhealthy) => "unhealthy".into(),
                    Some(HealthStatus::Starting) => "starting".into(),
                    Some(HealthStatus::Unknown) | None => "unknown".into(),
                }
            };
        }
        total += 1;
        match status_str.as_str() {
            "ok" => healthy += 1,
            "unhealthy" | "down" => unhealthy += 1,
            _ => unknown += 1,
        }
        let img = svc_def.and_then(|s| s.image.clone()).unwrap_or_default();
        rows.push(StatusRow {
            server: step.ns.namespace.clone(),
            service: svc_name.to_string(),
            status: status_str.clone(),
        });
        services_json.push(json!({
            "server": step.ns.namespace,
            "service": svc_name,
            "status": status_str,
            "image": img,
        }));
        data_lines.push(format!(
            "{ns}/{svc_name:<20} {status_str:<10} {img}",
            ns = step.ns.namespace
        ));
    }

    let summary = format!(
        "{total} service(s): {healthy} healthy, {unhealthy} unhealthy, {unknown} unknown"
    );
    let mut doc = OutputDoc::new(
        summary,
        json!({
            "services": services_json,
            "totals": {
                "total": total,
                "healthy": healthy,
                "unhealthy": unhealthy,
                "unknown": unknown,
            }
        }),
    )
    .with_meta("selector", args.selector.clone());
    for n in status_rules(&rows) {
        doc.push_next(n);
    }

    if args.json {
        doc.print_json();
    } else {
        doc.print_human(&data_lines);
    }

    Ok(ExitKind::Success)
}
