//! `inspect run <sel> -- <cmd>` (P6, v0.1.1).
//!
//! Read-only execution counterpart to [`crate::verbs::write::exec`]. Streams
//! the remote command's output line-by-line, never touches the audit log,
//! and has no `--apply`/confirmation gating. Use when you want to inspect
//! state with an ad-hoc shell snippet (`ps`, `cat /proc/...`, `df -h`,
//! `redis-cli info`, ...) without paying for the write-verb interlock.
//!
//! Field-pitfall driver: P6 in [INSPECT_v0.1.1_PATCH_SPEC.md]. Operators
//! routinely typed `inspect exec ... -- <read-only thing>` and ran into
//! the exec safety prompts on every iteration.

use anyhow::Result;

use crate::cli::RunArgs;
use crate::error::ExitKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::{Envelope, JsonOut, Renderer};
use crate::verbs::quote::shquote;

pub fn run(args: RunArgs) -> Result<ExitKind> {
    if args.cmd.is_empty() {
        crate::error::emit("run requires a command after `--`");
        return Ok(ExitKind::Error);
    }
    let user_cmd = args.cmd.join(" ");

    // Reason is informational for `run` (not audited). Validate length so the
    // operator gets a useful error before we dial out to remote hosts.
    let reason = crate::safety::validate_reason(args.reason.as_deref())?;
    if let Some(r) = &reason {
        eprintln!("# reason: {r}");
    }

    let fmt = args.format.resolve()?;
    let json = matches!(fmt, crate::format::OutputFormat::Json);

    let (runner, nses, targets) = plan(&args.selector)?;
    let steps: Vec<_> = iter_steps(&nses, &targets).collect();
    if steps.is_empty() {
        crate::error::emit(format!("'{}' matched no targets", args.selector));
        return Ok(ExitKind::Error);
    }

    let timeout_secs = args.timeout_secs.unwrap_or(120);
    let mut ok = 0usize;
    let mut bad = 0usize;

    for s in &steps {
        if crate::exec::cancel::is_cancelled() {
            break;
        }
        let svc_label = s.service().map(|x| format!("/{x}")).unwrap_or_default();
        let label = format!("{}{}", s.ns.namespace, svc_label);

        // Wrap in `docker exec` when the selector points at a container.
        // Apply server-side line filter (--filter-line-pattern) by piping
        // through `grep -E`, mirroring the same pushdown logs/grep use.
        let inner = match s.container() {
            Some(container) => format!("docker exec {} sh -c {}", shquote(container), shquote(&user_cmd)),
            None => user_cmd.clone(),
        };
        let cmd = match &args.filter_line_pattern {
            Some(pat) => format!("{inner} | grep -E {}", shquote(pat)),
            None => inner,
        };

        let opts = RunOpts::with_timeout(timeout_secs);
        let svc_name = s.service().unwrap_or("_").to_string();
        let ns_name = s.ns.namespace.clone();

        let exit = runner.run_streaming(&ns_name, &s.ns.target, &cmd, opts, &mut |line| {
            if json {
                JsonOut::write(
                    &Envelope::new(&ns_name, "run", "run")
                        .with_service(&svc_name)
                        .put(
                            "line",
                            crate::format::safe::safe_machine_line(line).as_ref(),
                        ),
                );
            } else {
                let safe = crate::format::safe::safe_terminal_line(
                    line,
                    crate::format::safe::DEFAULT_MAX_LINE_BYTES,
                );
                println!("{label} | {safe}");
            }
        });

        match exit {
            Ok(0) => ok += 1,
            Ok(code) => {
                bad += 1;
                if !json {
                    eprintln!("{label}: exit {code}");
                }
            }
            Err(e) => {
                bad += 1;
                if !json {
                    eprintln!("{label}: {e}");
                }
            }
        }
    }

    if !json {
        let mut r = Renderer::new();
        r.summary(format!("run: {ok} ok, {bad} failed"));
        r.print();
    }
    Ok(if bad == 0 {
        ExitKind::Success
    } else {
        ExitKind::Error
    })
}
