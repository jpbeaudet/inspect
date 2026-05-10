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
use crate::verbs::cache::{aggregate_sources, get_runtime, print_source_line, GetOpts};
use crate::verbs::correlation::{status_rules, StatusRow};
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::OutputDoc;
use crate::verbs::quote::shquote;

pub fn run(args: HealthArgs) -> Result<ExitKind> {
    let (runner, nses, targets) = plan(&args.selector)?;

    // Refresh / consult the runtime cache so that the
    // cached-health fallback (services without a probe URL) reports
    // the freshest known health_status — and so the SOURCE: line
    // tells operators whether they're seeing live or stale runtime
    // facts. The actual probe (curl) remains unconditionally live;
    // SOURCE describes the *cache state* used as fallback.
    let opts = GetOpts {
        force_refresh: args.refresh,
    };
    let mut runtime_by_ns: std::collections::HashMap<
        String,
        crate::profile::runtime::RuntimeSnapshot,
    > = std::collections::HashMap::new();
    let mut sources: Vec<crate::profile::runtime::SourceInfo> = Vec::new();
    let mut refresh_warnings: Vec<String> = Vec::new();
    for ns in &nses {
        match get_runtime(runner.as_ref(), ns, opts) {
            Ok((snap, info)) => {
                if info.stale {
                    if let Some(reason) = &info.reason {
                        refresh_warnings.push(format!(
                            "{}: serving cached data — {}",
                            ns.namespace, reason
                        ));
                    }
                }
                runtime_by_ns.insert(ns.namespace.clone(), snap);
                sources.push(info);
            }
            Err(_) => {
                sources.push(crate::profile::runtime::SourceInfo {
                    mode: crate::profile::runtime::SourceMode::Stale,
                    runtime_age_s: None,
                    inventory_age_s: crate::profile::runtime::inventory_age(&ns.namespace)
                        .map(|d| d.as_secs()),
                    stale: true,
                    reason: Some(format!(
                        "{}: runtime refresh failed (no cache)",
                        ns.namespace
                    )),
                });
            }
        }
    }
    let aggregated_source = aggregate_sources(&sources);
    let fmt = args.format.resolve()?;
    print_source_line(&aggregated_source, &fmt);

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
                // Prefer runtime snapshot's health_status over the
                // inventory tier's frozen-at-setup value.
                let rt_health = svc_def.and_then(|d| {
                    runtime_by_ns
                        .get(&step.ns.namespace)
                        .and_then(|s| s.lookup(&d.container_name))
                        .and_then(|r| r.health_status)
                });
                let marker = rt_health
                    .or_else(|| svc_def.and_then(|s| s.health_status))
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
            status: if result.healthy {
                "ok".into()
            } else {
                "unhealthy".into()
            },
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
    .with_meta("selector", args.selector.clone())
    .with_meta("source", aggregated_source.to_json())
    .with_quiet(args.format.quiet);
    for n in status_rules(&rows) {
        doc.push_next(n);
    }
    if aggregated_source.stale {
        doc.push_next(crate::verbs::output::NextStep::new(
            format!(
                "inspect connectivity {}",
                nses.first()
                    .map(|n| n.namespace.clone())
                    .unwrap_or_else(|| "<ns>".to_string())
            ),
            "diagnose why runtime refresh failed",
        ));
    }
    if !refresh_warnings.is_empty() {
        for w in &refresh_warnings {
            crate::tee_eprintln!("warning: {w}");
        }
    }

    crate::format::render::render_doc(&doc, &fmt, &data_lines, args.format.select_spec())
}

struct ProbeResult {
    healthy: bool,
    detail: String,
    url: Option<String>,
}

// `aggregate_sources` lives in `verbs::cache` — shared helper.
