//! `inspect disconnect-all` — close every inspect-managed master.

use std::io::{self, BufRead, Write};

use crate::cli::DisconnectAllArgs;
use crate::commands::list::json_string;
use crate::config::resolver;
use crate::error::ExitKind;
use crate::ssh::master::{exit_master, list_sockets, socket_path};
use crate::ssh::SshTarget;

pub fn run(args: DisconnectAllArgs) -> anyhow::Result<ExitKind> {
    let sockets = list_sockets()?;
    if sockets.is_empty() {
        if args.json {
            println!("{{\"schema_version\":1,\"closed\":[]}}");
        } else {
            println!("SUMMARY: no inspect-managed connections to close");
        }
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

    if args.json {
        emit_json(&closed, &failed);
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

fn emit_json(closed: &[String], failed: &[(String, String)]) {
    let mut s = String::from("{\"schema_version\":1,\"closed\":[");
    for (i, ns) in closed.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&json_string(ns));
    }
    s.push_str("],\"failed\":[");
    for (i, (ns, err)) in failed.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"namespace\":{ns},\"error\":{err}}}",
            ns = json_string(ns),
            err = json_string(err)
        ));
    }
    s.push_str("]}");
    println!("{s}");
}
