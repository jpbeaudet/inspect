//! `Inspect compose down
//! <ns>/<project>[/<service>]` — tear down a compose project, or
//! stop + remove a single service. Audited (`verb=compose.down`).
//!
//! `--volumes` is destructive (matches docker's `--volumes` semantics
//! for named volumes); the audit args carry an explicit
//! `[volumes=true]` tag so post-mortem readers see at a glance that
//! data may have been wiped.
//!
//! When the selector includes a service portion, the
//! shape changes to a compound command:
//! `docker compose -p <p> stop <svc> && docker compose -p <p> rm -f
//! <svc>`. This stops and removes only that service's container,
//! preserving every other service in the project. Per-service
//! `--volumes` is rejected loudly — compose's named-volume removal
//! is project-scoped (volumes are referenced by *every* service
//! that mounts them), and silently honoring `--volumes` against a
//! single-service tear-down would either no-op (confusing) or wipe
//! data shared with other services (worse). Per-service `--rmi` is
//! also rejected for the same reason: `--rmi local` operates on
//! the project's image set as a whole.
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
use super::write_common::{
    build_compose_cmd, build_compose_per_service_down_cmd, compose_file_sha_short, project_tags,
};

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
                 expected '<ns>/<project>[/<service>]'",
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

    // Per-service narrowing has different semantics than
    // project-level down — `--volumes` and `--rmi` are project-scoped
    // operations and silently honoring them against one service would
    // either no-op or wipe data the operator did not intend to touch.
    if let Some(svc) = parsed.service.as_deref() {
        if args.volumes {
            crate::error::emit(format!(
                "compose down --volumes is not supported for per-service selectors \
                 ('{ns}/{p}/{svc}'). Named volumes are project-scoped, often shared \
                 across services; removing them while only one service is being \
                 torn down would silently affect other services. \
                 hint: tear the whole project down with `inspect compose down {ns}/{p} \
                 --volumes --apply`, or stop just this service with `inspect compose \
                 down {ns}/{p}/{svc} --apply` (no --volumes).",
                ns = parsed.namespace,
                p = project.name,
            ));
            return Ok(ExitKind::Error);
        }
        if args.rmi {
            crate::error::emit(format!(
                "compose down --rmi is not supported for per-service selectors \
                 ('{ns}/{p}/{svc}'). `--rmi local` operates on the project's image \
                 set as a whole. \
                 hint: drop --rmi to stop + remove just this service, or run \
                 `inspect compose down {ns}/{p} --rmi --apply` against the project.",
                ns = parsed.namespace,
                p = project.name,
            ));
            return Ok(ExitKind::Error);
        }
    }

    let mut flags: Vec<&str> = Vec::new();
    if args.volumes {
        flags.push("--volumes");
    }
    if args.rmi {
        flags.push("--rmi");
        flags.push("local");
    }
    // Per-service form is `stop <svc> && rm -f <svc>` rather than
    // `down <svc>` — `docker compose down <svc>` is not a documented
    // shape and behaves inconsistently across compose versions. The
    // explicit two-step is what every operator's runbook says.
    let cmd = match parsed.service.as_deref() {
        Some(svc) => build_compose_per_service_down_cmd(&project, svc),
        None => build_compose_cmd(&project, "down", &flags, None),
    };
    let scope = parsed
        .service
        .as_deref()
        .map(|s| format!("service {s}"))
        .unwrap_or_else(|| "project".to_string());

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
                "DRY RUN. Would `compose down` ({scope}) on {ns}/{p}{warning}",
                ns = parsed.namespace,
                p = project.name
            ),
            json!({
                "namespace": parsed.namespace,
                "project": project.name,
                "service": parsed.service,
                "rendered_cmd": cmd,
                "destructive": args.volumes,
                "would_apply": false,
            }),
        )
        .with_meta("selector", args.selector.clone())
        .with_quiet(args.format.quiet);
        let exit =
            crate::format::render::render_doc(&doc, &fmt, &[cmd], args.format.select_spec())?;
        eprintln!("Re-run with --apply to execute");
        return Ok(exit);
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

    let audit_selector = match parsed.service.as_deref() {
        Some(svc) => format!("{}/{}/{}", parsed.namespace, project.name, svc),
        None => format!("{}/{}", parsed.namespace, project.name),
    };
    let mut entry = AuditEntry::new("compose.down", &audit_selector);
    entry.exit = out.exit_code;
    entry.duration_ms = dur;
    entry.reason = crate::safety::validate_reason(args.reason.as_deref())?;
    entry.applied = Some(out.ok());
    entry.rendered_cmd = Some(crate::redact::redact_for_audit(&cmd).into_owned());
    let mut extras: Vec<&str> = Vec::new();
    if args.volumes {
        extras.push("[volumes=true]");
    }
    if args.rmi {
        extras.push("[rmi=local]");
    }
    entry.args = project_tags(
        &project.name,
        parsed.service.as_deref(),
        &compose_hash,
        &extras,
    );
    let revert_target = match parsed.service.as_deref() {
        Some(svc) => format!(
            "inspect compose up {ns}/{p}/{svc} --apply",
            ns = parsed.namespace,
            p = project.name,
        ),
        None => format!(
            "inspect compose up {ns}/{p} --apply",
            ns = parsed.namespace,
            p = project.name,
        ),
    };
    entry.revert = Some(Revert::unsupported(format!(
        "compose down has no clean inverse{volumes_note}; \
         run `{revert_target}` to bring services back, \
         but state in removed volumes is gone",
        volumes_note = if args.volumes {
            " (--volumes obliterates named volumes)"
        } else {
            ""
        }
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

    let label = audit_selector.as_str();
    let summary = if out.ok() {
        format!(
            "compose down {label} ok (audit_id={id}, {dur}ms)",
            id = entry.id,
            dur = dur
        )
    } else {
        format!(
            "compose down {label} FAILED (exit {code}, audit_id={id}): {err}",
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
            "service": parsed.service,
            "audit_id": entry.id,
            "exit": out.exit_code,
            "duration_ms": dur,
            "compose_file_hash": compose_hash,
            "destructive": args.volumes,
        }),
    )
    .with_meta("selector", args.selector.clone())
    .with_quiet(args.format.quiet);
    // Per-service teardown points the operator at `compose ps` so
    // they can confirm the rest of the project is still healthy.
    // Project-level teardown points at `compose ls` because the
    // project itself should now be gone.
    let next_target = match parsed.service.as_deref() {
        Some(_) => (
            format!(
                "inspect compose ps {ns}/{p}",
                ns = parsed.namespace,
                p = project.name,
            ),
            "confirm the rest of the project is still healthy",
        ),
        None => (
            format!("inspect compose ls {ns}", ns = parsed.namespace),
            "confirm the project no longer appears as running",
        ),
    };
    doc.push_next(NextStep::new(next_target.0, next_target.1));
    let exit =
        crate::format::render::render_doc(&doc, &fmt, &data_lines, args.format.select_spec())?;

    // Exec failure exit class takes precedence over filter-class.
    Ok(if out.ok() { exit } else { ExitKind::Error })
}
