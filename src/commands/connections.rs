//! `inspect connections` — list inspect-managed persistent SSH masters.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use crate::cli::ConnectionsArgs;
use crate::config::resolver;
use crate::error::ExitKind;
use crate::ssh::master::{check_socket, list_sockets, MasterStatus};
use crate::ssh::ttl::{parse_ttl, resolve_with_ns};
use crate::ssh::SshTarget;
use crate::verbs::output::{NextStep, OutputDoc};

pub fn run(args: ConnectionsArgs) -> anyhow::Result<ExitKind> {
    let sockets = list_sockets()?;

    let mut rows: Vec<Row> = Vec::with_capacity(sockets.len());
    for (ns, sock) in sockets {
        let row = match resolver::resolve(&ns) {
            Ok(r) => match SshTarget::from_resolved(&r) {
                Ok(target) => {
                    let status = check_socket(&sock, &target);
                    // L4 (v0.1.3): surface auth mode + configured ttl
                    // + an upper-bound on remaining session lifetime.
                    // The expires_in figure is approximate: it
                    // measures elapsed wall-clock since the socket
                    // was created, against the configured ttl.
                    // ControlPersist resets on every traffic, so the
                    // real lifetime is at least this long; never
                    // shorter.
                    let password_auth = r.config.auth.as_deref() == Some("password");
                    let auth = if password_auth { "password" } else { "key" };
                    let ttl =
                        resolve_with_ns(None, r.config.session_ttl.as_deref(), Some(password_auth))
                            .map(|(t, _)| t)
                            .unwrap_or_else(|_| "?".to_string());
                    let expires_in = compute_expires_in(&sock, &ttl);
                    Row {
                        namespace: ns,
                        host: format!("{}@{}:{}", target.user, target.host, target.port),
                        socket: sock.display().to_string(),
                        status: status.label().to_string(),
                        auth: auth.to_string(),
                        session_ttl: ttl,
                        expires_in,
                    }
                }
                Err(e) => Row {
                    namespace: ns,
                    host: "<config error>".to_string(),
                    socket: sock.display().to_string(),
                    status: format!("error: {e}"),
                    auth: "?".to_string(),
                    session_ttl: "?".to_string(),
                    expires_in: None,
                },
            },
            Err(_) => Row {
                namespace: ns,
                host: "<orphan: namespace not configured>".to_string(),
                socket: sock.display().to_string(),
                status: MasterStatus::Stale.label().to_string(),
                auth: "?".to_string(),
                session_ttl: "?".to_string(),
                expires_in: None,
            },
        };
        rows.push(row);
    }

    if args.format.is_json() {
        return emit_json(&rows, &args.format);
    }

    if rows.is_empty() {
        println!("SUMMARY: no inspect-managed connections");
        println!("DATA:    (none)");
        println!("NEXT:    inspect connect <ns>");
        return Ok(ExitKind::Success);
    }

    println!("SUMMARY: {} connection(s)", rows.len());
    println!("DATA:");
    println!(
        "  {:<20}  {:<32} {:<9} {:<9} {:<6} {:<10} SOCKET",
        "NAMESPACE", "HOST", "STATUS", "AUTH", "TTL", "EXPIRES_IN"
    );
    for r in &rows {
        println!(
            "  {:<20}  {:<32} {:<9} {:<9} {:<6} {:<10} {}",
            r.namespace,
            r.host,
            r.status,
            r.auth,
            r.session_ttl,
            r.expires_in.as_deref().unwrap_or("?"),
            r.socket,
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
    /// L4 (v0.1.3): "key" or "password" (or "?" when the namespace
    /// is orphaned / config-broken).
    auth: String,
    /// L4 (v0.1.3): configured TTL string (e.g. "12h"). Reflects
    /// the same priority chain as `inspect connect`.
    session_ttl: String,
    /// L4 (v0.1.3): upper-bound time until the master would expire
    /// if no further traffic. `None` when the socket has no
    /// readable timestamp (orphan / config error).
    expires_in: Option<String>,
}

/// L4 (v0.1.3): upper-bound on remaining lifetime of an active
/// master. ControlPersist resets on every traffic so the real
/// lifetime is at least this long; never shorter. We use the
/// socket file's mtime as a "session opened at" proxy, which is
/// what ssh wrote when it materialized the control socket.
fn compute_expires_in(socket_path: &std::path::Path, ttl: &str) -> Option<String> {
    let meta = std::fs::metadata(socket_path).ok()?;
    let mtime = meta.modified().ok()?;
    let elapsed = SystemTime::now().duration_since(mtime).ok()?;
    let configured = parse_ttl(ttl).ok()?;
    let remaining = configured.checked_sub(elapsed).unwrap_or(Duration::ZERO);
    Some(format_duration(remaining))
}

fn format_duration(d: Duration) -> String {
    let total = d.as_secs();
    if total == 0 {
        return "0s".to_string();
    }
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 && m > 0 {
        format!("{h}h{m:02}m")
    } else if h > 0 {
        format!("{h}h")
    } else if m > 0 && s > 0 {
        format!("{m}m{s:02}s")
    } else if m > 0 {
        format!("{m}m")
    } else {
        format!("{s}s")
    }
}

fn emit_json(rows: &[Row], format: &crate::format::FormatArgs) -> anyhow::Result<ExitKind> {
    // P0.6 sweep (v0.1.3): L7 envelope. Pre-fix this verb emitted a
    // bare `{schema_version, connections}` shape — the surface that
    // surfaced as a non-L7 outlier during the SMOKE P1.5 live run on
    // 2026-05-09. The L4 fields (auth / session_ttl / expires_in)
    // ride on each row in `data.connections[]`.
    let connections: Vec<Value> = rows
        .iter()
        .map(|r| {
            let expires: Value = r
                .expires_in
                .as_ref()
                .map(|v| Value::String(v.clone()))
                .unwrap_or(Value::Null);
            json!({
                "namespace": r.namespace,
                "host": r.host,
                "socket": r.socket,
                "status": r.status,
                "auth": r.auth,
                "session_ttl": r.session_ttl,
                "expires_in": expires,
            })
        })
        .collect();
    let summary = if rows.is_empty() {
        "no inspect-managed connections".to_string()
    } else {
        format!("{} connection(s)", rows.len())
    };
    let data = json!({ "connections": connections });
    let mut doc = OutputDoc::new(summary, data).with_meta("count", rows.len());
    if rows.is_empty() {
        doc.push_next(NextStep::new("inspect connect <ns>", "open a master"));
    } else {
        doc.push_next(NextStep::new(
            "inspect disconnect <ns>",
            "close a single master",
        ));
        doc.push_next(NextStep::new(
            "inspect disconnect-all",
            "close every master at once",
        ));
    }
    doc.print_json(format.select_spec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l4_format_duration_compact_units() {
        assert_eq!(format_duration(Duration::from_secs(0)), "0s");
        assert_eq!(format_duration(Duration::from_secs(45)), "45s");
        assert_eq!(format_duration(Duration::from_secs(120)), "2m");
        assert_eq!(format_duration(Duration::from_secs(125)), "2m05s");
        assert_eq!(format_duration(Duration::from_secs(3600)), "1h");
        assert_eq!(
            format_duration(Duration::from_secs(3600 + 47 * 60)),
            "1h47m"
        );
        assert_eq!(
            format_duration(Duration::from_secs(11 * 3600 + 47 * 60)),
            "11h47m"
        );
    }
}
