//! `inspect disconnect-all` — close every inspect-managed master.

use std::io::{self, BufRead, Write};

use serde_json::{json, Value};

use crate::cli::DisconnectAllArgs;
use crate::config::resolver;
use crate::error::ExitKind;
use crate::ssh::master::{exit_master, list_sockets, socket_path};
use crate::ssh::SshTarget;
use crate::verbs::output::OutputDoc;

pub fn run(args: DisconnectAllArgs) -> anyhow::Result<ExitKind> {
    let sockets = list_sockets()?;
    if sockets.is_empty() {
        if args.format.is_json() {
            // P0.6 sweep (v0.1.3): L7 envelope.
            let doc = OutputDoc::new(
                "no inspect-managed connections to close",
                json!({ "closed": [], "failed": [] }),
            )
            .with_meta("count", 0);
            return doc.print_json(args.format.select_spec());
        }
        println!("SUMMARY: no inspect-managed connections to close");
        return Ok(ExitKind::Success);
    }

    if !args.yes && !confirm(sockets.len())? {
        println!("SUMMARY: cancelled");
        return Ok(ExitKind::Success);
    }

    let mut closed: Vec<String> = Vec::new();
    let mut failed: Vec<(String, String)> = Vec::new();

    for (ns, _sock) in &sockets {
        let socket = socket_path(ns);
        let target_result = resolver::resolve(ns)
            .map_err(anyhow::Error::from)
            .and_then(|r| SshTarget::from_resolved(&r));
        match target_result {
            Ok(target) => match exit_master(&socket, &target) {
                Ok(()) => closed.push(ns.clone()),
                Err(e) => failed.push((ns.clone(), e.to_string())),
            },
            Err(_) => {
                // Orphan socket: namespace no longer configured. Just delete
                // the socket file so it doesn't hang around.
                let _ = std::fs::remove_file(&socket);
                closed.push(ns.clone());
            }
        }
    }

    if args.format.is_json() {
        emit_json(&closed, &failed, &args.format)?;
    } else {
        println!(
            "SUMMARY: closed {} connection(s){}",
            closed.len(),
            if failed.is_empty() {
                String::new()
            } else {
                format!(", {} failed", failed.len())
            }
        );
        if !closed.is_empty() {
            println!("DATA:    {}", closed.join(", "));
        }
        for (ns, e) in &failed {
            eprintln!("  failed: {ns}: {e}");
        }
    }

    if failed.is_empty() {
        Ok(ExitKind::Success)
    } else {
        Ok(ExitKind::Error)
    }
}

fn confirm(n: usize) -> anyhow::Result<bool> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    write!(stdout, "Close {n} inspect-managed connection(s)? [y/N] ").ok();
    stdout.flush().ok();
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn emit_json(
    closed: &[String],
    failed: &[(String, String)],
    format: &crate::format::FormatArgs,
) -> anyhow::Result<()> {
    // P0.6 sweep (v0.1.3): L7 envelope.
    let summary = format!(
        "closed {} connection(s){}",
        closed.len(),
        if failed.is_empty() {
            String::new()
        } else {
            format!(", {} failed", failed.len())
        }
    );
    let failed_arr: Vec<Value> = failed
        .iter()
        .map(|(ns, err)| json!({ "namespace": ns, "error": err }))
        .collect();
    let data = json!({
        "closed": closed,
        "failed": failed_arr,
    });
    let doc = OutputDoc::new(summary, data)
        .with_meta("closed_count", closed.len())
        .with_meta("failed_count", failed.len());
    let _ = doc.print_json(format.select_spec())?;
    Ok(())
}
