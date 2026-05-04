//! `inspect exec <sel> -- <cmd>` (bible §8).
//!
//! Runs a free-form command on the target. `--apply` required (no
//! preview semantics — the command is itself the action).
//!
//! v0.1.2 (B7): output is streamed line-by-line to the operator's
//! terminal as the remote command produces it, instead of being
//! buffered until exit. A background heartbeat thread emits
//! `[inspect] still running on <ns> (Ns elapsed)` to stderr after
//! `--heartbeat <secs>` (default 30s) of remote silence so the
//! operator can tell `pg_dump` is alive vs. wedged.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::cli::ExecArgs;
use crate::error::ExitKind;
use crate::safety::gate::ConfirmResult;
use crate::safety::{AuditEntry, AuditStore, Confirm, Revert, SafetyGate};
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
    // F11 (v0.1.3): exec is the one write verb whose payload is
    // free-form shell, so we cannot synthesise an inverse. Refuse
    // `--apply` unless the operator opts in via `--no-revert`.
    if args.apply && !args.no_revert {
        crate::error::emit(
            "`inspect exec --apply` requires `--no-revert` because the inverse cannot be \
             synthesised from a free-form shell command. If the change is structured (file, \
             permission, lifecycle), use the matching write verb (`inspect put`, `inspect chmod`, \
             `inspect restart`) which captures a real inverse.",
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
    let mut last_inner: Option<i32> = None;
    let mut all_same = true;

    // F12 (v0.1.3): per-invocation env-overlay overrides. Validate
    // once before the per-step loop so a typo in `--env` short-
    // circuits the whole exec invocation.
    let user_env: Vec<(String, String)> = {
        let mut out = Vec::with_capacity(args.env.len());
        for raw in &args.env {
            out.push(crate::exec::env_overlay::parse_kv(raw)?);
        }
        out
    };

    // B7: heartbeat configuration. 0 = disabled.
    let heartbeat_secs: u64 = if args.no_heartbeat {
        0
    } else {
        args.heartbeat.unwrap_or(30)
    };
    // F13 (v0.1.3): track transport-class outcomes across steps.
    let mut uniform_transport: Option<crate::ssh::transport::TransportClass> = None;
    let mut transport_failures = 0usize;

    for s in &steps {
        // L7 (v0.1.3): one redactor per step. PEM block state must
        // not leak across steps because a step truncated mid-block
        // would otherwise poison the next step's detection. The
        // composer is cheap to construct (regex are global Lazy).
        let redactor = crate::redact::OutputRedactor::new(args.show_secrets, args.redact_all);
        let cmd = match s.container() {
            Some(container) => {
                format!(
                    "docker exec {} sh -c {}",
                    shquote(container),
                    shquote(&user_cmd)
                )
            }
            None => user_cmd.clone(),
        };
        // F12 (v0.1.3): apply per-namespace env overlay (merged with
        // `--env` overrides). Empty overlay → cmd unchanged.
        let effective_overlay =
            crate::exec::env_overlay::merge(Some(&s.ns.env_overlay), &user_env, args.env_clear);
        let cmd = crate::exec::env_overlay::apply_to_cmd(&cmd, &effective_overlay).into_owned();
        if args.debug {
            eprintln!("[inspect] rendered command for {}: {}", s.ns.namespace, cmd);
        }
        let started = Instant::now();
        let label = format!(
            "{}{}",
            s.ns.namespace,
            s.service().map(|x| format!("/{x}")).unwrap_or_default()
        );

        // B7: heartbeat thread. Wakes every 500ms and, if no remote line
        // has arrived for `heartbeat_secs`, emits a single line to stderr.
        let last_seen = Arc::new(Mutex::new(Instant::now()));
        let stop_heartbeat = Arc::new(AtomicBool::new(false));
        let heartbeat_handle = if heartbeat_secs > 0 {
            let last = Arc::clone(&last_seen);
            let stop = Arc::clone(&stop_heartbeat);
            let label_hb = label.clone();
            let interval = Duration::from_secs(heartbeat_secs);
            let started_hb = started;
            Some(std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(500));
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let last_t = *last.lock().unwrap();
                    if last_t.elapsed() >= interval {
                        let elapsed = started_hb.elapsed().as_secs();
                        eprintln!("[inspect] still running on {label_hb} ({elapsed}s elapsed)");
                        // Reset so we don't spam every 500ms after the
                        // threshold trips — we want one heartbeat per
                        // `interval` of silence.
                        *last.lock().unwrap() = Instant::now();
                    }
                }
            }))
        } else {
            None
        };

        // B7: stream stdout live; capture into a buffer for the audit log.
        // The closure prints each line as it arrives so the operator sees
        // progress in real time, and pokes `last_seen` so the heartbeat
        // thread knows the remote is still talking.
        let last_seen_cb = Arc::clone(&last_seen);
        let redactor_ref = &redactor;
        let mut on_line = |line: &str| {
            *last_seen_cb.lock().unwrap() = Instant::now();
            // L7 (v0.1.3): the redactor returns None for lines inside
            // (or ending) an active PEM private-key block; we skip
            // emission entirely so the BEGIN-line marker is the only
            // output for the whole block.
            let Some(masked) = redactor_ref.mask_line(line) else {
                return;
            };
            // Indented to match the previous `data_line("  {}", ...)`
            // shape so transcripts and audit log readers don't shift.
            println!("  {}", masked);
        };

        let stream_result = {
            let policy = crate::exec::dispatch::ReauthPolicy {
                allow_reauth: !args.no_reauth && s.ns.auto_reauth,
            };
            let cmd_ref = &cmd;
            let timeout = args.timeout_secs.unwrap_or(120);
            let runner_ref = runner.as_ref();
            crate::exec::dispatch::dispatch_with_reauth(
                &s.ns.namespace,
                &s.ns.target,
                runner_ref,
                Some(&store),
                "exec",
                &label,
                policy,
                || {
                    runner_ref.run_streaming_capturing(
                        &s.ns.namespace,
                        &s.ns.target,
                        cmd_ref,
                        RunOpts::with_timeout(timeout),
                        &mut on_line,
                    )
                },
            )
        };

        // Stop the heartbeat regardless of success/failure.
        stop_heartbeat.store(true, Ordering::Relaxed);
        if let Some(h) = heartbeat_handle {
            let _ = h.join();
        }

        let dur = started.elapsed().as_millis() as u64;

        // F13: classify dispatch outcome and split the existing
        // success / command-failed code paths from the new
        // transport-failure path.
        let (out, dispatch_class, dispatch_retried, dispatch_reauth_id) =
            match (stream_result.result, stream_result.failure_class) {
                (Ok(out), _) => (
                    Some(out),
                    Option::<crate::ssh::transport::TransportClass>::None,
                    stream_result.retried,
                    stream_result.reauth_id,
                ),
                (Err(e), Some(class)) => {
                    bad += 1;
                    all_same = false;
                    transport_failures += 1;
                    uniform_transport = match uniform_transport {
                        None if transport_failures == 1 => Some(class),
                        Some(prev) if prev == class => Some(prev),
                        _ => None,
                    };
                    renderer.data_line(format!(
                        "{label}: FAILED ({class}): {e}",
                        class = class.as_str()
                    ));
                    let mut entry = AuditEntry::new("exec", &label);
                    entry.args = stamp_args(&user_cmd, args.show_secrets, &redactor);
                    entry.exit = -1;
                    entry.duration_ms = dur;
                    entry.reason = crate::safety::validate_reason(args.reason.as_deref())?;
                    entry.diff_summary = format!("transport_error: {e}");
                    if !effective_overlay.is_empty() {
                        entry.env_overlay = Some(effective_overlay.clone());
                    }
                    // G2: redact wrapped shell command unless the
                    // operator opted into `--show-secrets`.
                    entry.rendered_cmd = Some(if args.show_secrets {
                        cmd.clone()
                    } else {
                        crate::redact::redact_for_audit(&cmd).into_owned()
                    });
                    entry.secrets_masked_kinds = collect_kinds(&redactor);
                    entry.failure_class = Some(class.as_str().to_string());
                    if stream_result.retried {
                        entry.retry_of = Some(format!("transport_stale@{label}"));
                    }
                    if let Some(rid) = &stream_result.reauth_id {
                        entry.reauth_id = Some(rid.clone());
                    }
                    let _ = store.append(&entry);
                    continue;
                }
                (Err(e), None) => return Err(e),
            };
        let out = out.unwrap();
        let _ = dispatch_class;

        let mut e = AuditEntry::new(
            "exec",
            &format!(
                "{}{}",
                s.ns.namespace,
                s.service().map(|x| format!("/{x}")).unwrap_or_default()
            ),
        );
        // P4 (v0.1.1) + L7 (v0.1.3): stamp audit args with whether
        // the operator opted into `--show-secrets` AND whether the
        // redactor fired during this step, so post-hoc reviewers can
        // distinguish verbatim output from masked output. The
        // `secrets_masked_kinds` field records which kinds of pattern
        // (`pem` / `header` / `url` / `env`) almost leaked.
        e.args = stamp_args(&user_cmd, args.show_secrets, &redactor);
        e.exit = out.exit_code;
        e.duration_ms = dur;
        e.reason = crate::safety::validate_reason(args.reason.as_deref())?;
        // F11 (v0.1.3): exec records `unsupported` revert + the
        // operator's explicit `--no-revert` acknowledgement so audit
        // readers can tell free-form mutations apart from mutations
        // that simply pre-date the contract.
        // G2 (post-v0.1.3 audit hardening): the preview text mirrors
        // the original user command, so redact it the same way as
        // the args field unless the operator opted into
        // `--show-secrets`.
        let preview_cmd: std::borrow::Cow<'_, str> = if args.show_secrets {
            std::borrow::Cow::Borrowed(user_cmd.as_str())
        } else {
            crate::redact::redact_for_audit(&user_cmd)
        };
        e.revert = Some(Revert::unsupported(format!(
            "exec is free-form shell; no inverse captured. Original cmd: {preview_cmd}"
        )));
        e.no_revert_acknowledged = true;
        e.applied = Some(out.ok());
        if !effective_overlay.is_empty() {
            e.env_overlay = Some(effective_overlay.clone());
        }
        // G2: the rendered shell command is the wrapped form
        // (`docker exec ... sh -c '<user_cmd>'`) which still embeds
        // any secrets the operator typed. Redact in the same
        // show-secrets-aware way.
        e.rendered_cmd = Some(if args.show_secrets {
            cmd.clone()
        } else {
            crate::redact::redact_for_audit(&cmd).into_owned()
        });
        e.secrets_masked_kinds = collect_kinds(&redactor);
        // F13: stamp retry / reauth correlation fields and a
        // `failure_class` of `ok` / `command_failed` so audit
        // consumers can filter by outcome alongside transport-error
        // entries.
        if dispatch_retried {
            e.retry_of = Some(format!("transport_stale@{label}"));
        }
        if let Some(rid) = &dispatch_reauth_id {
            e.reauth_id = Some(rid.clone());
        }
        e.failure_class = Some(if out.ok() { "ok" } else { "command_failed" }.to_string());
        if args.revert_preview {
            eprintln!(
                "[inspect] revert preview {label}: unsupported -- {}",
                e.revert.as_ref().map(|r| r.preview.as_str()).unwrap_or(""),
            );
        }
        store.append(&e)?;

        if let Some(prev) = last_inner {
            if prev != out.exit_code {
                all_same = false;
            }
        }
        last_inner = Some(out.exit_code);

        if out.ok() {
            ok += 1;
            renderer.data_line(format!("{label}: ok ({}ms)", dur));
            // We already streamed the captured stdout above; do NOT
            // re-emit it via the renderer (would duplicate).
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
                 Use `inspect put` (F15) for file transfer, or rebuild the image with a busybox/alpine layer."
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

    let trailer = if let Some(c) = uniform_transport {
        format!(
            "exec: {ok} ok, {bad} failed ({})",
            c.summary_hint(&args.selector)
        )
    } else {
        format!("exec: {ok} ok, {bad} failed")
    };
    renderer.summary(trailer).next("inspect audit ls");
    renderer.print();

    // F13: uniform transport failures route to ExitKind::Transport so
    // wrappers/scripts can branch on 12/13/14 and re-establish the
    // session before retrying.
    if let Some(c) = uniform_transport {
        if bad > 0 {
            return Ok(ExitKind::Transport(c));
        }
    }

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

/// L7 (v0.1.3): tag the audit `args` text with the redaction outcome.
/// `[secrets_exposed=true]` when the operator opted out via
/// `--show-secrets`; `[secrets_masked=true]` when the redactor fired
/// during this step; clean cmd otherwise.
///
/// G2 (post-v0.1.3 audit hardening): the `user_cmd` text itself is
/// passed through [`crate::redact::redact_for_audit`] so embedded
/// secrets in the command line never reach the audit log in plaintext.
fn stamp_args(
    user_cmd: &str,
    show_secrets: bool,
    redactor: &crate::redact::OutputRedactor,
) -> String {
    if show_secrets {
        format!("{user_cmd} [secrets_exposed=true]")
    } else {
        let masked = crate::redact::redact_for_audit(user_cmd);
        if redactor.was_active() || matches!(&masked, std::borrow::Cow::Owned(_)) {
            format!("{} [secrets_masked=true]", masked.as_ref())
        } else {
            user_cmd.to_string()
        }
    }
}

/// L7 (v0.1.3): collect the redactor's per-kind activity for
/// `AuditEntry::secrets_masked_kinds`. Returns `None` (so
/// `skip_serializing_if` elides the field) when no masker fired.
fn collect_kinds(redactor: &crate::redact::OutputRedactor) -> Option<Vec<String>> {
    let kinds = redactor.active_kinds();
    if kinds.is_empty() {
        None
    } else {
        Some(kinds.into_iter().map(|s| s.to_string()).collect())
    }
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
