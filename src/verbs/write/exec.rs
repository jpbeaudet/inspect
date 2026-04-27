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

    // Field pitfall §3.2: `exec` is the one write verb whose payload
    // is opaque user-supplied shell. The large-fanout interlock fires
    // at a tighter threshold (3 instead of 10) so a stray glob in the
    // selector cannot silently shell out across more than a couple of
    // hosts before the prompt fires. v0.1.1 dropped the separate
    // `--allow-exec` second-confirmation flag in favour of the read/
    // write split (`inspect run` for read, `inspect exec --apply`
    // for write); see [INSPECT_v0.1.1_PATCH_SPEC.md] P6/P7.
    let mut gate = SafetyGate::new(args.apply, args.yes, args.yes_all);
    gate.fanout_threshold = exec_fanout_threshold();
    if !gate.should_apply() {
        let mut r = Renderer::new();
        r.summary(format!("DRY RUN. Would exec on {} target(s):", steps.len()));
        for s in &steps {
            let svc = s.service().map(|x| format!("/{x}")).unwrap_or_default();
            r.data_line(format!("{}{svc}: {user_cmd}", s.ns.namespace));
        }
        r.next("Re-run with --apply to execute");
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
    let masker = crate::redact::EnvSecretMasker::new(args.show_secrets, args.redact_all);
    let mut last_inner: Option<i32> = None;
    let mut all_same = true;

    for s in &steps {
        let cmd = match s.container() {
            Some(container) => {
                format!("docker exec {} sh -c {}", shquote(container), shquote(&user_cmd))
            }
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
        // P4: stamp audit args with whether the operator opted into
        // `--show-secrets` AND whether masking actually fired during
        // this run, so post-hoc reviewers can distinguish verbatim
        // output from masked output.
        e.args = if args.show_secrets {
            format!("{user_cmd} [secrets_exposed=true]")
        } else if masker.was_active() {
            format!("{user_cmd} [secrets_masked=true]")
        } else {
            user_cmd.clone()
        };
        e.exit = out.exit_code;
        e.duration_ms = dur;
        e.reason = crate::safety::validate_reason(args.reason.as_deref())?;
        store.append(&e)?;

        if let Some(prev) = last_inner {
            if prev != out.exit_code {
                all_same = false;
            }
        }
        last_inner = Some(out.exit_code);

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
                    let masked = masker.mask_line(line);
                    renderer.data_line(format!("  {}", masked));
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
                "container has no `sh` (distroless/scratch image): \
                 `inspect exec` requires a shell on the target. \
                 Use `inspect cp` for file transfer, or rebuild the image with a busybox/alpine layer."
                    .to_string()
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

    // P11: surface the remote command's exit code when the run was
    // single-target or every target returned the same code. Mixed
    // exits collapse to ExitKind::Error to keep `set -e` scripts safe.
    if let Some(inner_code) = last_inner {
        if all_same {
            return Ok(ExitKind::Inner(crate::error::clamp_inner_exit(inner_code)));
        }
    }
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
