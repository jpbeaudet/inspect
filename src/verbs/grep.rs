//! `inspect grep <pat> <sel>` — search content in logs or files.
//!
//! Behavior:
//! - If selector has `:path` → grep that file inside the container/host.
//! - Else → fan out across services, piping `docker logs` through grep/rg.
//!
//! Tooling fallback (bible §profile + §pushdown): prefer `rg` if discovered;
//! else `grep`. The `rg` flag set we expose is intentionally a strict subset
//! shared with `grep` so the same flags map both ways.

use anyhow::Result;

use crate::cli::GrepArgs;
use crate::error::ExitKind;
use crate::profile::schema::Profile;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan, Step};
use crate::verbs::duration::parse_duration;
use crate::verbs::output::{Envelope, JsonOut};
use crate::verbs::quote::shquote;

pub fn run(mut args: GrepArgs) -> Result<ExitKind> {
    // Activate the FormatArgs mutex check
    // (e.g. `--select` without `--json` → exit 2).
    args.format.resolve()?;
    if let Some(s) = &args.since {
        parse_duration(s)?;
    }

    let (runner, nses, targets) = plan(&args.selector)?;

    // --reset-cursor and --since-last mirror the logs verb.
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

    let case_insensitive = resolve_case(&args);
    let mut matches = 0usize;

    // Construct the streaming `--select` filter ONCE at
    // function entry so a parse error fails fast before any frame is
    // emitted.
    let mut select = args.format.select_filter()?;

    for step in iter_steps(&nses, &targets) {
        let svc_for_cursor = step.service().unwrap_or("_").to_string();
        // One redactor per step. Grep emits matched lines
        // verbatim from the remote pipeline — anything that would
        // otherwise be a bare token in the operator's terminal goes
        // through the four-masker chain first.
        let redactor = crate::redact::OutputRedactor::new(args.show_secrets, false);
        if args.since_last {
            let prev = crate::verbs::cursor::Cursor::load(&step.ns.namespace, &svc_for_cursor)?;
            let since = match &prev {
                Some(c) if c.last_call > 0 => c.last_call.to_string(),
                _ => crate::verbs::cursor::default_since(),
            };
            args.since = Some(since);
            let now = crate::verbs::cursor::Cursor::now(&step.ns.namespace, &svc_for_cursor);
            if let Err(e) = now.save() {
                if !args.format.is_json() {
                    crate::tee_eprintln!("warn: failed to save cursor: {e}");
                }
            }
        }
        let cmd = build_grep_cmd(&step, &args, case_insensitive, step.ns.profile.as_ref());
        let label = format!("grep {}/{}", step.ns.namespace, svc_for_cursor);
        let show_progress = !args.format.is_json();
        let out = crate::verbs::progress::with_progress(&label, show_progress, || {
            runner.run(
                &step.ns.namespace,
                &step.ns.target,
                &cmd,
                RunOpts::with_timeout(60),
            )
        })?;
        // grep exits 1 on no match; treat as non-error.
        if !out.ok() && out.exit_code != 1 {
            if !args.format.is_json() {
                crate::tee_eprintln!(
                    "{}: grep failed (exit {}): {}",
                    step.ns.namespace,
                    out.exit_code,
                    out.stderr.trim()
                );
            }
            continue;
        }
        let svc = step.service().unwrap_or("_").to_string();
        let medium = if step.path.is_some() { "file" } else { "logs" };
        let source = step
            .path
            .as_ref()
            .map(|p| format!("file:{p}"))
            .unwrap_or_else(|| "logs".to_string());

        if args.count {
            // grep -c emits "<n>" on the file or "<file>:<n>" with -r;
            // we render the integer per target.
            let n: u64 = out.stdout.trim().parse().unwrap_or(0);
            matches += n as usize;
            if args.format.is_json() {
                JsonOut::write(
                    &Envelope::new(&step.ns.namespace, medium, &source)
                        .with_service(&svc)
                        .put("count", n),
                    select.as_mut(),
                )?;
            } else {
                crate::tee_println!("{}/{}: {n}", step.ns.namespace, svc);
            }
            continue;
        }

        for line in out.stdout.lines() {
            // Redact before counting — a line that is
            // entirely consumed by the PEM masker (interior block
            // line) is not a real match for the operator either.
            let masked = match redactor.mask_line(line) {
                Some(m) => m,
                None => continue,
            };
            matches += 1;
            if args.format.is_json() {
                JsonOut::write(
                    &Envelope::new(&step.ns.namespace, medium, &source)
                        .with_service(&svc)
                        .put(
                            "line",
                            crate::format::safe::safe_machine_line(&masked).as_ref(),
                        ),
                    select.as_mut(),
                )?;
            } else {
                let safe = crate::format::safe::safe_terminal_line(
                    &masked,
                    crate::format::safe::DEFAULT_MAX_LINE_BYTES,
                );
                crate::tee_println!("{}/{} | {safe}", step.ns.namespace, svc);
            }
        }
    }

    if args.format.is_json() {
        crate::verbs::output::flush_filter(select.as_mut())?;
    }

    Ok(if matches > 0 {
        ExitKind::Success
    } else {
        ExitKind::NoMatches
    })
}

/// Smart-case (rg-style): if neither -i nor -s is set and the pattern is
/// all-lowercase, default to case-insensitive.
fn resolve_case(args: &GrepArgs) -> bool {
    if args.case_sensitive {
        return false;
    }
    if args.ignore_case {
        return true;
    }
    args.pattern.chars().all(|c| !c.is_ascii_uppercase())
}

fn build_grep_cmd(step: &Step<'_>, args: &GrepArgs, ci: bool, profile: Option<&Profile>) -> String {
    let mut tool = pick_tool(profile);
    let pat = shquote(&args.pattern);

    let mut flags = String::new();
    if ci {
        flags.push_str(" -i");
    }
    if args.word {
        flags.push_str(" -w");
    }
    if args.fixed {
        flags.push_str(" -F");
    }
    if args.extended && tool == Tool::Grep {
        flags.push_str(" -E");
    }
    if args.invert {
        flags.push_str(" -v");
    }
    if args.count {
        flags.push_str(" -c");
    }
    if let Some(n) = args.max_count {
        flags.push_str(&format!(" -m {n}"));
    }
    if let Some(n) = args.context {
        flags.push_str(&format!(" -C {n}"));
    } else {
        if let Some(n) = args.before {
            flags.push_str(&format!(" -B {n}"));
        }
        if let Some(n) = args.after {
            flags.push_str(&format!(" -A {n}"));
        }
    }
    let tool_bin = tool.bin();
    let suf = crate::verbs::line_filter::build_suffix(&args.match_re, &args.exclude_re, false);

    if let Some(path) = step.path.as_deref() {
        let inner = format!("{tool_bin}{flags} -- {pat} {}{suf}", shquote(path));
        // Docker exec must receive the
        // container_name, not the canonical service name. See
        // `Step::container()` doc; same fix shipped for cat/ls/find.
        return match step.container() {
            Some(svc) => format!("docker exec {} sh -c {}", shquote(svc), shquote(&inner)),
            None => inner,
        };
    }
    // Logs path: docker logs <svc> 2>&1 | grep ...
    if let Some(svc) = step.service() {
        // For systemd units, swap docker logs for journalctl.
        let logs = if matches!(
            step.service_def().map(|s| s.kind),
            Some(crate::profile::schema::ServiceKind::Systemd)
        ) {
            let mut s = format!("journalctl --no-pager -u {}", shquote(svc));
            if let Some(since) = &args.since {
                s.push_str(" --since ");
                s.push_str(&shquote(&format!("-{since}")));
            }
            if let Some(tail) = args.tail {
                s.push_str(&format!(" -n {tail}"));
            }
            s
        } else {
            // `Docker logs` takes the
            // container_name; only the systemd journalctl branch
            // above keeps the canonical name (which IS the unit name).
            let docker_name = step.container().unwrap_or(svc);
            let mut s = String::from("docker logs");
            if let Some(since) = &args.since {
                s.push_str(" --since ");
                s.push_str(&shquote(since));
            }
            if let Some(tail) = args.tail {
                s.push_str(&format!(" --tail {tail}"));
            }
            s.push(' ');
            s.push_str(&shquote(docker_name));
            s.push_str(" 2>&1");
            s
        };
        return format!("{logs} | {tool_bin}{flags} -- {pat}{suf} || true");
    }
    // Host-level fallback.
    let _ = &mut tool;
    format!("{tool_bin}{flags} -- {pat} /var/log/syslog{suf} 2>/dev/null || {tool_bin}{flags} -- {pat} /var/log/messages{suf}")
}

#[derive(PartialEq, Eq)]
enum Tool {
    Rg,
    Grep,
}
impl Tool {
    fn bin(&self) -> &'static str {
        match self {
            Tool::Rg => "rg --no-heading",
            Tool::Grep => "grep -H",
        }
    }
}

fn pick_tool(profile: Option<&Profile>) -> Tool {
    if profile.map(|p| p.remote_tooling.rg).unwrap_or(false) {
        Tool::Rg
    } else {
        Tool::Grep
    }
}
