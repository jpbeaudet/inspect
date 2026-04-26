//! `inspect connections` — list inspect-managed persistent SSH masters.

use crate::cli::ConnectionsArgs;
use crate::commands::list::json_string;
use crate::config::resolver;
use crate::error::ExitKind;
use crate::ssh::master::{check_socket, list_sockets, MasterStatus};
use crate::ssh::SshTarget;

pub fn run(args: ConnectionsArgs) -> anyhow::Result<ExitKind> {
    let sockets = list_sockets()?;

    let mut rows: Vec<Row> = Vec::with_capacity(sockets.len());
    for (ns, sock) in sockets {
        let row = match resolver::resolve(&ns) {
            Ok(r) => match SshTarget::from_resolved(&r) {
                Ok(target) => {
                    let status = check_socket(&sock, &target);
                    Row {
                        namespace: ns,
                        host: format!("{}@{}:{}", target.user, target.host, target.port),
                        socket: sock.display().to_string(),
                        status: status.label().to_string(),
                    }
                }
                Err(e) => Row {
                    namespace: ns,
                    host: "<config error>".to_string(),
                    socket: sock.display().to_string(),
                    status: format!("error: {e}"),
                },
            },
            Err(_) => Row {
                namespace: ns,
                host: "<orphan: namespace not configured>".to_string(),
                socket: sock.display().to_string(),
                status: MasterStatus::Stale.label().to_string(),
            },
        };
        rows.push(row);
    }

    if args.format.is_json() {
        emit_json(&rows);
        return Ok(ExitKind::Success);
    }

    if rows.is_empty() {
        println!("SUMMARY: no inspect-managed connections");
        println!("DATA:    (none)");
        println!("NEXT:    inspect connect <ns>");
        return Ok(ExitKind::Success);
    }

    println!("SUMMARY: {} connection(s)", rows.len());
    println!("DATA:");
    println!("  NAMESPACE             HOST                                  STATUS    SOCKET");
    for r in &rows {
        println!(
            "  {:<20}  {:<37} {:<9} {}",
            r.namespace, r.host, r.status, r.socket,
        );
    }
    println!("NEXT:    inspect disconnect <ns>    inspect disconnect-all");
    Ok(ExitKind::Success)
}

struct Row {
    namespace: String,
    host: String,
    socket: String,
    status: String,
}

fn emit_json(rows: &[Row]) {
    let mut s = String::from("{\"schema_version\":1,\"connections\":[");
    for (i, r) in rows.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"namespace\":{ns},\"host\":{host},\"socket\":{sock},\"status\":{st}}}",
            ns = json_string(&r.namespace),
            host = json_string(&r.host),
            sock = json_string(&r.socket),
            st = json_string(&r.status),
        ));
    }
    s.push_str("]}");
    println!("{s}");
}
