//! Discovery orchestrator: runs every probe, merges fragments into a
//! [`Profile`], and persists it.

use anyhow::{Context, Result};
use chrono::Utc;

use super::probes::{
    probe_clock_offset, probe_compose_projects, probe_docker_containers, probe_docker_inventory,
    probe_host_listeners, probe_remote_tooling, probe_systemd_units,
};
use crate::profile::cache::{ensure_profiles_dir, save_profile};
use crate::profile::schema::{Profile, RemoteTooling, ServiceKind};
use crate::ssh::SshTarget;

/// Tunables for a single discovery pass.
#[derive(Debug, Clone, Default)]
pub struct DiscoverOptions {
    /// Skip the systemd probe even if `systemctl` is available.
    pub skip_systemd: bool,
    /// Skip the host listeners probe even if `ss`/`netstat` are available.
    pub skip_host_listeners: bool,
}

/// Run discovery against `target` and write the profile to disk.
pub fn discover(namespace: &str, target: &SshTarget, opts: DiscoverOptions) -> Result<Profile> {
    ensure_profiles_dir().map_err(anyhow::Error::from)?;

    let mut profile = Profile::empty(namespace, &target.host, &Utc::now().to_rfc3339());

    // 1) Probe remote tooling first; subsequent probes don't *need* this but
    //    it's the cheapest probe and it informs degradation messaging.
    let mut tooling = RemoteTooling::default();
    let tprobe = probe_remote_tooling(namespace, target);
    if let Some(t) = tprobe.remote_tooling {
        tooling = t;
    }
    profile.warnings.extend(tprobe.warnings);

    // 2) Docker container inventory (only if docker is present).
    if tooling.docker {
        let r = probe_docker_containers(namespace, target);
        // F2 (v0.1.3): the docker probe escalates a probe-level fatal
        // (e.g. daemon down, every per-container fallback failed) to
        // an `Err` here so setup exits non-zero with a chained hint
        // instead of folding the line into the warnings list. The
        // user-visible warning channel is reserved for actionable,
        // non-fatal noise.
        if let Some(fatal) = r.fatal {
            return Err(anyhow::anyhow!(fatal));
        }
        profile.services.extend(r.services);
        profile.warnings.extend(r.warnings);

        let inv = probe_docker_inventory(namespace, target);
        profile.volumes.extend(inv.volumes);
        profile.networks.extend(inv.networks);
        profile.images.extend(inv.images);
        profile.warnings.extend(inv.warnings);

        // F6 (v0.1.3): compose projects. Best-effort; silent when
        // `docker compose` is not installed (the absence of compose
        // is normal on plain container hosts).
        let cp = probe_compose_projects(namespace, target);
        profile.compose_projects.extend(cp.compose_projects);
        profile.warnings.extend(cp.warnings);
    } else {
        profile
            .warnings
            .push("docker not present on remote; container inventory skipped".into());
    }

    // 3) Host listeners.
    if !opts.skip_host_listeners && (tooling.ss || tooling.netstat) {
        let r = probe_host_listeners(namespace, target);
        for hl in r.host_listeners {
            // Promote each host listener into a synthetic "host service" so
            // the profile carries something useful even on hosts that don't
            // run docker. Container listeners would be redundant here, so we
            // only keep ports that aren't already mapped through a known
            // container port mapping.
            let already_mapped = profile
                .services
                .iter()
                .any(|s| s.ports.iter().any(|p| p.host == hl.port));
            if already_mapped {
                continue;
            }
            profile.services.push(crate::profile::schema::Service {
                name: hl
                    .process
                    .clone()
                    .unwrap_or_else(|| format!("port-{}", hl.port)),
                container_name: hl
                    .process
                    .clone()
                    .unwrap_or_else(|| format!("port-{}", hl.port)),
                compose_service: None,
                container_id: None,
                image: None,
                ports: vec![crate::profile::schema::Port {
                    host: hl.port,
                    container: hl.port,
                    proto: hl.proto,
                }],
                health: None,
                health_status: None,
                log_driver: None,
                log_readable_directly: false,
                mounts: vec![],
                kind: ServiceKind::HostListener,
                depends_on: vec![],
                discovery_incomplete: false,
            });
        }
        profile.warnings.extend(r.warnings);
    }

    // 4) Systemd units (best-effort).
    if !opts.skip_systemd && tooling.systemctl {
        let r = probe_systemd_units(namespace, target);
        // De-duplicate by name (don't overwrite container-derived services).
        let known: std::collections::HashSet<String> =
            profile.services.iter().map(|s| s.name.clone()).collect();
        for s in r.services {
            if !known.contains(&s.name) {
                profile.services.push(s);
            }
        }
        profile.warnings.extend(r.warnings);
    }

    profile.remote_tooling = tooling;

    // 5) Field pitfall §5.3: capture per-host clock offset so
    // operators can spot NTP failures before they cause silent
    // `--since` mis-windows. Best-effort: a probe failure leaves
    // `clock_offset_secs = None` (already the default).
    let (offset, clock_warnings) = probe_clock_offset(namespace, target);
    profile.clock_offset_secs = offset;
    profile.warnings.extend(clock_warnings);

    // Persist (atomic, mode 0600). User-owned sections are merged inside.
    save_profile(&profile).context("saving profile")?;

    // Clear any prior drift marker — we just rebuilt the truth.
    crate::profile::cache::clear_drift_marker(namespace);

    Ok(profile)
}

// Make `probes` reachable for unit tests in the same crate.
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use super::probes::*;
