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
        let cmd = build_logs(step.service_def(), step.service(), &args);
        let opts = if args.follow {
            // Use a long timeout for follow; users will Ctrl-C to stop.
            RunOpts::with_timeout(args.follow_timeout_secs.unwrap_or(60 * 60 * 8))
        } else {
            RunOpts::with_timeout(60)
        };
        let out = runner.run(&step.ns.namespace, &step.ns.target, &cmd, opts)?;
        if !out.ok() && out.stdout.is_empty() {
            if !args.json {
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
            if args.json {
                JsonOut::write(
                    &Envelope::new(&step.ns.namespace, "logs", "logs")
                        .with_service(&svc_name)
                        .put("line", line),
                );
            } else {
                println!("{}/{} | {line}", step.ns.namespace, svc_name);
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
    let mut s = String::from("docker logs");
    if args.follow {
        s.push_str(" -f");
    }
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
