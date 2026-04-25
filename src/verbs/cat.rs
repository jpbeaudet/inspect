//! `inspect cat <sel>:<path>` — read a file.

use anyhow::Result;

use crate::cli::CatArgs;
use crate::error::ExitKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::{Envelope, JsonOut, Renderer};
use crate::verbs::quote::shquote;

pub fn run(args: CatArgs) -> Result<ExitKind> {
    let (runner, nses, targets) = plan(&args.target)?;

    let mut printed_any = false;
    let mut errored_any = false;

    for step in iter_steps(&nses, &targets) {
        let Some(path) = step.path.as_deref() else {
            eprintln!(
                "warning: '{}' has no :path; cat requires a file path (e.g. arte/atlas:/etc/atlas.conf)",
                step.ns.namespace
            );
            errored_any = true;
            continue;
        };
        let cmd = build_cat(step.service(), path);
        let out = runner.run(&step.ns.namespace, &step.ns.target, &cmd, RunOpts::with_timeout(30))?;
        if !out.ok() {
            errored_any = true;
            if args.json {
                JsonOut::write(
                    &Envelope::new(&step.ns.namespace, "file", format!("file:{path}"))
                        .with_service(step.service().unwrap_or("_"))
                        .put("path", path)
                        .put("error", out.stderr.trim())
                        .put("exit", out.exit_code),
                );
            } else {
                eprintln!(
                    "{}: cat failed (exit {}): {}",
                    step.ns.namespace,
                    out.exit_code,
                    out.stderr.trim()
                );
            }
            continue;
        }
        printed_any = true;
        if args.json {
            for line in out.stdout.lines() {
                JsonOut::write(
                    &Envelope::new(&step.ns.namespace, "file", format!("file:{path}"))
                        .with_service(step.service().unwrap_or("_"))
                        .put("path", path)
                        .put("line", line),
                );
            }
        } else {
            print!("{}", out.stdout);
            if !out.stdout.ends_with('\n') {
                println!();
            }
        }
    }

    if args.json {
        return Ok(if printed_any { ExitKind::Success } else { ExitKind::Error });
    }
    if !printed_any && errored_any {
        Renderer::new()
            .summary("cat failed on every target")
            .next("inspect ls <sel>:<dir> to find the right path")
            .print();
        return Ok(ExitKind::Error);
    }
    Ok(ExitKind::Success)
}

fn build_cat(service: Option<&str>, path: &str) -> String {
    match service {
        Some(svc) => format!("docker exec {} cat -- {}", shquote(svc), shquote(path)),
        None => format!("cat -- {}", shquote(path)),
    }
}
