//! `inspect exec <sel> -- <cmd>` (bible §8).
//!
//! Runs a free-form command on the target. `--apply` required (no
//! preview semantics — the command is itself the action).

use std::time::Instant;

use anyhow::Result;

use crate::cli::ExecArgs;
use crate::error::ExitKind;
use crate::safety::{AuditEntry, AuditStore, Confirm, SafetyGate};
use crate::safety::gate::ConfirmResult;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::Renderer;
use crate::verbs::quote::shquote;

pub fn run(args: ExecArgs) -> Result<ExitKind> {
    if args.cmd.is_empty() {
        eprintln!("error: exec requires a command after `--`");
        return Ok(ExitKind::Error);
    }
    let user_cmd = args.cmd.join(" ");

    let (runner, nses, targets) = plan(&args.selector)?;
    let steps: Vec<_> = iter_steps(&nses, &targets).collect();
    if steps.is_empty() {
        eprintln!("error: '{}' matched no targets", args.selector);
        return Ok(ExitKind::Error);
    }

    let gate = SafetyGate::new(args.apply, args.yes, args.yes_all);
    if !gate.should_apply() {
        let mut r = Renderer::new();
        r.summary(format!(
            "DRY RUN. Would exec on {} target(s):",
            steps.len()
        ));
        for s in &steps {
            let svc = s.service().map(|x| format!("/{x}")).unwrap_or_default();
            r.data_line(format!("{}{svc}: {user_cmd}", s.ns.namespace));
        }
        r.next("Re-run with --apply to execute");
        r.print();
        return Ok(ExitKind::Success);
    }
    if let ConfirmResult::Aborted(why) =
        gate.confirm(Confirm::LargeFanout, steps.len(), "Continue?")
    {
        eprintln!("aborted: {why}");
        return Ok(ExitKind::Error);
    }

    let store = AuditStore::open()?;
    let mut ok = 0usize;
    let mut bad = 0usize;
    let mut renderer = Renderer::new();

    for s in &steps {
        let cmd = match s.service() {
            Some(svc) => format!("docker exec {} sh -c {}", shquote(svc), shquote(&user_cmd)),
            None => user_cmd.clone(),
        };
        let started = Instant::now();
        let out = runner.run(
            &s.ns.namespace,
            &s.ns.target,
            &cmd,
            RunOpts::with_timeout(args.timeout_secs.unwrap_or(120)),
        )?;
        let dur = started.elapsed().as_millis() as u64;

        let mut e = AuditEntry::new(
            "exec",
            &format!(
                "{}{}",
                s.ns.namespace,
                s.service().map(|x| format!("/{x}")).unwrap_or_default()
            ),
        );
        e.args = user_cmd.clone();
        e.exit = out.exit_code;
        e.duration_ms = dur;
        store.append(&e)?;

        let label = format!(
            "{}{}",
            s.ns.namespace,
            s.service().map(|x| format!("/{x}")).unwrap_or_default()
        );
        if out.ok() {
            ok += 1;
            renderer.data_line(format!("{label}: ok ({}ms)", dur));
            if !out.stdout.trim().is_empty() {
                for line in out.stdout.lines() {
                    renderer.data_line(format!("  {line}"));
                }
            }
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
        .summary(format!("exec: {ok} ok, {bad} failed"))
        .next("inspect audit ls");
    renderer.print();
    Ok(if bad == 0 { ExitKind::Success } else { ExitKind::Error })
}
