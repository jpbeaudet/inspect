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
use crate::verbs::quote::shquote;

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
        "rg",
        "jq",
        "journalctl",
        "sed",
        "grep",
        "netstat",
        "ss",
        "systemctl",
        "docker",
        "podman",
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
                let Some((k, v)) = line.split_once('=') else {
                    continue;
                };
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
                    "podman" => t.podman = present,
                    _ => {}
                }
            }
            // Audit §6.2: if neither container engine is present,
            // surface a single actionable line rather than letting the
            // user wade through nine "X=0" lines.
            if !t.docker && !t.podman {
                r.warnings.push(
                    "no container engine found on host (neither `docker` nor `podman` in PATH)"
                        .to_string(),
                );
            }
            r.remote_tooling = Some(t);
        }
        Ok(out) => {
            r.warnings.push(format!(
                "remote tooling probe exited {}: {}",
                out.exit_code,
                out.stderr.trim()
            ));
        }
        Err(e) => r.warnings.push(format!("remote tooling probe failed: {e}")),
    }
    // Field pitfall §1.1: surface a one-line warning when the remote's
    // sshd `MaxSessions` is below our local per-host concurrency cap.
    // Ignore failures silently — `sshd -T` typically requires root and
    // `/etc/ssh/sshd_config` may be unreadable; the goal is to *help
    // when we can*, not block discovery when we can't.
    if let Some(w) = probe_max_sessions(ns, target) {
        r.warnings.push(w);
    }
    r
}

/// Field pitfall §1.1: best-effort detection of the remote sshd's
/// `MaxSessions`. Returns a warning string when the remote setting is
/// lower than `INSPECT_MAX_SESSIONS_PER_HOST`. Returns `None` when the
/// setting could not be read or it is at least as large as our cap.
fn probe_max_sessions(ns: &str, target: &SshTarget) -> Option<String> {
    let local_cap = std::env::var("INSPECT_MAX_SESSIONS_PER_HOST")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(8);
    // `sshd -T` is the source of truth but normally requires root;
    // `awk` over `/etc/ssh/sshd_config` is the unprivileged fallback.
    // Either way the output is a single integer or empty.
    let cmd = "(sshd -T 2>/dev/null | awk '/^maxsessions /{print $2; exit}') \
               || (awk '/^[[:space:]]*MaxSessions[[:space:]]+/{print $2; exit}' \
                       /etc/ssh/sshd_config 2>/dev/null) \
               || true";
    let out = run_remote(ns, target, cmd, RunOpts::with_timeout(5)).ok()?;
    if !out.ok() {
        return None;
    }
    let raw = out.stdout.trim();
    if raw.is_empty() {
        return None;
    }
    let remote: u32 = raw.parse().ok()?;
    // OpenSSH's compiled-in default is 10; treat anything ≥ our cap as fine.
    if remote >= local_cap {
        return None;
    }
    Some(format!(
        "remote sshd MaxSessions={remote} is below the local per-host cap of {local_cap}; \
         `inspect` may queue or fail with `administratively prohibited`. \
         Either lower `INSPECT_MAX_SESSIONS_PER_HOST` (currently {local_cap}) \
         or raise `MaxSessions` on the remote sshd_config to at least {local_cap}."
    ))
}

/// Probe Docker container inventory. Uses `docker ps` with a stable format
/// and `docker inspect` for ports and mounts. Falls back to a warning if
/// docker isn't installed or the user can't access the daemon.
pub fn probe_docker_containers(ns: &str, target: &SshTarget) -> ProbeResult {
    let mut r = ProbeResult::default();
    // 1) `docker ps` with TSV-friendly format. We avoid `--format '{{json .}}'`
    //    because some old daemons emit non-stable keys; instead we ask for
    //    explicit fields separated by tabs.
    //
    //    Field pitfall §6.1: include the `com.docker.compose.service`
    //    label so we can prefer the operator-facing service name over
    //    the docker-generated container name (`<project>_<service>_1`,
    //    `<project>-<service>-1`, etc.). When the label is absent we
    //    fall back to `{{.Names}}` so non-compose containers still work.
    let ps_fmt = "{{.ID}}\t{{.Names}}\t{{.Image}}\t{{.Status}}\t{{.State}}\t{{.Label \"com.docker.compose.service\"}}";
    let ps_cmd = format!("docker ps --no-trunc --format '{ps_fmt}' 2>/dev/null");
    let ps_out = match run_remote(ns, target, &ps_cmd, RunOpts::with_timeout(20)) {
        Ok(o) => o,
        Err(e) => {
            r.warnings.push(format!("docker ps failed: {e}"));
            return r;
        }
    };
    if !ps_out.ok() {
        let stderr = ps_out.stderr.trim();
        if let Some(hint) = explain_docker_failure(stderr) {
            r.warnings.push(format!(
                "docker ps exited {}: {} -- {}",
                ps_out.exit_code, stderr, hint
            ));
        } else {
            r.warnings
                .push(format!("docker ps exited {}: {}", ps_out.exit_code, stderr));
        }
        return r;
    }

    let rows = parse_docker_ps(&ps_out.stdout);
    if rows.is_empty() {
        return r;
    }

    // 2) Collect ports + mounts + log driver via a single `docker inspect`
    //    on all the IDs. Output is JSON; we use `serde_json` to parse.
    //
    // P13: if the batch call times out we fall back to inspecting each
    // container individually with a tighter per-call timeout. A single
    // wedged container (e.g. one whose Docker daemon socket is slow,
    // or whose health check is hung) used to take the entire host's
    // discovery down with it; now we record a warning, mark just that
    // service as `discovery_incomplete`, and keep going.
    let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
    let mut incomplete_ids: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    let inspect_cmd = format!(
        "docker inspect --format '{{{{json .}}}}' {} 2>/dev/null",
        ids.join(" ")
    );
    let details = match run_remote(ns, target, &inspect_cmd, RunOpts::with_timeout(10)) {
        Ok(o) if o.ok() => parse_docker_inspect(&o.stdout),
        // Batched call failed (timeout, partial JSON, daemon hiccup) --
        // probe each container individually with a 5s budget so a
        // single wedged container can't block the rest.
        Ok(o) => {
            r.warnings.push(format!(
                "docker inspect (batched) exited {}: {} -- falling back to per-container probe",
                o.exit_code,
                o.stderr.trim()
            ));
            inspect_per_container(ns, target, &ids, &mut r, &mut incomplete_ids)
        }
        Err(e) => {
            r.warnings.push(format!(
                "docker inspect (batched) failed: {e} -- falling back to per-container probe"
            ));
            inspect_per_container(ns, target, &ids, &mut r, &mut incomplete_ids)
        }
    };

    // Field pitfall §2.1: warn when any json-file container log has
    // grown past `INSPECT_LOG_SIZE_WARN_BYTES` (default 1 GiB). One
    // batched `stat -c '%s\t%n'` call covers every log path we know
    // about so we don't pay an SSH round-trip per container.
    log_size_warnings(ns, target, &rows, &details, &mut r);
    // Field pitfall §6.1: when two compose containers from the same
    // service (replicas) both expose the same compose label, we'd
    // otherwise emit two services with the same `name`. Deduplicate
    // by selecting the first occurrence and recording a warning so
    // the operator knows the second replica is reachable only by
    // its full container name (we still keep the long name as the
    // service entry for the second one).
    let mut seen_compose: std::collections::HashSet<String> = std::collections::HashSet::new();

    for row in rows {
        // Field pitfall §6.1: prefer the compose service label when
        // present, but fall back to the container name. We only swap
        // when the label is unambiguous within this host (see
        // `seen_compose` above). The user-facing `name` is what
        // selectors match against; the *real* container name (always
        // `row.name`) is preserved separately as `container_name` and
        // is what every `docker logs|exec|restart` actually targets.
        // Without this split, a profile with `name: api` would
        // resolve in `inspect logs arte/api` but then run
        // `docker logs api` on a host whose container is actually
        // `luminary-api` — that's the v0.1.0 phantom-service bug.
        let svc_name = match &row.compose_service {
            Some(label) if !seen_compose.contains(label) => {
                seen_compose.insert(label.clone());
                label.clone()
            }
            _ => row.name.clone(),
        };
        // Audit §7.2 / §7.3: warn when a service name collides with a
        // selector reserved char or the host placeholder. The service
        // is still discovered (we don't drop data), but operators get
        // a one-line heads-up so they understand why selectors like
        // `srv,foo` won't match it.
        if let Some(reason) = problematic_service_name(&svc_name) {
            r.warnings.push(format!(
                "container '{}' on {}: {} -- selectors that target it must use the regex form `/{}$/`",
                svc_name,
                target.host,
                reason,
                regex::escape(&svc_name),
            ));
        }
        let det = details.get(&row.id);
        let incomplete = incomplete_ids.contains(&row.id);
        let (ports, mounts, log_driver) = det
            .map(|d| (d.ports.clone(), d.mounts.clone(), d.log_driver))
            .unwrap_or_default();
        // Field pitfall §2.3: warn for known-unsupported drivers at
        // discovery time so `inspect setup` surfaces the issue once,
        // not on every `inspect logs` call.
        if let Some(d) = log_driver {
            if !d.is_readable_via_docker_logs() {
                r.warnings.push(format!(
                    "service '{}' on {}: log driver `{}` is not readable via `docker logs` -- \
                     `inspect logs` will fail with an actionable error; route logs through the driver's sink instead",
                    svc_name,
                    target.host,
                    d.as_str(),
                ));
            }
        }
        r.services.push(Service {
            name: svc_name,
            container_name: row.name.clone(),
            compose_service: row.compose_service.clone(),
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
            discovery_incomplete: incomplete,
        });
    }
    r
}

/// Field pitfall §5.3: probe the *signed* offset (in seconds) between
/// the remote clock and our local clock. Returns `None` when the probe
/// fails so the caller can keep going (this is a soft warning, not a
/// fatal error). Result kept on the [`ProbeResult`] via the `services`
/// channel is awkward, so we expose a dedicated function.
pub fn probe_clock_offset(ns: &str, target: &SshTarget) -> (Option<i64>, Vec<String>) {
    // We measure round-trip time so we can subtract the SSH RTT from
    // the apparent offset. `date +%s` runs in well under a second on
    // any reasonable host, but ssh setup itself can add 200-800ms on
    // a fresh connection. Without this correction every freshly-
    // connected host would look 0.5s ahead.
    let cmd = "date +%s";
    let local_before = std::time::SystemTime::now();
    let out = match run_remote(ns, target, cmd, RunOpts::with_timeout(10)) {
        Ok(o) if o.ok() => o,
        Ok(o) => {
            return (
                None,
                vec![format!(
                    "clock-offset probe (`date +%s`) exited {}: {}",
                    o.exit_code,
                    o.stderr.trim()
                )],
            );
        }
        Err(e) => {
            return (None, vec![format!("clock-offset probe failed: {e}")]);
        }
    };
    let local_after = std::time::SystemTime::now();
    // Midpoint of local clock during the remote read.
    let local_mid = match (
        local_before.duration_since(std::time::UNIX_EPOCH),
        local_after.duration_since(std::time::UNIX_EPOCH),
    ) {
        (Ok(a), Ok(b)) => (a.as_secs() as i64 + b.as_secs() as i64) / 2,
        _ => return (None, vec!["local clock is before unix epoch".to_string()]),
    };
    let remote = match out.stdout.trim().parse::<i64>() {
        Ok(n) => n,
        Err(_) => {
            return (
                None,
                vec![format!(
                    "clock-offset probe returned non-numeric output: {}",
                    out.stdout.trim()
                )],
            );
        }
    };
    let offset = remote - local_mid;
    let mut warnings = Vec::new();
    // 5s threshold is the same one Kubernetes uses to flag NTP-skew
    // warnings on kubelet; it's small enough to surface real problems
    // (clock not syncing) and large enough not to cry wolf about
    // network jitter.
    if offset.abs() >= 5 {
        warnings.push(format!(
            "clock skew detected on {}: remote is {} seconds {} the local clock -- \
             `--since`/`--until` with absolute timestamps may surprise; check NTP",
            target.host,
            offset.abs(),
            if offset > 0 { "ahead of" } else { "behind" },
        ));
    }
    (Some(offset), warnings)
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
    let tries = ["ss -H -tlnp 2>/dev/null", "netstat -tlnp 2>/dev/null"];
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
    let cmd = "systemctl list-units --type=service --state=running --no-legend --plain 2>/dev/null";
    let out = match run_remote(ns, target, cmd, RunOpts::with_timeout(15)) {
        Ok(o) => o,
        Err(_) => return r,
    };
    if !out.ok() {
        return r;
    }
    let filter = systemd_unit_filter();
    let mut skipped = 0usize;
    for line in out.stdout.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.is_empty() {
            continue;
        }
        let name = cols[0].trim_end_matches(".service").to_string();
        if name.is_empty() {
            continue;
        }
        // Field pitfall §6.3: filter out OS-internal units (dbus,
        // cron, systemd-*, getty@, user@, etc.) so the inventory
        // shows only operator-relevant services. Override via
        // `INSPECT_SYSTEMD_INCLUDE=<regex>` (matches in addition to
        // the allowlist) or `INSPECT_SYSTEMD_NO_FILTER=1` to keep
        // every running unit (debug only).
        if !filter.allows(&name) {
            skipped += 1;
            continue;
        }
        r.services.push(Service {
            name: name.clone(),
            container_name: name,
            compose_service: None,
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
            discovery_incomplete: false,
        });
    }
    if skipped > 0 {
        r.warnings.push(format!(
            "systemd: filtered {skipped} OS-internal unit(s) from inventory \
             (set INSPECT_SYSTEMD_NO_FILTER=1 to keep every running unit, \
             or INSPECT_SYSTEMD_INCLUDE=<regex> to add specific names)"
        ));
    }
    r
}

/// Field pitfall §6.3: predicate over systemd unit names that
/// suppresses OS-internal noise by default.
pub(crate) struct SystemdUnitFilter {
    no_filter: bool,
    extra: Option<regex::Regex>,
}

impl SystemdUnitFilter {
    pub fn allows(&self, name: &str) -> bool {
        if self.no_filter {
            return true;
        }
        if let Some(re) = &self.extra {
            if re.is_match(name) {
                return true;
            }
        }
        !systemd_name_is_os_internal(name)
    }
}

pub(crate) fn systemd_unit_filter() -> SystemdUnitFilter {
    let no_filter = matches!(
        std::env::var("INSPECT_SYSTEMD_NO_FILTER").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    );
    let extra = std::env::var("INSPECT_SYSTEMD_INCLUDE")
        .ok()
        .and_then(|s| regex::Regex::new(&s).ok());
    SystemdUnitFilter { no_filter, extra }
}

/// Names matching this list are OS-internal infrastructure that no
/// operator actually wants in the inventory by default. Curated from
/// `systemctl list-units --state=running` on a stock Debian/Ubuntu
/// server (the union of the dozen most common offenders).
pub(crate) fn systemd_name_is_os_internal(name: &str) -> bool {
    // Prefix matches first.
    const PREFIXES: &[&str] = &[
        "systemd-",
        "user@",
        "session-",
        "getty@",
        "serial-getty@",
        "container-getty@",
        "user-runtime-dir@",
        "modprobe@",
        "rc-local",
    ];
    for p in PREFIXES {
        if name.starts_with(p) {
            return true;
        }
    }
    // Exact matches.
    matches!(
        name,
        "dbus"
            | "cron"
            | "crond"
            | "polkit"
            | "rpcbind"
            | "chrony"
            | "chronyd"
            | "ntp"
            | "ntpd"
            | "ssh"
            | "sshd"
            | "accounts-daemon"
            | "networkd-dispatcher"
            | "ModemManager"
            | "NetworkManager"
            | "wpa_supplicant"
            | "udisks2"
            | "colord"
            | "avahi-daemon"
            | "cups"
            | "cups-browsed"
            | "snapd"
            | "snapd.socket"
            | "rsyslog"
            | "auditd"
            | "atd"
            | "irqbalance"
            | "uuidd"
            | "lvm2-monitor"
            | "thermald"
            | "unattended-upgrades"
            | "multipathd"
            | "packagekit"
            | "fwupd"
    )
}

/// Field pitfall §2.1: emit a warning for any json-file log past
/// the size threshold. Soft probe — a `stat` failure (no permission,
/// missing path) is silently ignored.
fn log_size_warnings(
    ns: &str,
    target: &SshTarget,
    rows: &[PsRow],
    details: &std::collections::HashMap<String, InspectDetail>,
    r: &mut ProbeResult,
) {
    let threshold = log_size_warn_threshold();
    if threshold == u64::MAX {
        return;
    }
    // Collect (svc_name, path) for every container with a log path.
    let mut paths: Vec<(String, String)> = Vec::new();
    for row in rows {
        let Some(d) = details.get(&row.id) else {
            continue;
        };
        let Some(p) = &d.log_path else { continue };
        // Only json-file is sized this way; journald/local store
        // elsewhere and have their own retention.
        if !matches!(d.log_driver, Some(LogDriver::JsonFile)) {
            continue;
        }
        paths.push((row.name.clone(), p.clone()));
    }
    if paths.is_empty() {
        return;
    }
    // Build a single `stat` call. `-c '%s\t%n'` prints "<size>\t<path>"
    // per file; missing paths produce a warning on stderr we ignore.
    let mut cmd = String::from("stat -c '%s\t%n'");
    for (_, p) in &paths {
        cmd.push(' ');
        cmd.push_str(&shquote(p));
    }
    cmd.push_str(" 2>/dev/null");
    let out = match run_remote(ns, target, &cmd, RunOpts::with_timeout(20)) {
        Ok(o) if o.ok() => o,
        // Most common reason: the docker daemon stores logs under
        // /var/lib/docker which is root-only on most distros. We don't
        // want to spam every operator running as a non-root user.
        _ => return,
    };
    let by_path: std::collections::HashMap<&str, &str> = paths
        .iter()
        .map(|(s, p)| (p.as_str(), s.as_str()))
        .collect();
    for line in out.stdout.lines() {
        let mut it = line.splitn(2, '\t');
        let size_s = match it.next() {
            Some(s) => s,
            None => continue,
        };
        let path = match it.next() {
            Some(p) => p,
            None => continue,
        };
        let size: u64 = match size_s.parse() {
            Ok(n) => n,
            Err(_) => continue,
        };
        if size < threshold {
            continue;
        }
        let svc = by_path.get(path).copied().unwrap_or("?");
        r.warnings.push(format!(
            "service '{}' on {}: log file `{}` is {} (>{}) -- consider rotating with \
             `--log-opt max-size=100m --log-opt max-file=3`, or `truncate -s 0 <path>`",
            svc,
            target.host,
            path,
            human_bytes(size),
            human_bytes(threshold),
        ));
    }
}

fn log_size_warn_threshold() -> u64 {
    // Default 1 GiB; `INSPECT_LOG_SIZE_WARN_BYTES=0` disables the probe.
    match std::env::var("INSPECT_LOG_SIZE_WARN_BYTES") {
        Ok(v) => match v.parse::<u64>() {
            Ok(0) => u64::MAX,
            Ok(n) => n,
            Err(_) => 1 << 30,
        },
        Err(_) => 1 << 30,
    }
}

fn human_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut idx = 0usize;
    let mut v = n as f64;
    while v >= 1024.0 && idx + 1 < UNITS.len() {
        v /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{} {}", n, UNITS[idx])
    } else {
        format!("{:.1} {}", v, UNITS[idx])
    }
}

// ---------- pure parsers (unit-tested without ssh) ---------------------------

#[derive(Debug)]
pub(crate) struct PsRow {
    pub id: String,
    pub name: String,
    pub image: String,
    pub status: String,
    /// Field pitfall §6.1: value of the `com.docker.compose.service`
    /// label, when present. When non-empty this is preferred over
    /// `name` as the user-facing service identifier.
    pub compose_service: Option<String>,
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
        // The compose-service label column is optional: older daemons
        // and the pre-§6.1 format don't include it. `cols.get(5)`
        // gracefully degrades to `None`.
        let compose_service = cols
            .get(5)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty() && *s != "<no value>")
            .map(|s| s.to_string());
        out.push(PsRow {
            id: id.to_string(),
            name: name.to_string(),
            image: cols[2].trim().to_string(),
            status: cols[3].trim().to_string(),
            compose_service,
        });
    }
    out
}

#[derive(Debug, Clone, Default)]
pub(crate) struct InspectDetail {
    pub ports: Vec<Port>,
    pub mounts: Vec<Mount>,
    pub log_driver: Option<LogDriver>,
    /// Field pitfall §2.1: absolute path to the active json-file
    /// log on the daemon's host filesystem (`LogPath` from
    /// `docker inspect`). Used to size-warn at discovery time.
    pub log_path: Option<String>,
}

/// P13: per-container `docker inspect` fallback used when the batched
/// call timed out or otherwise failed. Each id gets its own 5-second
/// budget; ids whose individual probe also fails are recorded in
/// `incomplete_ids` so the caller can flag the corresponding service
/// with `discovery_incomplete = true`. Returns the merged
/// detail-by-id map of every container we *did* successfully inspect.
fn inspect_per_container(
    ns: &str,
    target: &SshTarget,
    ids: &[&str],
    r: &mut ProbeResult,
    incomplete_ids: &mut std::collections::HashSet<String>,
) -> std::collections::HashMap<String, InspectDetail> {
    let mut out = std::collections::HashMap::new();
    for id in ids {
        let cmd = format!("docker inspect --format '{{{{json .}}}}' {id} 2>/dev/null");
        match run_remote(ns, target, &cmd, RunOpts::with_timeout(5)) {
            Ok(o) if o.ok() => {
                let part = parse_docker_inspect(&o.stdout);
                out.extend(part);
            }
            Ok(o) => {
                r.warnings.push(format!(
                    "docker inspect for container {id} exited {}: {} -- service marked incomplete",
                    o.exit_code,
                    o.stderr.trim()
                ));
                incomplete_ids.insert((*id).to_string());
            }
            Err(e) => {
                r.warnings.push(format!(
                    "docker inspect for container {id} failed: {e} -- service marked incomplete"
                ));
                incomplete_ids.insert((*id).to_string());
            }
        }
    }
    out
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
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
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
                // Field pitfall §2.3: distinguish the unsupported
                // (read-via-docker) drivers so the `logs` verb can
                // emit a clear, driver-specific error instead of
                // returning empty output.
                "fluentd" => Some(LogDriver::Fluentd),
                "awslogs" => Some(LogDriver::Awslogs),
                "gelf" => Some(LogDriver::Gelf),
                "splunk" => Some(LogDriver::Splunk),
                "none" => Some(LogDriver::None),
                _ => Some(LogDriver::Other),
            };
        }

        // Field pitfall §2.1: capture LogPath so a follow-up `du`
        // probe can warn the operator when the json-file log has
        // grown past a sane threshold.
        if let Some(p) = v.get("LogPath").and_then(|x| x.as_str()) {
            if !p.is_empty() {
                det.log_path = Some(p.to_string());
            }
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
    let bind = toks
        .iter()
        .find(|t| t.contains(':') && !t.contains("users:"))?;
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

/// Detect container/service names that will collide with selector
/// syntax (audit §7.2, §7.3). Returns a human-readable reason when the
/// name is problematic, or `None` when it's safe.
///
/// Trip points:
///   * `_` is the reserved host-level placeholder; a real container
///     literally named `_` cannot be addressed without the regex
///     escape hatch.
///   * `,` separates services in a selector list.
///   * `/`, `:`, `*`, `~`, `[`, `]`, `{`, `}`, ` `, `\t` are reserved
///     by the selector grammar (path separator, regex delimiters,
///     glob metas, whitespace).
pub(crate) fn problematic_service_name(name: &str) -> Option<String> {
    if name == "_" {
        return Some("name `_` collides with the reserved host-level placeholder".to_string());
    }
    const RESERVED: &[char] = &[',', '/', ':', '*', '~', '[', ']', '{', '}', ' ', '\t'];
    let bad: Vec<char> = name.chars().filter(|c| RESERVED.contains(c)).collect();
    if !bad.is_empty() {
        let mut seen = std::collections::BTreeSet::new();
        for c in bad {
            seen.insert(c);
        }
        let list: Vec<String> = seen.into_iter().map(|c| format!("{c:?}")).collect();
        return Some(format!(
            "name contains selector-reserved chars: {}",
            list.join(", ")
        ));
    }
    None
}

/// Translate common docker CLI failures into a one-line actionable
/// hint (audit §6.1, §6.2). Returns `None` when we don't recognize the
/// failure — the raw stderr is still surfaced separately.
pub(crate) fn explain_docker_failure(stderr: &str) -> Option<&'static str> {
    let s = stderr.to_ascii_lowercase();
    if s.contains("permission denied") && (s.contains("docker.sock") || s.contains("docker daemon"))
    {
        return Some(
            "add user to the `docker` group (`sudo usermod -aG docker $USER`, then re-login), \
             run with `sudo`, or set `DOCKER_HOST` to a socket you can access",
        );
    }
    if s.contains("cannot connect to the docker daemon") {
        return Some(
            "the docker daemon is not running on this host -- start it with \
             `sudo systemctl start docker`, or set `DOCKER_HOST` if it lives elsewhere",
        );
    }
    if s.contains("command not found") || s.contains("docker: not found") {
        return Some(
            "the `docker` binary is not in PATH on this host -- if you use podman, \
             install `podman-docker` or alias `docker=podman`",
        );
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
        // Field pitfall §6.1: legacy 5-column rows must still parse;
        // compose_service is None.
        assert!(rows[0].compose_service.is_none());
    }

    #[test]
    fn parse_docker_ps_with_compose_label() {
        // Field pitfall §6.1: prefer compose label as service name.
        let s = "abc123\tmyproject_pulse_1\tluminary/pulse:1\tUp\trunning\tpulse\n\
                 def456\tdb-1\tpostgres:16\tUp\trunning\t<no value>";
        let rows = parse_docker_ps(s);
        assert_eq!(rows.len(), 2);
        // The label is captured when present and non-empty.
        assert_eq!(rows[0].compose_service.as_deref(), Some("pulse"));
        // `<no value>` (docker's literal for missing labels) is treated
        // as absent so the container name remains the fallback.
        assert!(rows[1].compose_service.is_none());
    }

    #[test]
    fn parse_inspect_recognises_unsupported_log_drivers() {
        // Field pitfall §2.3: distinguish unsupported drivers so the
        // logs verb can emit a clear, driver-specific error.
        let cases = [
            ("fluentd", LogDriver::Fluentd),
            ("awslogs", LogDriver::Awslogs),
            ("gelf", LogDriver::Gelf),
            ("splunk", LogDriver::Splunk),
            ("none", LogDriver::None),
        ];
        for (name, expected) in cases {
            let s = format!(r#"{{"Id":"x","HostConfig":{{"LogConfig":{{"Type":"{name}"}}}}}}"#);
            let m = parse_docker_inspect(&s);
            let d = m.get("x").expect("driver case");
            assert_eq!(d.log_driver, Some(expected), "driver={name}");
            assert!(
                !expected.is_readable_via_docker_logs(),
                "{name} must be marked unreadable"
            );
        }
        // Sanity: known-good driver still readable.
        assert!(LogDriver::JsonFile.is_readable_via_docker_logs());
        assert!(LogDriver::Journald.is_readable_via_docker_logs());
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

    #[test]
    fn explain_docker_perm_denied() {
        let s = "Got permission denied while trying to connect to the Docker daemon socket";
        let h = explain_docker_failure(s).expect("should recognize perm denied");
        assert!(h.contains("docker") && h.contains("group"));
    }

    #[test]
    fn explain_docker_daemon_down() {
        let s = "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?";
        let h = explain_docker_failure(s).expect("should recognize daemon down");
        assert!(h.contains("systemctl") || h.contains("DOCKER_HOST"));
    }

    #[test]
    fn explain_docker_unknown_returns_none() {
        assert!(explain_docker_failure("some unrelated stderr").is_none());
    }

    #[test]
    fn problematic_name_flags_underscore_placeholder() {
        let r = problematic_service_name("_").expect("`_` is reserved");
        assert!(r.contains("placeholder"));
    }

    #[test]
    fn problematic_name_flags_reserved_chars() {
        for bad in ["api,db", "svc/v1", "host:port", "x*y", "a~b", "a b", "a\tb"] {
            assert!(
                problematic_service_name(bad).is_some(),
                "should flag {bad:?}"
            );
        }
    }

    #[test]
    fn problematic_name_passes_normal_names() {
        for ok in ["api", "db_2", "svc-prod", "user.api", "abc123", "Web_API"] {
            assert!(
                problematic_service_name(ok).is_none(),
                "should pass {ok:?}: {:?}",
                problematic_service_name(ok)
            );
        }
    }

    #[test]
    fn systemd_filter_blocks_os_internal_names() {
        for blocked in [
            "systemd-resolved",
            "systemd-logind",
            "user@1000",
            "session-3",
            "getty@tty1",
            "dbus",
            "cron",
            "polkit",
            "snapd",
            "fwupd",
            "NetworkManager",
            "ssh",
        ] {
            assert!(
                systemd_name_is_os_internal(blocked),
                "expected {blocked:?} to be filtered as OS-internal"
            );
        }
    }

    #[test]
    fn systemd_filter_passes_user_workloads() {
        for ok in ["nginx", "postgresql", "my-app", "redis", "consul", "vault"] {
            assert!(
                !systemd_name_is_os_internal(ok),
                "expected {ok:?} to pass the filter"
            );
        }
    }

    #[test]
    fn human_bytes_formats_units() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1024), "1.0 KiB");
        assert_eq!(human_bytes(1024 * 1024), "1.0 MiB");
        assert_eq!(human_bytes(1024 * 1024 * 1024), "1.0 GiB");
        assert_eq!(human_bytes(2u64 * 1024 * 1024 * 1024), "2.0 GiB");
    }
}
