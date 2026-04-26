//! `inspect health <sel>` — per-service health probe.
//!
//! Strategy: if the cached profile has a `health` URL for the service, run
//! a remote `curl -fsS -m 3 <url>`. Otherwise report the cached
//! `health_status` if any. Host-level targets get a basic `uptime` probe.

use anyhow::Result;
use serde_json::{json, Value};

use crate::cli::HealthArgs;
use crate::error::ExitKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::correlation::{status_rules, StatusRow};
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::OutputDoc;
use crate::verbs::quote::shquote;

pub fn run(args: HealthArgs) -> Result<ExitKind> {
    let (runner, nses, targets) = plan(&args.selector)?;
    let mut data_lines: Vec<String> = Vec::new();
    let mut probes_json: Vec<Value> = Vec::new();
    let mut rows: Vec<StatusRow> = Vec::new();
    let mut total = 0usize;
    let mut ok = 0usize;
    let mut bad = 0usize;

    for step in iter_steps(&nses, &targets) {
        total += 1;
        let svc = step.service().unwrap_or("_").to_string();
        let svc_def = step.service_def();
        let url = svc_def.and_then(|s| s.health.clone());

        let result = match url.as_deref() {
            Some(u) => {
                let cmd = format!(
                    "curl -fsS -m 3 -o /dev/null -w '%{{http_code}}' {} || true",
                    shquote(u)
                );
                let out = runner.run(
                    &step.ns.namespace,
                    &step.ns.target,
                    &cmd,
                    RunOpts::with_timeout(10),
                )?;
                let code = out.stdout.trim().to_string();
                let healthy = code.starts_with('2') || code.starts_with('3');
                ProbeResult {
                    healthy,
                    detail: format!("HTTP {code}"),
                    url: Some(u.to_string()),
                }
            }
            None => {
                let marker = svc_def
                    .and_then(|s| s.health_status)
                    .map(|s| format!("{s:?}"))
                    .unwrap_or_else(|| "unknown".to_string());
                ProbeResult {
                    healthy: marker == "Ok",
                    detail: format!("cached: {marker}"),
                    url: None,
                }
            }
        };
        if result.healthy {
            ok += 1;
        } else {
            bad += 1;
        }
        rows.push(StatusRow {
            server: step.ns.namespace.clone(),
            service: svc.clone(),
            status: if result.healthy { "ok".into() } else { "unhealthy".into() },
        });
        probes_json.push(json!({
            "server": step.ns.namespace,
            "service": svc,
            "healthy": result.healthy,
            "detail": result.detail,
            "probe_url": result.url,
        }));
        let badge = if result.healthy { "OK " } else { "BAD" };
        data_lines.push(format!(
            "[{badge}] {ns}/{svc:<20} {detail}",
            ns = step.ns.namespace,
            detail = result.detail
        ));
    }

    let summary = format!("{total} probe(s): {ok} ok, {bad} not-ok");
    let mut doc = OutputDoc::new(
        summary,
        json!({
            "probes": probes_json,
            "totals": { "total": total, "ok": ok, "bad": bad },
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

struct ProbeResult {
    healthy: bool,
    detail: String,
    url: Option<String>,
}
