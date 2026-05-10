//! `inspect mkdir <sel>:<path>` (bible §8).

use std::time::Instant;

use anyhow::Result;

use crate::cli::PathArgArgs;
use crate::error::ExitKind;
use crate::safety::gate::ConfirmResult;
use crate::safety::{AuditEntry, AuditStore, Confirm, Revert, SafetyGate};
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::Renderer;
use crate::verbs::quote::shquote;

pub fn run(args: PathArgArgs) -> Result<ExitKind> {
    let (runner, nses, targets) = plan(&args.target)?;
    let mut planned = Vec::new();
    for s in iter_steps(&nses, &targets) {
        let Some(p) = s.path.clone() else {
            crate::error::emit("mkdir requires a :path on selector");
            return Ok(ExitKind::Error);
        };
        planned.push((s, p));
    }
    if planned.is_empty() {
        crate::error::emit(format!("'{}' matched no targets", args.target));
        return Ok(ExitKind::Error);
    }

    let gate = SafetyGate::new(args.apply, args.yes, args.yes_all);
    if !gate.should_apply() {
        let mut r = Renderer::new();
        r.summary(format!("DRY RUN. Would mkdir {} dir(s):", planned.len()));
        for (s, p) in &planned {
            r.data_line(format!(
                "{}{}:{p}",
                s.ns.namespace,
                s.service().map(|x| format!("/{x}")).unwrap_or_default()
            ));
        }
        r.next("Re-run with --apply to execute");
        r.print();
        return Ok(ExitKind::Success);
    }
    if let ConfirmResult::Aborted(why) =
        gate.confirm(Confirm::LargeFanout, planned.len(), "Continue?")
    {
        eprintln!("aborted: {why}");
        return Ok(ExitKind::Error);
    }

    let store = AuditStore::open()?;
    let mut ok = 0usize;
    let mut bad = 0usize;
    let mut renderer = Renderer::new();

    for (s, path) in &planned {
        // Probe whether the directory already exists.
        // If it did not, the inverse is `rmdir <path>` (only if the
        // operator's mkdir actually created it). If it did, mkdir is
        // a no-op and revert is unsupported.
        let probe_inner = format!("test -d {} && echo y || echo n", shquote(path));
        let probe_cmd = match s.container() {
            Some(container) => format!(
                "docker exec {} sh -c {}",
                shquote(container),
                shquote(&probe_inner)
            ),
            None => probe_inner,
        };
        let pre_existed = runner
            .run(
                &s.ns.namespace,
                &s.ns.target,
                &probe_cmd,
                RunOpts::with_timeout(15),
            )
            .ok()
            .map(|o| o.stdout.trim() == "y")
            .unwrap_or(true);
        let label = format!(
            "{}{}:{path}",
            s.ns.namespace,
            s.service().map(|x| format!("/{x}")).unwrap_or_default()
        );
        let revert = if pre_existed {
            Revert::unsupported(format!(
                "{path} already existed before mkdir; revert is a no-op"
            ))
        } else {
            // Payload is the literal
            // command for the runner, wrapped in `docker exec` when
            // the original verb dispatched through a container.
            let inner_revert = format!("rmdir -- {}", shquote(path));
            let payload = match s.container() {
                Some(container) => format!(
                    "docker exec {} sh -c {}",
                    shquote(container),
                    shquote(&inner_revert)
                ),
                None => inner_revert,
            };
            Revert::command_pair(payload, format!("rmdir {path}"))
        };
        if args.revert_preview {
            eprintln!(
                "[inspect] revert preview {label}: {kind} -- {preview}",
                kind = revert.kind.as_str(),
                preview = revert.preview,
            );
        }
        let inner = format!("mkdir -p -- {}", shquote(path));
        let cmd = match s.container() {
            Some(container) => format!(
                "docker exec {} sh -c {}",
                shquote(container),
                shquote(&inner)
            ),
            None => inner,
        };
        let started = Instant::now();
        let out = runner.run(
            &s.ns.namespace,
            &s.ns.target,
            &cmd,
            RunOpts::with_timeout(30),
        )?;
        let mut e = AuditEntry::new("mkdir", &label);
        e.exit = out.exit_code;
        e.duration_ms = started.elapsed().as_millis() as u64;
        e.reason = crate::safety::validate_reason(args.reason.as_deref())?;
        e.revert = Some(revert);
        e.applied = Some(out.ok());
        store.append(&e)?;

        if out.ok() {
            ok += 1;
            renderer.data_line(format!("{label}: created"));
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
        .summary(format!("mkdir: {ok} ok, {bad} failed"))
        .next("inspect audit ls");
    renderer.print();
    Ok(if bad == 0 {
        ExitKind::Success
    } else {
        ExitKind::Error
    })
}
