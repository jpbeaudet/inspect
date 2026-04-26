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

pub fn run(args: LogsArgs) -> Result<ExitKind> {
    if let Some(s) = &args.since {
        parse_duration(s)?;
    }
    if let Some(s) = &args.until {
        parse_duration(s)?;
    }

    let (runner, nses, targets) = plan(&args.selector)?;
    let mut any_lines = false;

    for step in iter_steps(&nses, &targets) {
        let svc_name = step.service().unwrap_or("_").to_string();

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
                        eprintln!("{msg}");
                    }
                    continue;
                }
            }
        }

        let cmd = build_logs(step.service_def(), step.service(), &args);
        let opts = if args.follow {
            // Use a long timeout for follow; users will Ctrl-C to stop.
            RunOpts::with_timeout(args.follow_timeout_secs.unwrap_or(60 * 60 * 8))
        } else {
            RunOpts::with_timeout(60)
        };
        let out = runner.run(&step.ns.namespace, &step.ns.target, &cmd, opts)?;
        if !out.ok() && out.stdout.is_empty() {
            if !args.format.is_json() {
                eprintln!(
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
            any_lines = true;
            if args.format.is_json() {
                JsonOut::write(
                    &Envelope::new(&step.ns.namespace, "logs", "logs")
                        .with_service(&svc_name)
                        .put("line", crate::format::safe::safe_machine_line(line).as_ref()),
                );
            } else {
                let safe = crate::format::safe::safe_terminal_line(
                    line,
                    crate::format::safe::DEFAULT_MAX_LINE_BYTES,
                );
                println!("{}/{} | {safe}", step.ns.namespace, svc_name);
            }
        }
    }

    Ok(if any_lines {
        ExitKind::Success
    } else {
        ExitKind::NoMatches
    })
}

fn build_logs(
    svc_def: Option<&crate::profile::schema::Service>,
    svc_name: Option<&str>,
    args: &LogsArgs,
) -> String {
    use crate::profile::schema::ServiceKind;

    let kind = svc_def.map(|s| s.kind).unwrap_or(ServiceKind::Container);
    match (svc_name, kind) {
        (Some(name), ServiceKind::Systemd) => build_journalctl(name, args),
        (Some(name), _) => build_docker_logs(name, args),
        (None, _) => {
            // Host-level: tail /var/log/syslog by default.
            let tail = args.tail.unwrap_or(200);
            format!("tail -n {tail} /var/log/syslog 2>/dev/null || tail -n {tail} /var/log/messages")
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
    let mut s = String::from("docker logs");
    if args.follow {
        s.push_str(" -f");
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
    s
}

fn build_journalctl(unit: &str, args: &LogsArgs) -> String {
    let mut s = String::from("journalctl --no-pager -u ");
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
    s
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
            until: None,
            tail: None,
            follow,
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
}
