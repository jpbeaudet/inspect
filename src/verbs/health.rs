//! `inspect health <sel>` — per-service health probe.
//!
//! Strategy: if the cached profile has a `health` URL for the service, run
//! a remote `curl -fsS -m 3 <url>`. Otherwise report the cached
//! `health_status` if any. Host-level targets get a basic `uptime` probe.

use anyhow::Result;

use crate::cli::HealthArgs;
use crate::error::ExitKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::{Envelope, JsonOut, Renderer};
use crate::verbs::quote::shquote;

pub fn run(args: HealthArgs) -> Result<ExitKind> {
    let (runner, nses, targets) = plan(&args.selector)?;
    let mut renderer = Renderer::new();
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
                let cmd = format!("curl -fsS -m 3 -o /dev/null -w '%{{http_code}}' {} || true", shquote(u));
                let out = runner.run(&step.ns.namespace, &step.ns.target, &cmd, RunOpts::with_timeout(10))?;
                let code = out.stdout.trim().to_string();
                let healthy = code.starts_with('2') || code.starts_with('3');
                ProbeResult {
                    healthy,
                    detail: format!("HTTP {code}"),
                    url: Some(u.to_string()),
                }
            }
            None => {
                // No URL: fall back to cached marker.
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

        if args.json {
            JsonOut::write(
                &Envelope::new(&step.ns.namespace, "discovery", "discovery")
                    .with_service(&svc)
                    .put("healthy", result.healthy)
                    .put("detail", result.detail.clone())
                    .put("probe_url", result.url.clone()),
            );
        } else {
            let badge = if result.healthy { "OK " } else { "BAD" };
            renderer.data_line(format!(
                "[{badge}] {ns}/{svc:<20} {detail}",
                ns = step.ns.namespace,
                detail = result.detail
            ));
        }
    }

    if !args.json {
        renderer
            .summary(format!("{total} probe(s): {ok} ok, {bad} not-ok"))
            .next("inspect logs <sel>/<service> --since 5m");
        renderer.print();
    }

    Ok(ExitKind::Success)
}

struct ProbeResult {
    healthy: bool,
    detail: String,
    url: Option<String>,
}
