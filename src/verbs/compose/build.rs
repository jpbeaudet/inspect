//! F6 (v0.1.3): `inspect compose build <ns>/<project>[/<service>]`
//! — build images for a compose project. Audited (`verb=compose.build`).
//!
//! Streams docker build output for visibility on long builds (some
//! Dockerfiles have 30+ minute builds; a buffered runner would
//! look hung). The captured exit code drives the audit `applied`
//! field exactly like `pull`.
//!
//! Revert: `revert.kind = unsupported`. Image build is fundamentally
//! non-reversible — the previous tagged image may still exist in the
//! local cache, but that's a docker-internal detail.

use std::time::Instant;

use anyhow::Result;
use serde_json::json;

use crate::cli::ComposeBuildArgs;
use crate::error::ExitKind;
use crate::safety::gate::ConfirmResult;
use crate::safety::{AuditEntry, AuditStore, Confirm, Revert, SafetyGate};
use crate::ssh::exec::RunOpts;
use crate::verbs::output::{NextStep, OutputDoc};
use crate::verbs::runtime::{current_runner, resolve_target};

use super::resolve::{project_in_profile, Parsed};
use super::write_common::{build_compose_cmd, compose_file_sha_short, project_tags};

pub fn run(args: ComposeBuildArgs) -> Result<ExitKind> {
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

    let mut flags: Vec<&str> = Vec::new();
    if args.no_cache {
        flags.push("--no-cache");
    }
    if args.pull {
        flags.push("--pull");
    }
    let cmd = build_compose_cmd(&project, "build", &flags, parsed.service.as_deref());

    let runner = current_runner();
    let (_resolved, target) = resolve_target(&parsed.namespace)?;

    let gate = SafetyGate::new(args.apply, args.yes, args.yes_all);
    if !gate.should_apply() {
        let scope = parsed
            .service
            .as_deref()
            .map(|s| format!("service {s}"))
            .unwrap_or_else(|| "every service".to_string());
        let doc = OutputDoc::new(
            format!(
                "DRY RUN. Would `compose build` ({scope}) on {ns}/{p}",
                ns = parsed.namespace,
                p = project.name
            ),
            json!({
                "namespace": parsed.namespace,
                "project": project.name,
                "service": parsed.service,
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

    // 1-hour timeout: builds with multi-stage Dockerfiles + cold
    // caches over slow networks can exceed 30 minutes. Streaming
    // makes the wait visible.
    let started = Instant::now();
    let mut printed = 0usize;
    let out = runner.run_streaming_capturing(
        &parsed.namespace,
        &target,
        &cmd,
        RunOpts::with_timeout(60 * 60),
        &mut |line| {
            crate::transcript::emit_stdout(line);
            printed += 1;
        },
    )?;
    let dur = started.elapsed().as_millis() as u64;

    let mut entry = AuditEntry::new(
        "compose.build",
        &format!(
            "{}/{}{}",
            parsed.namespace,
            project.name,
            parsed
                .service
                .as_deref()
                .map(|s| format!("/{s}"))
                .unwrap_or_default()
        ),
    );
    entry.exit = out.exit_code;
    entry.duration_ms = dur;
    entry.streamed = true;
    entry.reason = crate::safety::validate_reason(args.reason.as_deref())?;
    entry.applied = Some(out.ok());
    entry.rendered_cmd = Some(crate::redact::redact_for_audit(&cmd).into_owned());
    let mut extras: Vec<&str> = Vec::new();
    if args.no_cache {
        extras.push("[no_cache=true]");
    }
    if args.pull {
        extras.push("[pull=true]");
    }
    entry.args = project_tags(
        &project.name,
        parsed.service.as_deref(),
        &compose_hash,
        &extras,
    );
    entry.revert = Some(Revert::unsupported(
        "compose build has no inverse — image cache changes are not reversible \
         (the previous tag may exist locally but that's not a contract)"
            .to_string(),
    ));
    let store = AuditStore::open()?;
    store.append(&entry)?;

    let summary = if out.ok() {
        format!(
            "compose build {ns}/{p} ok ({lines} build line(s), audit_id={id}, {dur}ms)",
            ns = parsed.namespace,
            p = project.name,
            lines = printed,
            id = entry.id,
            dur = dur
        )
    } else {
        format!(
            "compose build {ns}/{p} FAILED (exit {code}, audit_id={id})",
            ns = parsed.namespace,
            p = project.name,
            code = out.exit_code,
            id = entry.id
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
            "lines_streamed": printed,
        }),
    )
    .with_meta("selector", args.selector.clone())
    .with_quiet(args.format.quiet);
    if out.ok() {
        doc.push_next(NextStep::new(
            format!(
                "inspect compose up {ns}/{p} --apply",
                ns = parsed.namespace,
                p = project.name
            ),
            "bring services up using the freshly built image(s)",
        ));
    }
    crate::format::render::render_doc(&doc, &fmt, &[])?;

    Ok(if out.ok() {
        ExitKind::Success
    } else {
        ExitKind::Error
    })
}
