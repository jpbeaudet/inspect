//! `inspect network <sel>` — list docker networks.

use anyhow::Result;

use crate::cli::SimpleSelectorArgs;
use crate::error::ExitKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::plan;
use crate::verbs::output::{Envelope, JsonOut, Renderer};

pub fn run(args: SimpleSelectorArgs) -> Result<ExitKind> {
    let (runner, nses, _) = plan(&args.selector)?;
    let mut renderer = Renderer::new();
    let mut count = 0usize;
    for ns in &nses {
        let out = runner.run(
            &ns.namespace,
            &ns.target,
            "docker network ls --format '{{json .}}'",
            RunOpts::with_timeout(20),
        )?;
        if !out.ok() {
            if !args.json {
                eprintln!(
                    "{}: docker network ls failed (exit {}): {}",
                    ns.namespace,
                    out.exit_code,
                    out.stderr.trim()
                );
            }
            continue;
        }
        for line in out.stdout.lines() {
            count += 1;
            let v: serde_json::Value = serde_json::from_str(line).unwrap_or_default();
            let name = v.get("Name").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let driver = v.get("Driver").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let scope = v.get("Scope").and_then(|x| x.as_str()).unwrap_or("").to_string();
            if args.json {
                JsonOut::write(
                    &Envelope::new(&ns.namespace, "network", format!("network:{name}"))
                        .put("name", name)
                        .put("driver", driver)
                        .put("scope", scope)
                        .put("raw", v),
                );
            } else {
                renderer.data_line(format!(
                    "{ns} | {name:<24} {driver:<12} {scope}",
                    ns = ns.namespace
                ));
            }
        }
    }
    if !args.json {
        renderer.summary(format!("{count} network(s)"));
        renderer.print();
    }
    Ok(ExitKind::Success)
}
