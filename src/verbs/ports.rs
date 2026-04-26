//! `inspect ports <sel>` — list listening ports.
//!
//! Strategy: prefer `ss -tlnp`, fall back to `netstat -tlnp` if the host
//! profile says `ss` isn't available. For container selectors, `docker
//! port <name>` is more honest (returns the published mapping).

use anyhow::Result;

use crate::cli::SimpleSelectorArgs;
use crate::error::ExitKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::{Envelope, JsonOut, Renderer};
use crate::verbs::quote::shquote;

pub fn run(args: SimpleSelectorArgs) -> Result<ExitKind> {
    let (runner, nses, targets) = plan(&args.selector)?;
    let mut renderer = Renderer::new();
    let mut count = 0usize;

    for step in iter_steps(&nses, &targets) {
        let cmd = match step.service() {
            Some(svc) => format!("docker port {} 2>/dev/null || true", shquote(svc)),
            None => {
                let prefer_ss = step
                    .ns
                    .profile
                    .as_ref()
                    .map(|p| p.remote_tooling.ss)
                    .unwrap_or(true);
                if prefer_ss {
                    "ss -tlnp 2>/dev/null || netstat -tlnp 2>/dev/null".to_string()
                } else {
                    "netstat -tlnp 2>/dev/null".to_string()
                }
            }
        };
        let out = runner.run(&step.ns.namespace, &step.ns.target, &cmd, RunOpts::with_timeout(15))?;
        if !out.ok() {
            if !args.format.is_json() {
                eprintln!(
                    "{}: ports failed (exit {}): {}",
                    step.ns.namespace,
                    out.exit_code,
                    out.stderr.trim()
                );
            }
            continue;
        }
        for line in out.stdout.lines() {
            count += 1;
            renderer.data_line(format!(
                    "{ns}{svc} | {line}",
                    ns = step.ns.namespace,
                    svc = step.service().map(|s| format!("/{s}")).unwrap_or_default()
                ));
            renderer.push_row(&Envelope::new(&step.ns.namespace, "network", "ports")
                        .with_service(step.service().unwrap_or("_"))
                        .put("line", line));
        }
    }
            renderer.summary(format!("{count} port-line(s)"));
    let __fmt = args.format.resolve()?;
    renderer.dispatch(&__fmt)?;
    Ok(ExitKind::Success)
}
