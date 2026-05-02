//! F6 (v0.1.3): `inspect compose up <ns>/<project>` — bring up a
//! compose project. Audited (`verb=compose.up`).
//!
//! Default invocation is `docker compose -p <p> up -d` (detached);
//! `--no-detach` switches to foreground (rare under inspect because
//! the audit-capture path would otherwise consume the long-lived
//! stdout). `--force-recreate` is a passthrough for the standard
//! compose flag.
//!
//! Revert: compose up has no clean inverse (the only honest answer
//! is `compose down`, but down has its own destructive side-effects
//! around volumes and networks). Recorded as `revert.kind =
//! unsupported` with a preview pointing the operator at
//! `inspect compose down` if they want to roll back.

use std::time::Instant;

use anyhow::Result;
use serde_json::json;

use crate::cli::ComposeUpArgs;
use crate::error::ExitKind;
use crate::safety::gate::ConfirmResult;
use crate::safety::{AuditEntry, AuditStore, Confirm, Revert, SafetyGate};
use crate::ssh::exec::RunOpts;
use crate::verbs::output::{NextStep, OutputDoc};
use crate::verbs::runtime::{current_runner, resolve_target};

use super::resolve::{project_in_profile, Parsed};
use super::write_common::{build_compose_cmd, compose_file_sha_short, project_tags};

pub fn run(args: ComposeUpArgs) -> Result<ExitKind> {
    let fmt = args.format.resolve()?;
    let parsed = match Parsed::parse(&args.selector) {
        Ok(p) => p,
        Err(e) => {
            crate::error::emit(format!("{e}"));
            return Ok(ExitKind::Error);
        }
    };
    let project_name = match parsed.project.as_deref() {
        Some(p) => p,
        None => {
            crate::error::emit(format!(
                "selector '{}' is missing the project portion — \
                 expected '<ns>/<project>'",
                args.selector
            ));
            return Ok(ExitKind::Error);
        }
    };
    let (_profile, project) = match project_in_profile(&parsed.namespace, project_name) {
        Ok(t) => t,
        Err(e) => {
            crate::error::emit(format!("{e}"));
            return Ok(ExitKind::NoMatches);
        }
    };

    // Build flag set: `-d` unless --no-detach; --force-recreate is
    // passthrough.
    let mut flags: Vec<&str> = Vec::new();
    if !args.no_detach {
        flags.push("-d");
    }
    if args.force_recreate {
        flags.push("--force-recreate");
    }
    let cmd = build_compose_cmd(&project, "up", &flags, None);

    let runner = current_runner();
    let (_resolved, target) = resolve_target(&parsed.namespace)?;

    let gate = SafetyGate::new(args.apply, args.yes, args.yes_all);
    if !gate.should_apply() {
        let doc = OutputDoc::new(
            format!(
                "DRY RUN. Would `compose up` on {ns}/{p}",
                ns = parsed.namespace,
                p = project.name
            ),
            json!({
                "namespace": parsed.namespace,
                "project": project.name,
                "rendered_cmd": cmd,
                "would_apply": false,
            }),
        )
        .with_meta("selector", args.selector.clone())
        .with_quiet(args.format.quiet);
        crate::format::render::render_doc(&doc, &fmt, &[cmd])?;
        eprintln!("Re-run with --apply to execute");
        return Ok(ExitKind::Success);
    }
    match gate.confirm(Confirm::Always, 1, "Continue?") {
        ConfirmResult::Aborted(why) => {
            eprintln!("aborted: {why}");
            return Ok(ExitKind::Error);
        }
        ConfirmResult::DryRun => unreachable!("apply branch already taken"),
        ConfirmResult::Apply => {}
    }

    let compose_hash =
        compose_file_sha_short(runner.as_ref(), &parsed.namespace, &target, &project);

    // up is typically fast (`-d` returns once orchestration is
    // initiated, not once every healthcheck passes). 5-minute
    // timeout covers slow image pulls when no separate `pull` was
    // run; pair with `compose pull --apply` first for big images.
    let started = Instant::now();
    let out = runner.run(&parsed.namespace, &target, &cmd, RunOpts::with_timeout(300))?;
    let dur = started.elapsed().as_millis() as u64;

    let mut entry = AuditEntry::new(
        "compose.up",
        &format!("{}/{}", parsed.namespace, project.name),
    );
    entry.exit = out.exit_code;
    entry.duration_ms = dur;
    entry.reason = crate::safety::validate_reason(args.reason.as_deref())?;
    entry.applied = Some(out.ok());
    entry.rendered_cmd = Some(cmd.clone());
    let mut extras: Vec<&str> = Vec::new();
    if args.no_detach {
        extras.push("[no_detach=true]");
    }
    if args.force_recreate {
        extras.push("[force_recreate=true]");
    }
    entry.args = project_tags(&project.name, None, &compose_hash, &extras);
    entry.revert = Some(Revert::unsupported(format!(
        "compose up has no clean inverse; \
         run `inspect compose down {ns}/{p} --apply` to roll back",
        ns = parsed.namespace,
        p = project.name
    )));
    let store = AuditStore::open()?;
    store.append(&entry)?;

    crate::verbs::cache::invalidate(&parsed.namespace);

    let mut data_lines: Vec<String> = Vec::new();
    if !out.stdout.trim().is_empty() {
        for line in out.stdout.lines() {
            data_lines.push(line.to_string());
        }
    }
    if !out.stderr.trim().is_empty() {
        for line in out.stderr.lines() {
            data_lines.push(line.to_string());
        }
    }

    let summary = if out.ok() {
        format!(
            "compose up {ns}/{p} ok (audit_id={id}, {dur}ms)",
            ns = parsed.namespace,
            p = project.name,
            id = entry.id,
            dur = dur
        )
    } else {
        format!(
            "compose up {ns}/{p} FAILED (exit {code}, audit_id={id}): {err}",
            ns = parsed.namespace,
            p = project.name,
            code = out.exit_code,
            id = entry.id,
            err = out.stderr.trim()
        )
    };

    let mut doc = OutputDoc::new(
        summary,
        json!({
            "namespace": parsed.namespace,
            "project": project.name,
            "audit_id": entry.id,
            "exit": out.exit_code,
            "duration_ms": dur,
            "compose_file_hash": compose_hash,
        }),
    )
    .with_meta("selector", args.selector.clone())
    .with_quiet(args.format.quiet);
    if out.ok() {
        doc.push_next(NextStep::new(
            format!(
                "inspect compose ps {ns}/{p}",
                ns = parsed.namespace,
                p = project.name
            ),
            "verify the per-service state",
        ));
    } else {
        doc.push_next(NextStep::new(
            format!(
                "inspect compose logs {ns}/{p} --tail 200",
                ns = parsed.namespace,
                p = project.name
            ),
            "look at recent logs to triage the failure",
        ));
    }
    crate::format::render::render_doc(&doc, &fmt, &data_lines)?;

    Ok(if out.ok() {
        ExitKind::Success
    } else {
        ExitKind::Error
    })
}
