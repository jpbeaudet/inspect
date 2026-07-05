//! `inspect ps <sel>` — list running containers.

use anyhow::Result;

use crate::cli::PsArgs;
use crate::error::ExitKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::plan;
use crate::verbs::output::{Envelope, Renderer};

pub fn run(args: PsArgs) -> Result<ExitKind> {
    // K7 (v0.1.4): a k8s namespace lists pods from its discovered profile —
    // the docker `plan()` + `docker ps` path is SSH-bound.
    if let Some(ns_name) = args.selector.split('/').next() {
        if let Ok(resolved) = crate::config::resolver::resolve(ns_name) {
            if resolved.config.runtime_kind() == crate::exec::runtime::RuntimeKind::K8s {
                return ps_k8s(&args, ns_name);
            }
        }
    }

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

/// K7 (v0.1.4): `inspect ps <k8s-ns>` — list pods from the discovered profile
/// (the pod is the container-equivalent; its name is the kubectl address).
fn ps_k8s(args: &PsArgs, ns: &str) -> Result<ExitKind> {
    let fmt = args.format.resolve()?;
    let mut human = Renderer::new();
    let select = args.format.select_filter()?;

    let profile = match crate::profile::cache::load_profile(ns)? {
        Some(p) => p,
        None => {
            human
                .summary(format!(
                    "no cached profile for '{ns}' — run `inspect setup {ns}` first"
                ))
                .next(format!("inspect setup {ns}"));
            return human.dispatch(&fmt, select);
        }
    };

    let mut count = 0usize;
    for s in &profile.services {
        count += 1;
        let image = s.image.clone().unwrap_or_default();
        let phase = s.health.clone().unwrap_or_default(); // pod phase, e.g. Running
        human.data_line(format!("{ns} | {name:<28} {image:<40} {phase}", name = s.name));
        human.push_row(
            &Envelope::new(ns, "state", "state")
                .with_service(&s.name)
                .put("image", image)
                .put("status", phase),
        );
    }
    human
        .summary(format!("{count} pod(s)"))
        .next("inspect status <sel> for health rollup");
    human.dispatch(&fmt, select)
}
