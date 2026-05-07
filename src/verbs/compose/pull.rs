//! F6 (v0.1.3): `inspect compose pull <ns>/<project>[/<service>]`
//! — pull images for a compose project. Audited (`verb=compose.pull`).
//!
//! Streams docker pull progress lines via the streaming-capturing
//! runner so operators see what's happening during multi-minute
//! pulls (a buffered runner would silently hold for 10+ minutes on
//! a large image and look hung). The captured output is preserved
//! in the audit entry's `rendered_cmd` neighbourhood for later
//! forensic review.
//!
//! Revert: `revert.kind = unsupported`. There is no `unpull` —
//! the previous image tag may still be in the local cache, but
//! that's a docker-internal detail, not a contract.

use std::time::Instant;

use anyhow::Result;
use serde_json::json;

use crate::cli::ComposePullArgs;
use crate::error::ExitKind;
use crate::safety::gate::ConfirmResult;
use crate::safety::{AuditEntry, AuditStore, Confirm, Revert, SafetyGate};
use crate::ssh::exec::RunOpts;
use crate::verbs::output::OutputDoc;
use crate::verbs::runtime::{current_runner, resolve_target};

use super::resolve::{project_in_profile, Parsed};
use super::write_common::{build_compose_cmd, compose_file_sha_short, project_tags};

pub fn run(args: ComposePullArgs) -> Result<ExitKind> {
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
    if args.ignore_pull_failures {
        flags.push("--ignore-pull-failures");
    }
    let cmd = build_compose_cmd(&project, "pull", &flags, parsed.service.as_deref());

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
                "DRY RUN. Would `compose pull` ({scope}) on {ns}/{p}",
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

    // 30-minute timeout: large multi-arch images on slow networks
    // can legitimately take that long. The streaming runner makes
    // the wait visible, so a hung pull is obvious in real time.
    let started = Instant::now();
    let mut printed = 0usize;
    let out = runner.run_streaming_capturing(
        &parsed.namespace,
        &target,
        &cmd,
        RunOpts::with_timeout(30 * 60),
        &mut |line| {
            crate::transcript::emit_stdout(line);
            printed += 1;
        },
    )?;
    let dur = started.elapsed().as_millis() as u64;

    let mut entry = AuditEntry::new(
        "compose.pull",
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
    let extras = if args.ignore_pull_failures {
        vec!["[ignore_pull_failures=true]"]
    } else {
        vec![]
    };
    entry.args = project_tags(
        &project.name,
        parsed.service.as_deref(),
        &compose_hash,
        &extras,
    );
    entry.revert = Some(Revert::unsupported(
        "compose pull has no inverse — image cache changes are not reversible \
         through inspect (`docker image rm` would only delete the new tag)"
            .to_string(),
    ));
    let store = AuditStore::open()?;
    store.append(&entry)?;

    let summary = if out.ok() {
        format!(
            "compose pull {ns}/{p} ok ({lines} progress line(s), audit_id={id}, {dur}ms)",
            ns = parsed.namespace,
            p = project.name,
            lines = printed,
            id = entry.id,
            dur = dur
        )
    } else {
        format!(
            "compose pull {ns}/{p} FAILED (exit {code}, audit_id={id})",
            ns = parsed.namespace,
            p = project.name,
            code = out.exit_code,
            id = entry.id
        )
    };

    let doc = OutputDoc::new(
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
    // Streaming output already hit stdout; the human renderer's
    // DATA section stays empty so we don't double-print.
    let exit = crate::format::render::render_doc(&doc, &fmt, &[], args.format.select_spec())?;

    // Exec failure exit class takes precedence over filter-class.
    Ok(if out.ok() { exit } else { ExitKind::Error })
}
