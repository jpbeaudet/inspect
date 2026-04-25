//! `inspect find <sel>:<path> [pat]` — find files matching a pattern.

use anyhow::Result;

use crate::cli::FindArgs;
use crate::error::ExitKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::{Envelope, JsonOut};
use crate::verbs::quote::shquote;

pub fn run(args: FindArgs) -> Result<ExitKind> {
    let (runner, nses, targets) = plan(&args.target)?;

    let mut total_hits = 0usize;
    for step in iter_steps(&nses, &targets) {
        let path = step.path.as_deref().unwrap_or(".");
        let mut find_cmd = format!("find {} -type f", shquote(path));
        if let Some(pat) = args.pattern.as_deref() {
            find_cmd.push(' ');
            find_cmd.push_str("-name ");
            find_cmd.push_str(&shquote(pat));
        }
        let cmd = match step.service() {
            Some(svc) => format!(
                "docker exec {} sh -c {}",
                shquote(svc),
                shquote(&find_cmd)
            ),
            None => find_cmd,
        };
        let out = runner.run(&step.ns.namespace, &step.ns.target, &cmd, RunOpts::with_timeout(60))?;
        if !out.ok() && out.stdout.is_empty() {
            // find can return non-zero on permission denied while still
            // listing matches; only treat true failures (no stdout) as errors.
            if !args.json {
                eprintln!(
                    "{}: find failed (exit {}): {}",
                    step.ns.namespace,
                    out.exit_code,
                    out.stderr.trim()
                );
            }
            continue;
        }
        for line in out.stdout.lines().filter(|l| !l.is_empty()) {
            total_hits += 1;
            if args.json {
                JsonOut::write(
                    &Envelope::new(&step.ns.namespace, "dir", format!("dir:{path}"))
                        .with_service(step.service().unwrap_or("_"))
                        .put("path", line),
                );
            } else {
                println!(
                    "{}{}: {line}",
                    step.ns.namespace,
                    step.service().map(|s| format!("/{s}")).unwrap_or_default()
                );
            }
        }
    }
    if total_hits == 0 {
        if !args.json {
            eprintln!("(no matches)");
        }
        return Ok(ExitKind::NoMatches);
    }
    Ok(ExitKind::Success)
}
