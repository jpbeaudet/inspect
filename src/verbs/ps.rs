//! `inspect ps <sel>` — list running containers.

use anyhow::Result;

use crate::cli::PsArgs;
use crate::error::ExitKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::plan;
use crate::verbs::output::{Envelope, Renderer};

pub fn run(args: PsArgs) -> Result<ExitKind> {
    let (runner, nses, _targets) = plan(&args.selector)?;
    let fmt = args.format.resolve()?;
    let mut count = 0usize;
    let mut human = Renderer::new();
    let flag = if args.all { " -a" } else { "" };

    for ns in &nses {
        let cmd = format!("docker ps{flag} --format '{{{{json .}}}}'");
        let out = runner.run(&ns.namespace, &ns.target, &cmd, RunOpts::with_timeout(20))?;
        if !out.ok() {
            if fmt.shows_envelope() {
                crate::tee_eprintln!(
                    "{}: docker ps failed (exit {}): {}",
                    ns.namespace,
                    out.exit_code,
                    out.stderr.trim()
                );
            }
            continue;
        }
        for line in out.stdout.lines() {
            count += 1;
            let value: serde_json::Value = serde_json::from_str(line)
                .unwrap_or_else(|_| serde_json::Value::String(line.to_string()));
            let name = value
                .get("Names")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let image = value
                .get("Image")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let status = value
                .get("Status")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            human.data_line(format!(
                "{ns} | {name:<20} {image:<32} {status}",
                ns = ns.namespace
            ));
            human.push_row(
                &Envelope::new(&ns.namespace, "state", "state")
                    .with_service(&name)
                    .put("image", image)
                    .put("status", status)
                    .put("raw", value),
            );
        }
    }
    human.summary(format!("{count} container(s) running"));
    human.next("inspect status <sel> for health rollup");
    let select = args.format.select_filter()?;
    human.dispatch(&fmt, select)
}
