//! `inspect chown <sel>:<path> <owner>[:<group>]` (bible §8).

use std::time::Instant;

use anyhow::Result;

use crate::cli::ChownArgs;
use crate::error::ExitKind;
use crate::safety::gate::ConfirmResult;
use crate::safety::{AuditEntry, AuditStore, Confirm, Revert, SafetyGate};
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::Renderer;
use crate::verbs::quote::shquote;

pub fn run(args: ChownArgs) -> Result<ExitKind> {
    if !is_safe_owner(&args.owner) {
        eprintln!(
            "error: invalid owner spec '{}': expected user[:group] with [a-zA-Z0-9_.-]",
            args.owner
        );
        return Ok(ExitKind::Error);
    }
    let (runner, nses, targets) = plan(&args.target)?;
    let mut planned = Vec::new();
    for s in iter_steps(&nses, &targets) {
        let Some(p) = s.path.clone() else {
            crate::error::emit("chown requires a :path on selector");
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
        r.summary(format!(
            "DRY RUN. Would chown {} -> {}",
            planned.len(),
            args.owner
        ));
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
    if let ConfirmResult::Aborted(why) = gate.confirm(
        Confirm::Always,
        planned.len(),
        &format!("chown {} on {} target(s)?", args.owner, planned.len()),
    ) {
        eprintln!("aborted: {why}");
        return Ok(ExitKind::Error);
    }
    let store = AuditStore::open()?;
    let mut ok = 0usize;
    let mut bad = 0usize;
    let mut renderer = Renderer::new();
    for (s, path) in &planned {
        // F11 (v0.1.3): capture prior owner for the inverse.
        let stat_inner = format!("stat -c %u:%g -- {}", shquote(path));
        let stat_cmd = match s.container() {
            Some(container) => format!(
                "docker exec {} sh -c {}",
                shquote(container),
                shquote(&stat_inner)
            ),
            None => stat_inner,
        };
        let prev_owner = runner
            .run(&s.ns.namespace, &s.ns.target, &stat_cmd, RunOpts::with_timeout(15))
            .ok()
            .and_then(|o| if o.ok() { Some(o.stdout.trim().to_string()) } else { None })
            .filter(|s| !s.is_empty() && is_safe_owner(s));
        let label = format!(
            "{}{}:{path}",
            s.ns.namespace,
            s.service().map(|x| format!("/{x}")).unwrap_or_default()
        );
        let revert = match prev_owner.as_deref() {
            Some(o) => Revert::command_pair(
                format!("chown {} -- {}", shquote(o), shquote(path)),
                format!("chown {o} {path}"),
            ),
            None => Revert::unsupported(format!(
                "could not capture prior owner of {path}; revert unavailable"
            )),
        };
        if args.revert_preview {
            eprintln!(
                "[inspect] revert preview {label}: {kind} -- {preview}",
                kind = revert.kind.as_str(),
                preview = revert.preview,
            );
        }
        let inner = format!("chown {} -- {}", shquote(&args.owner), shquote(path));
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
        let mut e = AuditEntry::new("chown", &label);
        e.args = args.owner.clone();
        e.exit = out.exit_code;
        e.duration_ms = started.elapsed().as_millis() as u64;
        e.reason = crate::safety::validate_reason(args.reason.as_deref())?;
        e.revert = Some(revert);
        e.applied = Some(out.ok());
        store.append(&e)?;
        if out.ok() {
            ok += 1;
            renderer.data_line(format!("{label}: chown {}", args.owner));
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
        .summary(format!("chown: {ok} ok, {bad} failed"))
        .next("inspect audit ls");
    renderer.print();
    Ok(if bad == 0 {
        ExitKind::Success
    } else {
        ExitKind::Error
    })
}

fn is_safe_owner(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | ':'))
}
