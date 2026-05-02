//! F6 (v0.1.3): `inspect compose exec <ns>/<project>/<service> -- <cmd>`
//! — run a command inside a compose service container. Mirrors
//! `inspect run`'s contract:
//!
//! - **Not audited.** The operator's intent is inspection / fast
//!   iteration inside a running service container, not a recorded
//!   mutation. For audited mutations operators should use the
//!   purpose-built write verbs (`compose restart`, `compose up`,
//!   `compose down`) or `inspect exec` (the generic write verb).
//! - **No apply gate.** Mirrors `inspect run` exactly.
//! - **Output redacted** through the L7 four-masker pipeline
//!   unless `--show-secrets` is passed; `--redact-all` masks every
//!   `KEY=VALUE` line regardless of key name.
//! - **`-u` / `-w` passthrough** to `docker compose exec` for
//!   per-invocation user / working-directory overrides.

use anyhow::Result;

use crate::cli::ComposeExecArgs;
use crate::error::ExitKind;
use crate::redact::OutputRedactor;
use crate::ssh::exec::RunOpts;
use crate::ssh::options::SshTarget;
use crate::verbs::quote::shquote;
use crate::verbs::runtime::{current_runner, resolve_target, RemoteRunner};

use super::resolve::{project_in_profile, Parsed};

pub fn run(args: ComposeExecArgs) -> Result<ExitKind> {
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
                 expected '<ns>/<project>/<service>'",
                args.selector
            ));
            return Ok(ExitKind::Error);
        }
    };
    let service = match parsed.service.as_deref() {
        Some(s) => s,
        None => {
            crate::error::emit(format!(
                "selector '{}' is missing the service portion — \
                 compose exec requires a target service. \
                 hint: `inspect compose ps {}/{}` lists services in this project.",
                args.selector, parsed.namespace, project_name
            ));
            return Ok(ExitKind::Error);
        }
    };
    if args.cmd.is_empty() {
        crate::error::emit(
            "compose exec requires a command after `--`, e.g. \
             `inspect compose exec arte/luminary-onyx/onyx-vault -- ps -ef`",
        );
        return Ok(ExitKind::Error);
    }
    let (_profile, project) = match project_in_profile(&parsed.namespace, project_name) {
        Ok(t) => t,
        Err(e) => {
            crate::error::emit(format!("{e}"));
            return Ok(ExitKind::NoMatches);
        }
    };

    // Echo the operator's reason to stderr — same UX as `inspect run`.
    if let Some(r) = args.reason.as_deref() {
        if !r.trim().is_empty() {
            crate::tee_eprintln!("[inspect compose exec] reason: {}", r.trim());
        }
    }

    // Build flags. `-T` disables docker's pseudo-TTY allocation,
    // which is what we want when piping output through a non-tty
    // ssh master + the redactor; without `-T`, docker would emit
    // ANSI control sequences that confuse the line-oriented maskers.
    let mut parts: Vec<String> = vec![
        format!("cd {wd} &&", wd = shquote(&project.working_dir)),
        format!("docker compose -p {p} exec -T", p = shquote(&project.name)),
    ];
    if let Some(u) = args.user.as_deref() {
        parts.push(format!("-u {}", shquote(u)));
    }
    if let Some(w) = args.workdir.as_deref() {
        parts.push(format!("-w {}", shquote(w)));
    }
    parts.push(shquote(service));
    // Forward the operator's command verbatim — every arg shquoted
    // so spaces / quotes survive the round-trip.
    for c in &args.cmd {
        parts.push(shquote(c));
    }
    let cmd = parts.join(" ");

    let runner = current_runner();
    let (_resolved, target) = resolve_target(&parsed.namespace)?;

    // Stream + redact, mirroring `inspect run`'s discipline.
    let redactor = OutputRedactor::new(args.show_secrets, args.redact_all);
    let exit = stream_with_redaction(
        runner.as_ref(),
        &parsed.namespace,
        &target,
        &cmd,
        // No fixed timeout for compose exec — the operator's
        // command might be a quick `ls` or a multi-hour
        // `psql -c '\\timing'`. Default 8h matches inspect logs --follow.
        RunOpts::with_timeout(8 * 3600),
        &redactor,
    )?;
    if exit == 0 {
        Ok(ExitKind::Success)
    } else {
        // Inner-process exit code is preserved as the inspect exit
        // code on `compose exec`, mirroring `inspect run`'s
        // contract. Non-zero exits land here as `Error` (exit 2);
        // a future enhancement could pass through the raw inner
        // code, but that would require widening ExitKind. v0.1.3
        // ships the conservative shape.
        Ok(ExitKind::Error)
    }
}

fn stream_with_redaction(
    runner: &dyn RemoteRunner,
    ns: &str,
    target: &SshTarget,
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
