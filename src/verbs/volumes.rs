//! `inspect volumes <sel>` — list docker volumes.

use anyhow::Result;

use crate::cli::SimpleSelectorArgs;
use crate::error::ExitKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::plan;
use crate::verbs::output::{Envelope, Renderer};

pub fn run(args: SimpleSelectorArgs) -> Result<ExitKind> {
    let (runner, nses, _) = plan(&args.selector)?;
    let mut renderer = Renderer::new();
    let mut count = 0usize;
    for ns in &nses {
        let out = runner.run(
            &ns.namespace,
            &ns.target,
            "docker volume ls --format '{{json .}}'",
            RunOpts::with_timeout(20),
        )?;
        if !out.ok() {
            if !args.format.is_json() {
                eprintln!(
                    "{}: docker volume ls failed (exit {}): {}",
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
            renderer.data_line(format!(
                    "{ns} | {name:<32} {driver}",
                    ns = ns.namespace
                ));
            renderer.push_row(&Envelope::new(&ns.namespace, "volume", format!("volume:{name}"))
                        .put("name", name)
                        .put("driver", driver)
                        .put("raw", v));
        }
    }
            renderer.summary(format!("{count} volume(s)"));
    let __fmt = args.format.resolve()?;
    renderer.dispatch(&__fmt)?;
    Ok(ExitKind::Success)
}
