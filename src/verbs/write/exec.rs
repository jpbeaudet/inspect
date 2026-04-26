//! `inspect exec <sel> -- <cmd>` (bible §8).
//!
//! Runs a free-form command on the target. `--apply` required (no
//! preview semantics — the command is itself the action).

use std::time::Instant;

use anyhow::Result;

use crate::cli::ExecArgs;
use crate::error::ExitKind;
use crate::safety::gate::ConfirmResult;
use crate::safety::{AuditEntry, AuditStore, Confirm, SafetyGate};
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::Renderer;
use crate::verbs::quote::shquote;

pub fn run(args: ExecArgs) -> Result<ExitKind> {
    if args.cmd.is_empty() {
        crate::error::emit("exec requires a command after `--`");
        return Ok(ExitKind::Error);
    }
    let user_cmd = args.cmd.join(" ");

    let (runner, nses, targets) = plan(&args.selector)?;
    let steps: Vec<_> = iter_steps(&nses, &targets).collect();
    if steps.is_empty() {
        crate::error::emit(format!("'{}' matched no targets", args.selector));
        return Ok(ExitKind::Error);
    }

    // Field pitfall §3.2: `exec` is the only write verb whose payload
    // is opaque user-supplied shell — `--apply` alone is not a strong
    // enough signal. Require `--allow-exec` as a second confirmation
    // when the operator actually intends to run the command. Tighten
    // the large-fanout interlock from the default 10 down to 3 so a
    // typo cannot shell out across more than a couple of hosts before
    // the prompt fires.
    let mut gate = SafetyGate::new(args.apply, args.yes, args.yes_all);
    gate.fanout_threshold = exec_fanout_threshold();
    if gate.should_apply() && !args.allow_exec {
        eprintln!(
            "error: `inspect exec` is opaque, free-form remote shell. \
             Pass `--allow-exec` in addition to `--apply` to confirm intent."
        );
        return Ok(ExitKind::Error);
    }
    if !gate.should_apply() {
        let mut r = Renderer::new();
        r.summary(format!("DRY RUN. Would exec on {} target(s):", steps.len()));
        for s in &steps {
            let svc = s.service().map(|x| format!("/{x}")).unwrap_or_default();
            r.data_line(format!("{}{svc}: {user_cmd}", s.ns.namespace));
        }
        r.next("Re-run with --apply --allow-exec to execute");
        r.print();
        return Ok(ExitKind::Success);
    }
    if let ConfirmResult::Aborted(why) =
        gate.confirm(Confirm::LargeFanout, steps.len(), "Continue?")
    {
        eprintln!("aborted: {why}");
        return Ok(ExitKind::Error);
    }

    let store = AuditStore::open()?;
    let mut ok = 0usize;
    let mut bad = 0usize;
    let mut renderer = Renderer::new();

    for s in &steps {
        let cmd = match s.service() {
            Some(svc) => format!("docker exec {} sh -c {}", shquote(svc), shquote(&user_cmd)),
            None => user_cmd.clone(),
        };
        let started = Instant::now();
        let out = runner.run(
            &s.ns.namespace,
            &s.ns.target,
            &cmd,
            RunOpts::with_timeout(args.timeout_secs.unwrap_or(120)),
        )?;
        let dur = started.elapsed().as_millis() as u64;

        let mut e = AuditEntry::new(
            "exec",
            &format!(
                "{}{}",
                s.ns.namespace,
                s.service().map(|x| format!("/{x}")).unwrap_or_default()
            ),
        );
        e.args = user_cmd.clone();
        e.exit = out.exit_code;
        e.duration_ms = dur;
        store.append(&e)?;

        let label = format!(
            "{}{}",
            s.ns.namespace,
            s.service().map(|x| format!("/{x}")).unwrap_or_default()
        );
        if out.ok() {
            ok += 1;
            renderer.data_line(format!("{label}: ok ({}ms)", dur));
            if !out.stdout.trim().is_empty() {
                for line in out.stdout.lines() {
                    renderer.data_line(format!("  {line}"));
                }
            }
        } else {
            bad += 1;
            // Field pitfall §7.3: distroless / scratch images have no
            // shell, so `docker exec ... sh -c ...` fails with the
            // OCI runtime error below. Translate the runtime-spec
            // wall-of-text into a one-line, actionable message so
            // the operator knows to either install a shell in the
            // image or use `docker cp` for file IO.
            let stderr_msg = if looks_like_no_shell(&out.stderr) {
                format!(
                    "container has no `sh` (distroless/scratch image): \
                     `inspect exec` requires a shell on the target. \
                     Use `inspect cp` for file transfer, or rebuild the image with a busybox/alpine layer."
                )
            } else {
                out.stderr.trim().to_string()
            };
            renderer.data_line(format!(
                "{label}: FAILED (exit {}): {}",
                out.exit_code, stderr_msg
            ));
        }
    }

    renderer
        .summary(format!("exec: {ok} ok, {bad} failed"))
        .next("inspect audit ls");
    renderer.print();
    Ok(if bad == 0 {
        ExitKind::Success
    } else {
        ExitKind::Error
    })
}

/// Field pitfall §3.2: detect docker's runtime-spec error for "no
/// shell in image". Three observed phrasings across docker 20.x-25.x
/// (and the equivalent containerd/CRI message).
pub(crate) fn looks_like_no_shell(stderr: &str) -> bool {
    let s = stderr;
    s.contains("\"sh\": executable file not found")
        || s.contains("exec: \"sh\":")
        || (s.contains("OCI runtime exec failed") && s.contains("sh"))
        || s.contains("starting container process caused: exec: \"sh\"")
}

/// Field pitfall §3.2: large-fanout interlock threshold for `exec`.
/// Defaults to 3 (vs. the 10 used by predictable verbs) so a stray
/// glob in the selector cannot silently shell out across the fleet.
/// Operators who genuinely need a higher cap can raise it via
/// `INSPECT_EXEC_FANOUT_THRESHOLD=<n>`; pass `--yes-all` to skip the
/// prompt entirely once the threshold fires.
fn exec_fanout_threshold() -> usize {
    if let Ok(s) = std::env::var("INSPECT_EXEC_FANOUT_THRESHOLD") {
        if let Ok(n) = s.parse::<usize>() {
            if n >= 1 {
                return n;
            }
        }
    }
    3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_distroless_no_shell() {
        // OCI runtime spec wording on Docker 20-25.
        let s = "OCI runtime exec failed: exec failed: unable to start container process: \
                 exec: \"sh\": executable file not found in $PATH: unknown";
        assert!(looks_like_no_shell(s));

        // Containerd / CRI wording.
        let s2 = "starting container process caused: exec: \"sh\": executable file not found";
        assert!(looks_like_no_shell(s2));

        // Genuine other failure must not trip the heuristic.
        assert!(!looks_like_no_shell("permission denied"));
        assert!(!looks_like_no_shell("container not found"));
    }
}
