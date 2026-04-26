//! `inspect rm <sel>:<path>` (bible §8). Always prompts on apply unless `-y`.

use std::time::Instant;

use anyhow::Result;

use crate::cli::PathArgArgs;
use crate::error::ExitKind;
use crate::safety::{AuditEntry, AuditStore, Confirm, SafetyGate};
use crate::safety::gate::ConfirmResult;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::Renderer;
use crate::verbs::quote::shquote;

pub fn run(args: PathArgArgs) -> Result<ExitKind> {
    let (runner, nses, targets) = plan(&args.target)?;
    let mut steps_with_path = Vec::new();
    for s in iter_steps(&nses, &targets) {
        let Some(p) = s.path.clone() else {
            eprintln!("error: rm requires a :path on selector");
            return Ok(ExitKind::Error);
        };
        steps_with_path.push((s, p));
    }
    if steps_with_path.is_empty() {
        eprintln!("error: '{}' matched no targets", args.target);
        return Ok(ExitKind::Error);
    }

    let gate = SafetyGate::new(args.apply, args.yes, args.yes_all);
    if !gate.should_apply() {
        let mut r = Renderer::new();
        r.summary(format!(
            "DRY RUN. Would rm {} path(s):",
            steps_with_path.len()
        ));
        for (s, p) in &steps_with_path {
            let svc = s.service().map(|x| format!("/{x}")).unwrap_or_default();
            r.data_line(format!("{}{svc}:{p}", s.ns.namespace));
        }
        r.next("Re-run with --apply to execute");
        r.print();
        return Ok(ExitKind::Success);
    }
    if let ConfirmResult::Aborted(why) = gate.confirm(
        Confirm::Always,
        steps_with_path.len(),
        &format!("Delete {} file(s)?", steps_with_path.len()),
    ) {
        eprintln!("aborted: {why}");
        return Ok(ExitKind::Error);
    }

    let store = AuditStore::open()?;
    let mut ok = 0usize;
    let mut bad = 0usize;
    let mut renderer = Renderer::new();

    for (s, path) in &steps_with_path {
        let inner = format!("rm -f -- {}", shquote(path));
        let cmd = match s.service() {
            Some(svc) => format!("docker exec {} sh -c {}", shquote(svc), shquote(&inner)),
            None => inner.clone(),
        };
        let started = Instant::now();
        let out = runner.run(&s.ns.namespace, &s.ns.target, &cmd, RunOpts::with_timeout(30))?;
        let dur = started.elapsed().as_millis() as u64;

        let label = format!(
            "{}{}:{path}",
            s.ns.namespace,
            s.service().map(|x| format!("/{x}")).unwrap_or_default()
        );
        let mut e = AuditEntry::new("rm", &label);
        e.exit = out.exit_code;
        e.duration_ms = dur;
        store.append(&e)?;

        if out.ok() {
            ok += 1;
            renderer.data_line(format!("{label}: removed"));
        } else {
            bad += 1;
            renderer.data_line(format!(
                "{label}: FAILED (exit {}): {}",
                out.exit_code,
                out.stderr.trim()
            ));
        }
    }
    renderer
        .summary(format!("rm: {ok} ok, {bad} failed"))
        .next("inspect audit ls");
    renderer.print();
    Ok(if bad == 0 { ExitKind::Success } else { ExitKind::Error })
}
