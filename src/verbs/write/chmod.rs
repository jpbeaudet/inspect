//! `inspect chmod <sel>:<path> <mode>` (bible §8).
//!
//! Always prompts on apply (irreversible perms change) unless `--yes`.

use std::time::Instant;

use anyhow::Result;

use crate::cli::ChmodArgs;
use crate::error::ExitKind;
use crate::safety::gate::ConfirmResult;
use crate::safety::{AuditEntry, AuditStore, Confirm, SafetyGate};
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::Renderer;
use crate::verbs::quote::shquote;

pub fn run(args: ChmodArgs) -> Result<ExitKind> {
    if !is_safe_mode(&args.mode) {
        eprintln!(
            "error: invalid mode '{}': expected octal like 0644 or symbolic (u+x, g-w)",
            args.mode
        );
        return Ok(ExitKind::Error);
    }

    let (runner, nses, targets) = plan(&args.target)?;
    let mut planned = Vec::new();
    for s in iter_steps(&nses, &targets) {
        let Some(p) = s.path.clone() else {
            crate::error::emit("chmod requires a :path on selector");
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
            "DRY RUN. Would chmod {} -> {}",
            planned.len(),
            args.mode
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
        &format!("chmod {} on {} target(s)?", args.mode, planned.len()),
    ) {
        eprintln!("aborted: {why}");
        return Ok(ExitKind::Error);
    }

    let store = AuditStore::open()?;
    let mut ok = 0usize;
    let mut bad = 0usize;
    let mut renderer = Renderer::new();

    for (s, path) in &planned {
        let inner = format!("chmod {} -- {}", shquote(&args.mode), shquote(path));
        let cmd = match s.service() {
            Some(svc) => format!("docker exec {} sh -c {}", shquote(svc), shquote(&inner)),
            None => inner,
        };
        let started = Instant::now();
        let out = runner.run(
            &s.ns.namespace,
            &s.ns.target,
            &cmd,
            RunOpts::with_timeout(30),
        )?;
        let label = format!(
            "{}{}:{path}",
            s.ns.namespace,
            s.service().map(|x| format!("/{x}")).unwrap_or_default()
        );
        let mut e = AuditEntry::new("chmod", &label);
        e.args = args.mode.clone();
        e.exit = out.exit_code;
        e.duration_ms = started.elapsed().as_millis() as u64;
        store.append(&e)?;
        if out.ok() {
            ok += 1;
            renderer.data_line(format!("{label}: chmod {}", args.mode));
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
        .summary(format!("chmod: {ok} ok, {bad} failed"))
        .next("inspect audit ls");
    renderer.print();
    Ok(if bad == 0 {
        ExitKind::Success
    } else {
        ExitKind::Error
    })
}

fn is_safe_mode(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // Octal: only digits 0-7, length 3 or 4.
    if s.chars().all(|c| ('0'..='7').contains(&c)) && (s.len() == 3 || s.len() == 4) {
        return true;
    }
    // Symbolic: only [ugoa+-=rwxXst,], no shell metas.
    s.chars().all(|c| {
        matches!(
            c,
            'u' | 'g' | 'o' | 'a' | '+' | '-' | '=' | 'r' | 'w' | 'x' | 'X' | 's' | 't' | ','
        )
    })
}
