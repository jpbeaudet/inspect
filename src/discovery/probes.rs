//! Per-source probes. Each probe runs a remote command via the persistent
//! SSH master and parses the output into typed fragments.
//!
//! Probes return a [`ProbeResult`] so the engine can record warnings and
//! degrade gracefully when a tool is missing or a command fails.

use crate::profile::schema::{
    HealthStatus, Image, LogDriver, Mount, Network, Port, RemoteTooling, Service, ServiceKind,
    Volume,
};
use crate::ssh::{run_remote, RunOpts, SshTarget};

/// Outcome of a single probe.
#[derive(Debug, Default)]
pub struct ProbeResult {
    pub services: Vec<Service>,
    pub volumes: Vec<Volume>,
    pub images: Vec<Image>,
    pub networks: Vec<Network>,
    pub remote_tooling: Option<RemoteTooling>,
    pub host_listeners: Vec<HostListener>,
    pub warnings: Vec<String>,
}

/// A single host-level listening socket discovered via `ss` / `netstat`.
#[derive(Debug, Clone)]
pub struct HostListener {
    pub port: u16,
    pub proto: String,
    pub process: Option<String>,
}

/// Probe which remote tools are present. Cheap; runs a single shell line.
pub fn probe_remote_tooling(ns: &str, target: &SshTarget) -> ProbeResult {
    // `command -v X >/dev/null 2>&1 && echo X=1 || echo X=0`
    let tools = [
        "rg", "jq", "journalctl", "sed", "grep", "netstat", "ss", "systemctl", "docker",
    ];
    let parts: Vec<String> = tools
        .iter()
        .map(|t| format!("(command -v {t} >/dev/null 2>&1 && echo {t}=1 || echo {t}=0)"))
        .collect();
    let cmd = parts.join("; ");

    let mut r = ProbeResult::default();
    match run_remote(ns, target, &cmd, RunOpts::with_timeout(10)) {
        Ok(out) if out.ok() => {
            let mut t = RemoteTooling::default();
            for line in out.stdout.lines() {
                let line = line.trim();
                let Some((k, v)) = line.split_once('=') else { continue };
                let present = v == "1";
                match k {
                    "rg" => t.rg = present,
                    "jq" => t.jq = present,
                    "journalctl" => t.journalctl = present,
                    "sed" => t.sed = present,
                    "grep" => t.grep = present,
                    "netstat" => t.netstat = present,
                    "ss" => t.ss = present,
                    "systemctl" => t.systemctl = present,
                    "docker" => t.docker = present,
                    _ => {}
                }
            }
            r.remote_tooling = Some(t);
        }
        Ok(out) => {
            r.warnings
                .push(format!("remote tooling probe exited {}: {}", out.exit_code, out.stderr.trim()));
        }
        Err(e) => r.warnings.push(format!("remote tooling probe failed: {e}")),
    }
    r
}

/// Probe Docker container inventory. Uses `docker ps` with a stable format
/// and `docker inspect` for ports and mounts. Falls back to a warning if
/// docker isn't installed or the user can't access the daemon.
pub fn probe_docker_containers(ns: &str, target: &SshTarget) -> ProbeResult {
    let mut r = ProbeResult::default();
    // 1) `docker ps` with TSV-friendly format. We avoid `--format '{{json .}}'`
    //    because some old daemons emit non-stable keys; instead we ask for
    //    explicit fields separated by tabs.
    let ps_fmt = "{{.ID}}\t{{.Names}}\t{{.Image}}\t{{.Status}}\t{{.State}}";
    let ps_cmd = format!("docker ps --no-trunc --format '{ps_fmt}' 2>/dev/null");
    let ps_out = match run_remote(ns, target, &ps_cmd, RunOpts::with_timeout(20)) {
        Ok(o) => o,
        Err(e) => {
            r.warnings.push(format!("docker ps failed: {e}"));
            return r;
        }
    };
    if !ps_out.ok() {
        r.warnings
            .push(format!("docker ps exited {}: {}", ps_out.exit_code, ps_out.stderr.trim()));
        return r;
    }

    let rows = parse_docker_ps(&ps_out.stdout);
    if rows.is_empty() {
        return r;
    }

    // 2) Collect ports + mounts + log driver via a single `docker inspect`
    //    on all the IDs. Output is JSON; we use `serde_json` to parse.
    let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
    let inspect_cmd = format!(
        "docker inspect --format '{{{{json .}}}}' {} 2>/dev/null",
        ids.join(" ")
    );
    let details = match run_remote(ns, target, &inspect_cmd, RunOpts::with_timeout(30)) {
        Ok(o) if o.ok() => parse_docker_inspect(&o.stdout),
        Ok(o) => {
            r.warnings.push(format!(
                "docker inspect exited {}: {}",
                o.exit_code,
                o.stderr.trim()
            ));
            std::collections::HashMap::new()
        }
        Err(e) => {
            r.warnings.push(format!("docker inspect failed: {e}"));
            std::collections::HashMap::new()
        }
    };

    for row in rows {
        let det = details.get(&row.id);
        let (ports, mounts, log_driver) = det
            .map(|d| (d.ports.clone(), d.mounts.clone(), d.log_driver))
            .unwrap_or_default();
        r.services.push(Service {
            name: row.name,
            container_id: Some(row.id),
            image: Some(row.image),
            ports,
            health: None,
            health_status: parse_health_from_status(&row.status),
            log_driver,
            log_readable_directly: matches!(log_driver, Some(LogDriver::JsonFile)),
            mounts,
            kind: ServiceKind::Container,
            depends_on: Vec::new(),
        });
    }
    r
}

/// Probe Docker volumes/networks/images. Each is independent and degrades
/// gracefully.
pub fn probe_docker_inventory(ns: &str, target: &SshTarget) -> ProbeResult {
    let mut r = ProbeResult::default();

    let vol_cmd = "docker volume ls --format '{{.Name}}\t{{.Driver}}\t{{.Mountpoint}}' 2>/dev/null";
    if let Ok(o) = run_remote(ns, target, vol_cmd, RunOpts::with_timeout(15)) {
        if o.ok() {
            for line in o.stdout.lines() {
                let cols: Vec<&str> = line.split('\t').collect();
                if cols.is_empty() || cols[0].is_empty() {
                    continue;
                }
                r.volumes.push(Volume {
                    name: cols[0].to_string(),
                    driver: cols.get(1).map(|s| s.to_string()).filter(|s| !s.is_empty()),
                    mountpoint: cols.get(2).map(|s| s.to_string()).filter(|s| !s.is_empty()),
                });
            }
        } else {
            r.warnings
                .push(format!("docker volume ls exited {}", o.exit_code));
        }
    }

    let net_cmd = "docker network ls --format '{{.Name}}\t{{.Driver}}\t{{.Scope}}' 2>/dev/null";
    if let Ok(o) = run_remote(ns, target, net_cmd, RunOpts::with_timeout(15)) {
        if o.ok() {
            for line in o.stdout.lines() {
                let cols: Vec<&str> = line.split('\t').collect();
                if cols.is_empty() || cols[0].is_empty() {
                    continue;
                }
                r.networks.push(Network {
                    name: cols[0].to_string(),
                    driver: cols.get(1).map(|s| s.to_string()).filter(|s| !s.is_empty()),
                    scope: cols.get(2).map(|s| s.to_string()).filter(|s| !s.is_empty()),
                });
            }
        }
    }

    let img_cmd = "docker image ls --format '{{.Repository}}:{{.Tag}}\t{{.ID}}\t{{.Size}}' --no-trunc 2>/dev/null";
    if let Ok(o) = run_remote(ns, target, img_cmd, RunOpts::with_timeout(15)) {
        if o.ok() {
            for line in o.stdout.lines() {
                let cols: Vec<&str> = line.split('\t').collect();
                if cols.is_empty() || cols[0].is_empty() || cols[0] == "<none>:<none>" {
                    continue;
                }
                r.images.push(Image {
                    repo_tag: cols[0].to_string(),
                    id: cols.get(1).map(|s| s.to_string()).filter(|s| !s.is_empty()),
                    size_bytes: None, // human size; deferred for now
                });
            }
        }
    }
    r
}

/// Probe host-level listening sockets via `ss -tlnpH` (preferred) with
/// `netstat -tlnp` fallback.
pub fn probe_host_listeners(ns: &str, target: &SshTarget) -> ProbeResult {
    let mut r = ProbeResult::default();
    let tries = [
        "ss -H -tlnp 2>/dev/null",
        "netstat -tlnp 2>/dev/null",
    ];
    for cmd in tries {
        if let Ok(out) = run_remote(ns, target, cmd, RunOpts::with_timeout(10)) {
            if out.ok() && !out.stdout.trim().is_empty() {
                for line in out.stdout.lines() {
                    if let Some(l) = parse_listener_line(line) {
                        r.host_listeners.push(l);
                    }
                }
                return r;
            }
        }
    }
    r.warnings
        .push("no host-port listing available (ss/netstat absent or no permission)".into());
    r
}

/// Probe non-Docker services via systemd. Returns at most a few hundred
/// `host_listener`-flavored entries; we keep the probe cheap.
pub fn probe_systemd_units(ns: &str, target: &SshTarget) -> ProbeResult {
    let mut r = ProbeResult::default();
    let cmd =
        "systemctl list-units --type=service --state=running --no-legend --plain 2>/dev/null";
    let out = match run_remote(ns, target, cmd, RunOpts::with_timeout(15)) {
        Ok(o) => o,
        Err(_) => return r,
    };
    if !out.ok() {
        return r;
    }
    for line in out.stdout.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.is_empty() {
            continue;
        }
        let name = cols[0].trim_end_matches(".service").to_string();
        if name.is_empty() {
            continue;
        }
        r.services.push(Service {
            name,
            container_id: None,
            image: None,
            ports: vec![],
            health: None,
            health_status: None,
            log_driver: None,
            log_readable_directly: false,
            mounts: vec![],
            kind: ServiceKind::Systemd,
            depends_on: vec![],
        });
    }
    r
}

// ---------- pure parsers (unit-tested without ssh) ---------------------------

#[derive(Debug)]
pub(crate) struct PsRow {
    pub id: String,
    pub name: String,
    pub image: String,
    pub status: String,
    #[allow(dead_code)]
    pub state: String,
}

pub(crate) fn parse_docker_ps(stdout: &str) -> Vec<PsRow> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 5 {
            continue;
        }
        let id = cols[0].trim();
        let name = cols[1].trim();
        if id.is_empty() || name.is_empty() {
            continue;
        }
        out.push(PsRow {
            id: id.to_string(),
            name: name.to_string(),
            image: cols[2].trim().to_string(),
            status: cols[3].trim().to_string(),
            state: cols[4].trim().to_string(),
        });
    }
    out
}

#[derive(Debug, Clone, Default)]
pub(crate) struct InspectDetail {
    pub ports: Vec<Port>,
    pub mounts: Vec<Mount>,
    pub log_driver: Option<LogDriver>,
}

pub(crate) fn parse_docker_inspect(
    stdout: &str,
) -> std::collections::HashMap<String, InspectDetail> {
    let mut out = std::collections::HashMap::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let id = v
            .get("Id")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            continue;
        }
        let mut det = InspectDetail::default();

        // Ports: NetworkSettings.Ports = { "8000/tcp": [{HostIp, HostPort}, ...] }
        if let Some(ports) = v
            .get("NetworkSettings")
            .and_then(|n| n.get("Ports"))
            .and_then(|p| p.as_object())
        {
            for (key, bindings) in ports {
                let (cport, proto) = match key.split_once('/') {
                    Some((p, pr)) => (p, pr),
                    None => (key.as_str(), "tcp"),
                };
                let cport: u16 = match cport.parse() {
                    Ok(n) => n,
                    Err(_) => continue,
                };
                if let Some(arr) = bindings.as_array() {
                    for b in arr {
                        if let Some(hp) = b.get("HostPort").and_then(|x| x.as_str()) {
                            if let Ok(host) = hp.parse::<u16>() {
                                det.ports.push(Port {
                                    host,
                                    container: cport,
                                    proto: proto.to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }

        // Mounts: top-level Mounts array of { Source, Destination, Type }
        if let Some(arr) = v.get("Mounts").and_then(|x| x.as_array()) {
            for m in arr {
                let source = m
                    .get("Source")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let target = m
                    .get("Destination")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let ty = m
                    .get("Type")
                    .and_then(|x| x.as_str())
                    .unwrap_or("bind")
                    .to_string();
                if !source.is_empty() && !target.is_empty() {
                    det.mounts.push(Mount {
                        source,
                        target,
                        mount_type: ty,
                    });
                }
            }
        }

        // Log driver: HostConfig.LogConfig.Type
        if let Some(t) = v
            .get("HostConfig")
            .and_then(|h| h.get("LogConfig"))
            .and_then(|l| l.get("Type"))
            .and_then(|x| x.as_str())
        {
            det.log_driver = match t {
                "json-file" => Some(LogDriver::JsonFile),
                "journald" => Some(LogDriver::Journald),
                "local" => Some(LogDriver::Local),
                "syslog" => Some(LogDriver::Syslog),
                _ => Some(LogDriver::Other),
            };
        }
        out.insert(id, det);
    }
    out
}

pub(crate) fn parse_health_from_status(status: &str) -> Option<HealthStatus> {
    let lc = status.to_lowercase();
    if lc.contains("(healthy)") {
        Some(HealthStatus::Ok)
    } else if lc.contains("(unhealthy)") {
        Some(HealthStatus::Unhealthy)
    } else if lc.contains("(health: starting)") || lc.contains("starting") {
        Some(HealthStatus::Starting)
    } else if lc.starts_with("up") {
        // No explicit health probe configured; we don't claim unknown unless
        // the docker status itself tells us nothing useful.
        Some(HealthStatus::Unknown)
    } else {
        None
    }
}

pub(crate) fn parse_listener_line(line: &str) -> Option<HostListener> {
    // ss -H -tlnp output (one socket per line):
    //   LISTEN 0  4096  0.0.0.0:22  0.0.0.0:*  users:(("sshd",pid=1,fd=3))
    // netstat -tlnp output:
    //   tcp  0  0  0.0.0.0:22  0.0.0.0:*  LISTEN  1/sshd
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let toks: Vec<&str> = line.split_whitespace().collect();
    // Find a `*:port` or `addr:port` token.
    let bind = toks.iter().find(|t| t.contains(':') && !t.contains("users:"))?;
    let port_str = bind.rsplit(':').next()?;
    let port: u16 = port_str.parse().ok()?;

    // Both `ss -tln` and `netstat -tln` filter to TCP, so we always tag
    // these listeners as `tcp`. UDP discovery is deferred.
    let proto = "tcp";

    let process = extract_process(line);
    Some(HostListener {
        port,
        proto: proto.to_string(),
        process,
    })
}

fn extract_process(line: &str) -> Option<String> {
    // ss style: users:(("sshd",pid=1,fd=3))
    if let Some(idx) = line.find("users:((\"") {
        let rest = &line[idx + "users:((\"".len()..];
        if let Some(end) = rest.find('"') {
            return Some(rest[..end].to_string());
        }
    }
    // netstat style: ` 1/sshd` at end
    if let Some(slash) = line.rfind('/') {
        let after = &line[slash + 1..];
        let name: String = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
            .collect();
        if !name.is_empty() {
            return Some(name);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_docker_ps_typical() {
        let s = "abc123\tnginx\tnginx:1.27\tUp 2 hours (healthy)\trunning\n\
                 def456\tdb\tpostgres:16\tUp 5 days\trunning";
        let rows = parse_docker_ps(s);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "nginx");
        assert_eq!(rows[0].image, "nginx:1.27");
        assert_eq!(rows[1].id, "def456");
    }

    #[test]
    fn parse_inspect_extracts_ports_mounts_and_driver() {
        let s = r#"{"Id":"abc123","NetworkSettings":{"Ports":{"8000/tcp":[{"HostIp":"0.0.0.0","HostPort":"8000"}]}},"Mounts":[{"Source":"/a","Destination":"/b","Type":"bind"}],"HostConfig":{"LogConfig":{"Type":"json-file"}}}"#;
        let m = parse_docker_inspect(s);
        let d = m.get("abc123").unwrap();
        assert_eq!(d.ports.len(), 1);
        assert_eq!(d.ports[0].host, 8000);
        assert_eq!(d.ports[0].container, 8000);
        assert_eq!(d.mounts.len(), 1);
        assert_eq!(d.mounts[0].target, "/b");
        assert!(matches!(d.log_driver, Some(LogDriver::JsonFile)));
    }

    #[test]
    fn parse_health_status_variants() {
        assert!(matches!(
            parse_health_from_status("Up 2 hours (healthy)"),
            Some(HealthStatus::Ok)
        ));
        assert!(matches!(
            parse_health_from_status("Up 30 seconds (unhealthy)"),
            Some(HealthStatus::Unhealthy)
        ));
        assert!(matches!(
            parse_health_from_status("Up Less than a second (health: starting)"),
            Some(HealthStatus::Starting)
        ));
        assert!(matches!(
            parse_health_from_status("Up 12 days"),
            Some(HealthStatus::Unknown)
        ));
        assert!(parse_health_from_status("Exited (0)").is_none());
    }

    #[test]
    fn parse_ss_line() {
        let l = "LISTEN 0  4096  0.0.0.0:22  0.0.0.0:*  users:((\"sshd\",pid=1,fd=3))";
        let r = parse_listener_line(l).unwrap();
        assert_eq!(r.port, 22);
        assert_eq!(r.process.as_deref(), Some("sshd"));
    }

    #[test]
    fn parse_netstat_line() {
        let l = "tcp        0      0 0.0.0.0:8080            0.0.0.0:*               LISTEN      42/myapp";
        let r = parse_listener_line(l).unwrap();
        assert_eq!(r.port, 8080);
        assert_eq!(r.process.as_deref(), Some("myapp"));
    }
}
