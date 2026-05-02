//! F6 (v0.1.3): `inspect compose logs <ns>/<project>[/<service>]`
//! — aggregated logs for a project, or one service inside it.
//!
//! Wraps `cd <wd> && docker compose -p <p> logs [--tail N] [--since
//! X] [--follow] [<svc>]` over the persistent ssh socket. The
//! flag set is intentionally a subset of `inspect logs`'s flags —
//! the cross-medium / merged / `--match`/`--exclude` / cursor /
//! since-last surface is deferred so the v0.1.3 contract stays
//! crisp. Operators who need those features fall back to
//! `inspect logs <ns>/<service>` (which F5's resolver now finds
//! via the compose-service label).

use anyhow::Result;

use crate::cli::ComposeLogsArgs;
use crate::error::ExitKind;
use crate::redact::OutputRedactor;
use crate::ssh::exec::RunOpts;
use crate::verbs::quote::shquote;
use crate::verbs::runtime::{current_runner, resolve_target, RemoteRunner};

use super::resolve::{project_in_profile, Parsed};

pub fn run(args: ComposeLogsArgs) -> Result<ExitKind> {
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

    let runner = current_runner();
    let (_resolved, target) = resolve_target(&parsed.namespace)?;

    // Build the docker compose logs invocation.
    let mut parts: Vec<String> = vec![
        format!("cd {wd} &&", wd = shquote(&project.working_dir)),
        format!("docker compose -p {p} logs", p = shquote(&project.name)),
        // `--no-color` so the redaction pipeline's regexes don't
        // have to fight ANSI sequences. Operators who need color
        // can drop back to `inspect run -- 'docker compose logs ...'`.
        "--no-color".into(),
        // `--no-log-prefix` would strip the `[svc]` prefix; we
        // *want* it, both for human reading and for the JSON
        // envelope's `service` field, so we leave the default on.
    ];
    if let Some(tail) = args.tail {
        parts.push(format!("--tail {tail}"));
    }
    if let Some(since) = args.since.as_deref() {
        parts.push(format!("--since {}", shquote(since)));
    }
    if args.follow {
        parts.push("--follow".into());
    }
    if let Some(svc) = parsed.service.as_deref() {
        parts.push(shquote(svc));
    }
    let cmd = parts.join(" ");

    // Streaming with redaction. We pipe each line through the
    // L7 maskers and emit in real time so `--follow` is responsive.
    let redactor = OutputRedactor::new(args.show_secrets, false);
    // Long timeout for `--follow`, normal otherwise — matches
    // `inspect logs --follow`'s 8h convention.
    let timeout = if args.follow {
        RunOpts::with_timeout(8 * 3600)
    } else {
        RunOpts::with_timeout(60)
    };
    let exit = stream_with_redaction(
        runner.as_ref(),
        &parsed.namespace,
        &target,
        &cmd,
        timeout,
        &redactor,
    )?;
    if exit == 0 {
        Ok(ExitKind::Success)
    } else {
        Ok(ExitKind::Error)
    }
}

/// Run the remote command in streaming mode and emit each line
/// through the redactor. Returns the remote exit code.
fn stream_with_redaction(
    runner: &dyn RemoteRunner,
    ns: &str,
    target: &crate::ssh::options::SshTarget,
    cmd: &str,
    opts: RunOpts,
    redactor: &OutputRedactor,
) -> Result<i32> {
    runner.run_streaming(ns, target, cmd, opts, &mut |line| {
        if let Some(masked) = redactor.mask_line(line) {
            crate::transcript::emit_stdout(&masked);
        }
    })
}
