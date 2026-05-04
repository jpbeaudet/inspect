//! `inspect logs <sel>` — tail or view container/host-service logs.
//!
//! For container services we run `docker logs [-f] [--since X] [--tail N]`.
//! For systemd services we run `journalctl -u <name> [...]`.
//! `--follow` switches the runner to streaming mode (line-by-line passthrough).

use anyhow::Result;

use crate::cli::LogsArgs;
use crate::error::ExitKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::duration::parse_duration;
use crate::verbs::output::{Envelope, JsonOut};
use crate::verbs::quote::shquote;

pub fn run(mut args: LogsArgs) -> Result<ExitKind> {
    if let Some(s) = &args.since {
        parse_duration(s)?;
    }
    if let Some(s) = &args.until {
        parse_duration(s)?;
    }

    let (runner, nses, targets) = plan(&args.selector)?;

    // P10 (v0.1.1): handle --reset-cursor up front -- it does not stream
    // any logs, just drops the cursor file(s) for every selected target.
    if args.reset_cursor {
        let mut deleted = 0usize;
        for step in iter_steps(&nses, &targets) {
            let svc = step.service().unwrap_or("_");
            if crate::verbs::cursor::reset(&step.ns.namespace, svc)? {
                deleted += 1;
            }
        }
        if !args.format.is_json() {
            crate::tee_eprintln!("reset {deleted} cursor(s)");
        }
        return Ok(ExitKind::Success);
    }

    let mut any_lines = false;

    // P5 (v0.1.1): merged multi-container view. We honor --since-last
    // and the log-driver gate per source, then dispatch to the merger
    // module which fans out execution and re-orders by RFC3339
    // timestamp (batch) or arrival order (follow).
    let merged_steps: Vec<_> = iter_steps(&nses, &targets).collect();
    if args.merged && merged_steps.len() > 1 {
        // Per-step preflight: cursor handling + log-driver rejection.
        let mut sources: Vec<crate::verbs::merged::MergeSource> = Vec::new();
        // Stash a per-step `args` clone so each source can have its own
        // expanded `--since` from --since-last without poisoning peers.
        let mut per_step_args: Vec<LogsArgs> = Vec::new();
        for step in &merged_steps {
            let svc_name = step.service().unwrap_or("_").to_string();
            let mut step_args = args.clone();
            if step_args.since_last {
                let prev = crate::verbs::cursor::Cursor::load(&step.ns.namespace, &svc_name)?;
                let since = match &prev {
                    Some(c) if c.last_call > 0 => c.last_call.to_string(),
                    _ => crate::verbs::cursor::default_since(),
                };
                step_args.since = Some(since);
                let now = crate::verbs::cursor::Cursor::now(&step.ns.namespace, &svc_name);
                if let Err(e) = now.save() {
                    if !args.format.is_json() {
                        crate::tee_eprintln!("warn: failed to save cursor: {e}");
                    }
                }
            }
            // Driver gate.
            if let Some(svc_def) = step.service_def() {
                if let Some(driver) = svc_def.log_driver {
                    if !driver.is_readable_via_docker_logs() {
                        if !args.format.is_json() {
                            crate::tee_eprintln!(
                                "{}/{}: log driver `{}` is not readable via `docker logs` -- skipped in merged view",
                                step.ns.namespace,
                                svc_name,
                                driver.as_str(),
                            );
                        }
                        continue;
                    }
                }
            }
            per_step_args.push(step_args);
        }
        // Build the source list with command strings derived from each
        // step's args clone.
        let live_steps: Vec<_> = merged_steps
            .iter()
            .filter(|step| {
                step.service_def()
                    .and_then(|s| s.log_driver)
                    .map(|d| d.is_readable_via_docker_logs())
                    .unwrap_or(true)
            })
            .collect();
        for (step, step_args) in live_steps.iter().zip(per_step_args.iter()) {
            let cmd = build_logs(
                step.service_def(),
                step.service(),
                step.container(),
                step_args,
            );
            sources.push(crate::verbs::merged::MergeSource {
                namespace: step.ns.namespace.as_str(),
                target: &step.ns.target,
                svc: step.service().unwrap_or("_").to_string(),
                cmd,
            });
        }
        let json = args.format.is_json();
        let timeout = if args.follow {
            args.follow_timeout_secs.unwrap_or(60 * 60 * 8)
        } else {
            60
        };
        let total = if args.follow {
            crate::verbs::merged::follow_merged(
                runner.as_ref(),
                &sources,
                timeout,
                args.show_secrets,
                |m| {
                    if json {
                        crate::verbs::merged::print_json(
                            // The MergeSource borrows namespace as &str; we
                            // re-derive it from svc_idx into the source list.
                            sources.get(m.svc_idx).map(|s| s.namespace).unwrap_or("_"),
                            &m.svc,
                            &m.line,
                        );
                    } else {
                        crate::verbs::merged::print_human(&m.svc, &m.line);
                    }
                },
            )?
        } else {
            crate::verbs::merged::batch_merged(
                runner.as_ref(),
                &sources,
                timeout,
                args.show_secrets,
                |m| {
                    if json {
                        crate::verbs::merged::print_json(
                            sources.get(m.svc_idx).map(|s| s.namespace).unwrap_or("_"),
                            &m.svc,
                            &m.line,
                        );
                    } else {
                        crate::verbs::merged::print_human(&m.svc, &m.line);
                    }
                },
            )?
        };
        return Ok(if total > 0 {
            ExitKind::Success
        } else if !args.match_re.is_empty() && !args.follow {
            // B3 (v0.1.2): same exit-0-with-notice contract as the
            // non-merged path. We don't bother distinguishing per
            // source here — the merged view is one logical stream.
            if !args.format.is_json() {
                crate::tee_eprintln!("{}", no_match_notice(&args));
            }
            ExitKind::Success
        } else {
            ExitKind::NoMatches
        });
    }

    for step in iter_steps(&nses, &targets) {
        let svc_name = step.service().unwrap_or("_").to_string();
        // L7 (v0.1.3): per-step redactor. Used for the non-follow
        // batch path below; `stream_follow` constructs its own per
        // reconnect attempt so a transport drop mid-PEM-block does
        // not poison the post-reconnect state.
        let redactor = crate::redact::OutputRedactor::new(args.show_secrets, false);

        // P10: --since-last expands into --since <unix-ts> from the
        // saved cursor (or INSPECT_SINCE_LAST_DEFAULT on cold start).
        // We always rewrite the cursor at the start of the run so a
        // crash mid-stream still leaves the next call resumable; the
        // small overlap is acceptable (logs are append-only).
        if args.since_last {
            let prev = crate::verbs::cursor::Cursor::load(&step.ns.namespace, &svc_name)?;
            let since = match &prev {
                Some(c) if c.last_call > 0 => c.last_call.to_string(),
                _ => crate::verbs::cursor::default_since(),
            };
            args.since = Some(since);
            // Persist the new cursor up-front.
            let now = crate::verbs::cursor::Cursor::now(&step.ns.namespace, &svc_name);
            // Best effort: a cursor write failure should not abort
            // the user's actual log query.
            if let Err(e) = now.save() {
                if !args.format.is_json() {
                    crate::tee_eprintln!("warn: failed to save cursor: {e}");
                }
            }
        }

        // Field pitfall §2.3: refuse early when the service is
        // configured with a log driver that ships logs out of the
        // docker daemon's reach. `docker logs` would otherwise return
        // empty stdout with exit 0, and the operator would think the
        // service is silent rather than misconfigured for log capture.
        if let Some(svc_def) = step.service_def() {
            if let Some(driver) = svc_def.log_driver {
                if !driver.is_readable_via_docker_logs() {
                    let msg = format!(
                        "{}/{}: log driver `{}` is not readable via `docker logs` -- \
                         logs are shipped to that driver's sink (query it directly: \
                         e.g. `journalctl`, CloudWatch, your fluentd/splunk/gelf collector)",
                        step.ns.namespace,
                        svc_name,
                        driver.as_str(),
                    );
                    if args.format.is_json() {
                        JsonOut::write(
                            &Envelope::new(&step.ns.namespace, "logs", "logs")
                                .with_service(&svc_name)
                                .put("error", msg.as_str())
                                .put("log_driver", driver.as_str()),
                        );
                    } else {
                        crate::tee_eprintln!("{msg}");
                    }
                    continue;
                }
            }
        }

        let cmd = build_logs(step.service_def(), step.service(), step.container(), &args);

        // P1 (v0.1.1): in --follow mode, render each line as it
        // arrives instead of buffering until the SSH process exits.
        // We also implement client-side reconnect (3 tries with 1/2/4s
        // backoff) so a transient SSH drop doesn't end the user's
        // session: the server-side loop in `build_docker_logs` already
        // re-attaches across docker log rotations, so this layer only
        // covers the SSH transport.
        if args.follow {
            stream_follow(
                runner.as_ref(),
                &step.ns.namespace,
                &step.ns.target,
                &cmd,
                &svc_name,
                args.follow_timeout_secs.unwrap_or(60 * 60 * 8),
                args.format.is_json(),
                args.show_secrets,
                &mut any_lines,
            );
            continue;
        }

        let opts = RunOpts::with_timeout(60);
        let label = format!("logs {}/{}", step.ns.namespace, svc_name);
        let show_progress = !args.format.is_json();
        let out = crate::verbs::progress::with_progress(&label, show_progress, || {
            runner.run(&step.ns.namespace, &step.ns.target, &cmd, opts)
        })?;
        if !out.ok() && out.stdout.is_empty() {
            // B3 (v0.1.2): when `--match` is in play, the remote
            // pipeline ends in `grep -E '<pat>'`, which exits 1 when
            // it finds zero lines. That is the predicate doing its
            // job, not a real failure. Suppress the spurious "logs
            // failed" line in that case; the post-loop summary will
            // emit a single, clear `(no matches …)` notice for the
            // whole run instead.
            let is_grep_no_match =
                !args.match_re.is_empty() && out.exit_code == 1 && out.stderr.trim().is_empty();
            if is_grep_no_match {
                continue;
            }
            if !args.format.is_json() {
                crate::tee_eprintln!(
                    "{}/{}: logs failed (exit {}): {}",
                    step.ns.namespace,
                    svc_name,
                    out.exit_code,
                    out.stderr.trim()
                );
            }
            continue;
        }
        for line in out.stdout.lines() {
            // L7 (v0.1.3): redactor is stateful for PEM blocks; lines
            // inside a block return None and are swallowed so the
            // BEGIN-line marker is the only output for the whole
            // block.
            let masked = match redactor.mask_line(line) {
                Some(m) => m,
                None => continue,
            };
            any_lines = true;
            if args.format.is_json() {
                JsonOut::write(
                    &Envelope::new(&step.ns.namespace, "logs", "logs")
                        .with_service(&svc_name)
                        .put(
                            "line",
                            crate::format::safe::safe_machine_line(&masked).as_ref(),
                        ),
                );
            } else {
                let safe = crate::format::safe::safe_terminal_line(
                    &masked,
                    crate::format::safe::DEFAULT_MAX_LINE_BYTES,
                );
                crate::tee_println!("{}/{} | {safe}", step.ns.namespace, svc_name);
            }
        }
    }

    Ok(if any_lines {
        ExitKind::Success
    } else if !args.match_re.is_empty() && !args.follow {
        // B3 (v0.1.2): treat `inspect logs --match <pat>` with zero
        // hits as a successful narrowing of the log view, not an
        // error. Mirrors how operators read this flag ("filter the
        // stream, tell me if there's anything") rather than how grep
        // models it ("selector that fails when nothing matches").
        // `inspect grep` keeps the grep convention. `--follow` paths
        // never reach this branch — the stream stays open until the
        // user cancels or the upstream times out.
        if !args.format.is_json() {
            crate::tee_eprintln!("{}", no_match_notice(&args));
        }
        ExitKind::Success
    } else {
        ExitKind::NoMatches
    })
}

/// B3 (v0.1.2): build the human-readable `"(no matches for X in <window>)"` line printed when `inspect logs --match` produces zero
/// hits. Pulled out so the same message is reachable from both the
/// per-step and merged code paths.
fn no_match_notice(args: &LogsArgs) -> String {
    let pats = args
        .match_re
        .iter()
        .map(|p| format!("'{p}'"))
        .collect::<Vec<_>>()
        .join(" or ");
    let window = if let Some(s) = &args.since {
        format!("{s} window")
    } else if args.since_last {
        "--since-last window".to_string()
    } else if let Some(n) = args.tail {
        format!("last {n} lines")
    } else {
        "current window".to_string()
    };
    format!("(no matches for {pats} in {window})")
}

fn build_logs(
    svc_def: Option<&crate::profile::schema::Service>,
    svc_name: Option<&str>,
    container: Option<&str>,
    args: &LogsArgs,
) -> String {
    use crate::profile::schema::ServiceKind;

    let kind = svc_def.map(|s| s.kind).unwrap_or(ServiceKind::Container);
    match (svc_name, kind) {
        // Systemd: journalctl wants the unit name (== `name`), not the
        // container_name token.
        (Some(name), ServiceKind::Systemd) => build_journalctl(name, args),
        // Container: every docker subcommand must target the real
        // container name; `container` falls back to `svc_name` when
        // no profile is loaded.
        (Some(_), _) => build_docker_logs(container.unwrap_or("_"), args),
        (None, _) => {
            // Host-level: tail /var/log/syslog by default.
            let tail = args.tail.unwrap_or(200);
            let base = format!(
                "tail -n {tail} /var/log/syslog 2>/dev/null || tail -n {tail} /var/log/messages"
            );
            // Match/exclude wrap: tail's `||` already protects the
            // empty case, so we splice the filter through an sh -c.
            let suf = crate::verbs::line_filter::build_suffix(
                &args.match_re,
                &args.exclude_re,
                args.follow,
            );
            if suf.is_empty() {
                base
            } else {
                format!("sh -c {}", shquote(&format!("{base}{suf}")))
            }
        }
    }
}

fn build_docker_logs(svc: &str, args: &LogsArgs) -> String {
    // Field pitfall §2.2: when `docker logs -f` returns early
    // (typically because the underlying log file was truncated or
    // rotated), the local stream silently goes quiet. The container
    // is still running and still producing logs — we just lost the
    // file handle on the daemon side.
    //
    // Fix: wrap follow mode in a server-side shell loop that
    // re-attaches to `docker logs -f` as long as the container is
    // still alive. The first iteration honours `--since/--until/
    // --tail`; subsequent iterations use `--tail 0` so we don't
    // replay history every time the file rotates.
    if args.follow {
        let head = build_docker_logs_once(svc, args, /*reconnect=*/ false);
        let tail = build_docker_logs_once(svc, args, /*reconnect=*/ true);
        let svc_q = shquote(svc);
        // `set -u` guards against typos; the explicit
        // `docker inspect` check distinguishes "container shut down"
        // (exit cleanly) from "file rotated" (retry).
        let inner = format!(
            "set -u; first=1; while :; do \
                if [ \"$first\" = 1 ]; then {head}; else {tail}; fi; \
                first=0; \
                docker inspect -f x {svc_q} >/dev/null 2>&1 || exit 0; \
                sleep 1; \
             done"
        );
        return format!("sh -c {}", shquote(&inner));
    }
    build_docker_logs_once(svc, args, /*reconnect=*/ false)
}

/// One invocation of `docker logs`. When `reconnect == true` we omit
/// `--since`/`--until`/`--tail` and force `--tail 0` so we pick up
/// only new lines after a follow-mode reconnect.
fn build_docker_logs_once(svc: &str, args: &LogsArgs, reconnect: bool) -> String {
    // Field pitfall §5.1: `docker logs -f` chunks output through the
    // daemon's pipe, which is block-buffered when stdout is not a tty.
    // Operators see laggy, bursty lines instead of the live stream
    // they expect. `stdbuf -oL -eL` overrides the libc buffer to
    // line-buffered for this child only -- safe because docker logs
    // is itself line-oriented. Apply only in follow mode (non-follow
    // already drains the buffer at exit).
    let mut s = if args.follow {
        String::from("stdbuf -oL -eL docker logs")
    } else {
        String::from("docker logs")
    };
    if args.follow {
        s.push_str(" -f");
    }
    if args.merged {
        // P5: required so the merger has a parseable RFC3339 prefix
        // on every line. The merger strips it before printing.
        s.push_str(" --timestamps");
    }
    if reconnect {
        // After a rotation we don't want the full history again.
        s.push_str(" --tail 0");
    } else {
        if let Some(since) = &args.since {
            s.push_str(" --since ");
            s.push_str(&shquote(since));
        }
        if let Some(until) = &args.until {
            s.push_str(" --until ");
            s.push_str(&shquote(until));
        }
        if let Some(tail) = args.tail {
            s.push_str(&format!(" --tail {tail}"));
        }
    }
    s.push(' ');
    s.push_str(&shquote(svc));
    // docker logs writes to both stderr+stdout; merge for line discipline.
    s.push_str(" 2>&1");
    s.push_str(&crate::verbs::line_filter::build_suffix(
        &args.match_re,
        &args.exclude_re,
        args.follow,
    ));
    s
}

fn build_journalctl(unit: &str, args: &LogsArgs) -> String {
    // Field pitfall §5.1: line-buffer journalctl in follow mode so
    // operators get a live stream instead of block-buffered chunks.
    let mut s = if args.follow {
        String::from("stdbuf -oL -eL journalctl --no-pager -u ")
    } else {
        String::from("journalctl --no-pager -u ")
    };
    s.push_str(&shquote(unit));
    if args.follow {
        s.push_str(" -f");
    }
    if let Some(since) = &args.since {
        s.push_str(" --since ");
        s.push_str(&shquote(&format!("-{since}")));
    }
    if let Some(tail) = args.tail {
        s.push_str(&format!(" -n {tail}"));
    }
    s.push_str(&crate::verbs::line_filter::build_suffix(
        &args.match_re,
        &args.exclude_re,
        args.follow,
    ));
    s
}

/// P1 (v0.1.1): client-side reconnect wrapper for `--follow`. Streams
/// each line from the SSH child to stdout (or JSON), and on transient
/// SSH failure retries up to 3 times with 1s/2s/4s backoff. Aborts
/// cleanly on Ctrl-C (cancellation flag set by [`crate::exec::cancel`]).
///
/// L7 (v0.1.3): a fresh redactor is constructed inside each retry
/// iteration. A transport drop mid-PEM-block invalidates the prior
/// in-block state because the post-reconnect stream is a new server
/// process; carrying the flag across would over-redact (suppressing
/// good lines until an END marker that may never come). The downside
/// is a vanishingly small window where a key whose BEGIN was on the
/// pre-drop stream and whose END is on the post-drop one would have
/// its post-drop body bytes leak through; the per-line maskers
/// (header/URL/env) still apply, and the realistic failure mode is
/// that `docker logs -f` after reconnect resumes from "now", not
/// mid-block.
#[allow(clippy::too_many_arguments)]
fn stream_follow(
    runner: &dyn crate::verbs::runtime::RemoteRunner,
    namespace: &str,
    target: &crate::ssh::options::SshTarget,
    cmd: &str,
    svc_name: &str,
    timeout_secs: u64,
    json: bool,
    show_secrets: bool,
    any_lines: &mut bool,
) {
    const MAX_RECONNECTS: u32 = 3;
    let mut attempt: u32 = 0;
    loop {
        if crate::exec::cancel::is_cancelled() {
            return;
        }
        let opts = RunOpts::with_timeout(timeout_secs);
        // Borrow-checker: `any_lines` is mutated inside the closure.
        // Use a local flag and merge after the call.
        let mut got_any = false;
        let redactor = crate::redact::OutputRedactor::new(show_secrets, false);
        let result = runner.run_streaming(namespace, target, cmd, opts, &mut |line| {
            let masked = match redactor.mask_line(line) {
                Some(m) => m,
                None => return,
            };
            got_any = true;
            if json {
                JsonOut::write(
                    &Envelope::new(namespace, "logs", "logs")
                        .with_service(svc_name)
                        .put(
                            "line",
                            crate::format::safe::safe_machine_line(&masked).as_ref(),
                        ),
                );
            } else {
                let safe = crate::format::safe::safe_terminal_line(
                    &masked,
                    crate::format::safe::DEFAULT_MAX_LINE_BYTES,
                );
                crate::tee_println!("{namespace}/{svc_name} | {safe}");
            }
        });
        if got_any {
            *any_lines = true;
        }

        // User pressed Ctrl-C: stop without reconnecting.
        if crate::exec::cancel::is_cancelled() {
            return;
        }

        match result {
            Ok(0) => return,
            Ok(_) | Err(_) if attempt >= MAX_RECONNECTS => {
                if let Err(e) = result {
                    if !json {
                        crate::tee_eprintln!("{namespace}/{svc_name}: stream ended ({e})");
                    }
                }
                return;
            }
            _ => {}
        }

        attempt += 1;
        let backoff = std::time::Duration::from_secs(1u64 << (attempt - 1));
        if !json {
            crate::tee_eprintln!(
                "{namespace}/{svc_name}: stream dropped, reconnecting in {}s (attempt {attempt}/{MAX_RECONNECTS})",
                backoff.as_secs()
            );
        }
        // Sleep in small slices so Ctrl-C still cancels promptly.
        let deadline = std::time::Instant::now() + backoff;
        while std::time::Instant::now() < deadline {
            if crate::exec::cancel::is_cancelled() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::LogsArgs;
    use crate::format::FormatArgs;

    fn args(follow: bool) -> LogsArgs {
        LogsArgs {
            selector: "x".into(),
            since: None,
            since_last: false,
            reset_cursor: false,
            until: None,
            tail: None,
            follow,
            merged: false,
            match_re: Vec::new(),
            exclude_re: Vec::new(),
            show_secrets: false,
            format: FormatArgs::default(),
            follow_timeout_secs: None,
        }
    }

    #[test]
    fn non_follow_is_a_single_docker_logs() {
        let s = build_docker_logs("svc", &args(false));
        assert!(s.starts_with("docker logs"));
        assert!(!s.contains("while :"));
    }

    #[test]
    fn follow_wraps_in_resilient_loop() {
        // Field pitfall §2.2: follow mode must reconnect after a
        // file rotation/truncate.
        let s = build_docker_logs("svc", &args(true));
        assert!(s.starts_with("sh -c "), "got: {s}");
        assert!(s.contains("while :"), "missing reconnect loop: {s}");
        assert!(s.contains("docker inspect"), "missing liveness check: {s}");
        // Reconnect path must use --tail 0 to avoid replaying history.
        assert!(s.contains("--tail 0"), "missing post-reconnect tail-0: {s}");
        // First iteration honours the original docker logs args.
        assert!(s.contains("docker logs -f"));
    }

    #[test]
    fn follow_loop_quotes_service_name_with_special_chars() {
        // Defence against shell injection via service name (already
        // discovered + warned in §7.3 of the original audit, but
        // reverify here since this builder constructs an extra layer
        // of `sh -c`).
        let s = build_docker_logs("svc;rm -rf /", &args(true));
        assert!(!s.contains("rm -rf /;"), "unquoted injection: {s}");
        // Still must mention the service name literally inside quotes.
        assert!(s.contains("svc;rm -rf /") || s.contains("'svc;rm -rf /'"));
    }

    // --- B3 (v0.1.2): friendly "(no matches ...)" notice ---

    #[test]
    fn no_match_notice_single_pattern_with_since() {
        let mut a = args(false);
        a.match_re = vec!["xyzzy".into()];
        a.since = Some("5m".into());
        assert_eq!(no_match_notice(&a), "(no matches for 'xyzzy' in 5m window)");
    }

    #[test]
    fn no_match_notice_multiple_patterns_or_separated() {
        let mut a = args(false);
        a.match_re = vec!["foo".into(), "bar".into()];
        a.since = Some("1h".into());
        assert_eq!(
            no_match_notice(&a),
            "(no matches for 'foo' or 'bar' in 1h window)"
        );
    }

    #[test]
    fn no_match_notice_falls_back_to_tail_when_no_since() {
        let mut a = args(false);
        a.match_re = vec!["pat".into()];
        a.tail = Some(50);
        assert_eq!(
            no_match_notice(&a),
            "(no matches for 'pat' in last 50 lines)"
        );
    }

    #[test]
    fn no_match_notice_uses_since_last_label() {
        let mut a = args(false);
        a.match_re = vec!["pat".into()];
        a.since_last = true;
        assert_eq!(
            no_match_notice(&a),
            "(no matches for 'pat' in --since-last window)"
        );
    }

    #[test]
    fn no_match_notice_default_window_label() {
        let mut a = args(false);
        a.match_re = vec!["pat".into()];
        // No since / since_last / tail -> generic label.
        assert_eq!(
            no_match_notice(&a),
            "(no matches for 'pat' in current window)"
        );
    }
}
