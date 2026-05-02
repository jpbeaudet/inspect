//! F6 (v0.1.3): `inspect compose down <ns>/<project>` — tear down a
//! compose project. Audited (`verb=compose.down`).
//!
//! `--volumes` is destructive (matches docker's `--volumes` semantics
//! for named volumes); the audit args carry an explicit
//! `[volumes=true]` tag so post-mortem readers see at a glance that
//! data may have been wiped.
//!
//! Revert: `revert.kind = unsupported`. The honest counterpart is
//! `inspect compose up`, but a `--volumes` down obliterates state
//! that no `up` can restore. Recorded as unsupported so
//! `inspect revert <id>` surfaces the right error rather than
//! silently no-opping.

use std::time::Instant;

use anyhow::Result;
use serde_json::json;

use crate::cli::ComposeDownArgs;
use crate::error::ExitKind;
use crate::safety::gate::ConfirmResult;
use crate::safety::{AuditEntry, AuditStore, Confirm, Revert, SafetyGate};
use crate::ssh::exec::RunOpts;
use crate::verbs::output::{NextStep, OutputDoc};
use crate::verbs::runtime::{current_runner, resolve_target};

use super::resolve::{project_in_profile, Parsed};
use super::write_common::{build_compose_cmd, compose_file_sha_short, project_tags};

pub fn run(args: ComposeDownArgs) -> Result<ExitKind> {
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

    let mut flags: Vec<&str> = Vec::new();
    if args.volumes {
        flags.push("--volumes");
    }
    if args.rmi {
        flags.push("--rmi");
        flags.push("local");
    }
    let cmd = build_compose_cmd(&project, "down", &flags, None);

    let runner = current_runner();
    let (_resolved, target) = resolve_target(&parsed.namespace)?;

    let gate = SafetyGate::new(args.apply, args.yes, args.yes_all);
    if !gate.should_apply() {
        let warning = if args.volumes {
            " (DESTRUCTIVE: --volumes would remove named volumes)"
        } else {
            ""
        };
        let doc = OutputDoc::new(
            format!(
                "DRY RUN. Would `compose down` on {ns}/{p}{warning}",
                ns = parsed.namespace,
                p = project.name
            ),
            json!({
                "namespace": parsed.namespace,
                "project": project.name,
                "rendered_cmd": cmd,
                "destructive": args.volumes,
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

    // 5-minute timeout — `down` waits for graceful shutdown of every
    // container; well-behaved apps may take a minute or two.
    let started = Instant::now();
    let out = runner.run(&parsed.namespace, &target, &cmd, RunOpts::with_timeout(300))?;
    let dur = started.elapsed().as_millis() as u64;

    let mut entry = AuditEntry::new(
        "compose.down",
        &format!("{}/{}", parsed.namespace, project.name),
    );
    entry.exit = out.exit_code;
    entry.duration_ms = dur;
    entry.reason = crate::safety::validate_reason(args.reason.as_deref())?;
    entry.applied = Some(out.ok());
    entry.rendered_cmd = Some(cmd.clone());
    let mut extras: Vec<&str> = Vec::new();
    if args.volumes {
        extras.push("[volumes=true]");
    }
    if args.rmi {
        extras.push("[rmi=local]");
    }
    entry.args = project_tags(&project.name, None, &compose_hash, &extras);
    entry.revert = Some(Revert::unsupported(format!(
        "compose down has no clean inverse{volumes_note}; \
         run `inspect compose up {ns}/{p} --apply` to bring services back, \
         but state in removed volumes is gone",
        volumes_note = if args.volumes {
            " (--volumes obliterates named volumes)"
        } else {
            ""
        },
        ns = parsed.namespace,
        p = project.name
    )));
    let store = AuditStore::open()?;
    store.append(&entry)?;

    crate::verbs::cache::invalidate(&parsed.namespace);

    let mut data_lines: Vec<String> = Vec::new();
    for line in out.stdout.lines() {
        data_lines.push(line.to_string());
    }
    for line in out.stderr.lines() {
        data_lines.push(line.to_string());
    }

    let summary = if out.ok() {
        format!(
            "compose down {ns}/{p} ok (audit_id={id}, {dur}ms)",
            ns = parsed.namespace,
            p = project.name,
            id = entry.id,
            dur = dur
        )
    } else {
        format!(
            "compose down {ns}/{p} FAILED (exit {code}, audit_id={id}): {err}",
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
            "destructive": args.volumes,
        }),
    )
    .with_meta("selector", args.selector.clone())
    .with_quiet(args.format.quiet);
    doc.push_next(NextStep::new(
        format!("inspect compose ls {ns}", ns = parsed.namespace),
        "confirm the project no longer appears as running",
    ));
    crate::format::render::render_doc(&doc, &fmt, &data_lines)?;

    Ok(if out.ok() {
        ExitKind::Success
    } else {
        ExitKind::Error
    })
}
