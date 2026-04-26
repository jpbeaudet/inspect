//! `inspect ls <sel>:<path>` — list a directory.

use anyhow::Result;

use crate::cli::LsArgs;
use crate::error::ExitKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::{Envelope, JsonOut};
use crate::verbs::quote::shquote;

pub fn run(args: LsArgs) -> Result<ExitKind> {
    let (runner, nses, targets) = plan(&args.target)?;

    let mut any_ok = false;
    for step in iter_steps(&nses, &targets) {
        let path = step.path.as_deref().unwrap_or("/");
        let mut ls_args = String::from("ls -1");
        if args.long {
            ls_args.push_str(" -l");
        }
        if args.all {
            ls_args.push_str(" -A");
        }
        let cmd = match step.service() {
            Some(svc) => format!(
                "docker exec {} sh -c {}",
                shquote(svc),
                shquote(&format!("{ls_args} -- {}", shquote(path)))
            ),
            None => format!("{ls_args} -- {}", shquote(path)),
        };
        let out = runner.run(&step.ns.namespace, &step.ns.target, &cmd, RunOpts::with_timeout(30))?;
        if !out.ok() {
            if !args.format.is_json() {
                eprintln!(
                    "{}: ls failed (exit {}): {}",
                    step.ns.namespace,
                    out.exit_code,
                    out.stderr.trim()
                );
            } else {
                JsonOut::write(
                    &Envelope::new(&step.ns.namespace, "dir", format!("dir:{path}"))
                        .with_service(step.service().unwrap_or("_"))
                        .put("path", path)
                        .put("error", out.stderr.trim())
                        .put("exit", out.exit_code),
                );
            }
            continue;
        }
        any_ok = true;
        if args.format.is_json() {
            for line in out.stdout.lines() {
                JsonOut::write(
                    &Envelope::new(&step.ns.namespace, "dir", format!("dir:{path}"))
                        .with_service(step.service().unwrap_or("_"))
                        .put("path", path)
                        .put("entry", line),
                );
            }
        } else {
            let svc_part = step.service().map(|s| format!("/{s}")).unwrap_or_default();
            println!("# {}{svc_part}:{path}", step.ns.namespace);
            print!("{}", out.stdout);
            if !out.stdout.ends_with('\n') {
                println!();
            }
        }
    }
    Ok(if any_ok { ExitKind::Success } else { ExitKind::Error })
}
