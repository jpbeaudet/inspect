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
    // K7 (v0.1.4): a k8s namespace reports pod health from its discovered
    // profile (the docker plan()/get_runtime path is SSH-bound).
    if let Some(ns_name) = args.selector.split('/').next() {
        if let Ok(resolved) = crate::config::resolver::resolve(ns_name) {
            if resolved.config.runtime_kind() == crate::exec::runtime::RuntimeKind::K8s {
                return health_k8s(&args, ns_name);
            }
        }
    }

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

/// K7 (v0.1.4): `inspect health <k8s-ns>` — per-pod health from the discovered
/// profile. Each pod is a "probe": healthy iff its readiness-derived
/// `health_status` is Ok; a missing readiness / CrashLoop / Failed is not-ok.
/// Same envelope shape as docker health so an agent branches identically.
fn health_k8s(args: &HealthArgs, ns: &str) -> Result<ExitKind> {
    use crate::profile::schema::HealthStatus;
    let fmt = args.format.resolve()?;

    let profile = match crate::profile::cache::load_profile(ns)? {
        Some(p) => p,
        None => {
            let mut doc = OutputDoc::new(
                format!("no cached profile for '{ns}' — run `inspect setup {ns}` first"),
                json!({ "probes": [], "totals": { "total": 0, "ok": 0, "bad": 0 } }),
            )
            .with_meta("selector", args.selector.clone())
            .with_meta("runtime", "k8s".to_string());
            doc.push_next(crate::verbs::output::NextStep::new(
                format!("inspect setup {ns}"),
                "discover the cluster's pods first",
            ));
            return crate::format::render::render_doc(&doc, &fmt, &[], args.format.select_spec());
        }
    };

    let (mut total, mut ok, mut bad) = (0usize, 0, 0);
    let mut probes_json: Vec<Value> = Vec::new();
    let mut data_lines: Vec<String> = Vec::new();
    for s in &profile.services {
        total += 1;
        let healthy = matches!(s.health_status, Some(HealthStatus::Ok));
        if healthy {
            ok += 1;
        } else {
            bad += 1;
        }
        let hs = match s.health_status {
            Some(HealthStatus::Ok) => "ok",
            Some(HealthStatus::Unhealthy) => "unhealthy",
            Some(HealthStatus::Starting) => "starting",
            _ => "unknown",
        };
        let phase = s.health.clone().unwrap_or_default();
        let detail = format!("{phase} ({hs})");
        probes_json.push(json!({
            "server": ns,
            "service": s.name,
            "healthy": healthy,
            "detail": detail,
            "probe_url": Value::Null,
        }));
        let badge = if healthy { "OK " } else { "BAD" };
        data_lines.push(format!("[{badge}] {ns}/{name:<28} {detail}", name = s.name));
    }

    let summary = format!("{total} probe(s): {ok} ok, {bad} not-ok");
    let doc = OutputDoc::new(
        summary,
        json!({
            "probes": probes_json,
            "totals": { "total": total, "ok": ok, "bad": bad },
        }),
    )
    .with_meta("selector", args.selector.clone())
    .with_meta("runtime", "k8s".to_string())
    .with_meta("context", profile.host.clone())
    .with_meta("source", "cached (profile) — refresh with `inspect setup --force`".to_string())
    .with_quiet(args.format.quiet);
    crate::format::render::render_doc(&doc, &fmt, &data_lines, args.format.select_spec())
}

struct ProbeResult {
    healthy: bool,
    detail: String,
    url: Option<String>,
}

// `aggregate_sources` lives in `verbs::cache` — shared helper.
