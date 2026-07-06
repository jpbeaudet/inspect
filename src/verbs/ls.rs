//! `inspect ls <sel>:<path>` — list a directory.

use anyhow::Result;

use crate::cli::LsArgs;
use crate::error::ExitKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::{Envelope, JsonOut};
use crate::verbs::quote::shquote;

pub fn run(args: LsArgs) -> Result<ExitKind> {
    // Activate the FormatArgs mutex check
    // (e.g. `--select` without `--json` → exit 2).
    args.format.resolve()?;

    // `inspect ls <k8s-ns>/<pod>:<path>` via `kubectl exec`.
    if let Some(ns_name) = args.target.split('/').next() {
        if let Ok(resolved) = crate::config::resolver::resolve(ns_name) {
            if resolved.config.runtime_kind() == crate::exec::runtime::RuntimeKind::K8s {
                return ls_k8s(&args, ns_name, &resolved.config);
            }
        }
    }

    let (runner, nses, targets) = plan(&args.target)?;

    // Construct the streaming `--select` filter ONCE at
    // function entry so a parse error fails fast before any frame is
    // emitted.
    let mut select = args.format.select_filter()?;

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
        // Docker exec must receive the
        // container_name, not the canonical service name. See
        // `Step::container()` doc; same fix shipped for cat/find/grep.
        let cmd = match step.container() {
            Some(svc) => format!(
                "docker exec {} sh -c {}",
                shquote(svc),
                shquote(&format!("{ls_args} -- {}", shquote(path)))
            ),
            None => format!("{ls_args} -- {}", shquote(path)),
        };
        let out = runner.run(
            &step.ns.namespace,
            &step.ns.target,
            &cmd,
            RunOpts::with_timeout(30),
        )?;
        if !out.ok() {
            if !args.format.is_json() {
                crate::tee_eprintln!(
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
                    select.as_mut(),
                )?;
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
                        .put(
                            "entry",
                            crate::format::safe::safe_machine_line(line).as_ref(),
                        ),
                    select.as_mut(),
                )?;
            }
        } else {
            let svc_part = step.service().map(|s| format!("/{s}")).unwrap_or_default();
            crate::tee_println!("# {}{svc_part}:{path}", step.ns.namespace);
            for line in out.stdout.lines() {
                let safe = crate::format::safe::safe_terminal_line(
                    line,
                    crate::format::safe::DEFAULT_MAX_LINE_BYTES,
                );
                crate::tee_println!("{safe}");
            }
        }
    }
    if args.format.is_json() {
        crate::verbs::output::flush_filter(select.as_mut())?;
    }
    Ok(if any_ok {
        ExitKind::Success
    } else {
        ExitKind::Error
    })
}

/// `inspect ls <k8s-ns>/<pod>[:<path>]` via `kubectl exec <pod>
/// -- ls`. Distroless/no-shell failures classify to no_shell_in_container ->
/// exit 16, never a raw OCI error.
fn ls_k8s(
    args: &LsArgs,
    ns: &str,
    cfg: &crate::config::namespace::NamespaceConfig,
) -> Result<ExitKind> {
    let rest = args.target.split_once('/').map(|(_, r)| r).unwrap_or("");
    let (pod, path) = match rest.split_once(':') {
        Some((p, path)) => (p, if path.is_empty() { "/" } else { path }),
        None => (rest, "/"),
    };
    if pod.is_empty() {
        crate::tee_eprintln!("ls: specify a pod — `inspect ls {ns}/<pod>[:/path]`");
        return Ok(ExitKind::Error);
    }

    let mut argv: Vec<&str> = vec!["ls", "-1"];
    if args.long {
        argv.push("-l");
    }
    if args.all {
        argv.push("-A");
    }
    argv.push("--");
    argv.push(path);

    let res = crate::exec::kubectl::exec_in_pod(cfg, pod, None, &argv)?;
    if let Some(f) = res.failure {
        crate::tee_eprintln!("ls: [{}] {}", f.failure_class(), f.hint(""));
        return Ok(ExitKind::Inner(f.exit_code()));
    }
    let as_json = args.format.is_json();
    for line in res.stdout.lines() {
        if as_json {
            println!(
                "{}",
                serde_json::json!({ "entry": crate::format::safe::safe_machine_line(line).as_ref() })
            );
        } else {
            println!("{line}");
        }
    }
    Ok(ExitKind::Success)
}
