//! `inspect setup <ns>` — auto-discover and persist a server profile.
//!
//! Also serves `inspect discover` (alias) and `inspect setup --check-drift`.

use anyhow::Context;

use crate::cli::SetupArgs;
use crate::commands::list::json_string;
use crate::config::namespace::validate_namespace_name;
use crate::config::resolver;
use crate::discovery::{
    self,
    drift::{run_drift_check, DriftStatus},
};
use crate::error::ExitKind;
use crate::profile::cache::{is_stale, load_profile};
use crate::profile::schema::{Profile, ServiceKind};
use crate::ssh::SshTarget;

pub fn run(args: SetupArgs) -> anyhow::Result<ExitKind> {
    validate_namespace_name(&args.namespace)?;
    let resolved = resolver::resolve(&args.namespace)?;
    resolved.config.validate(&resolved.name)?;
    let target = SshTarget::from_resolved(&resolved)?;

    if args.check_drift {
        return drift_only(&resolved.name, &target, args.format.is_json());
    }

    // Honor TTL unless --force.
    if !args.force {
        if let Ok(Some(prev)) = load_profile(&resolved.name) {
            if !is_stale(&prev) {
                return print_existing(&prev, args.format.is_json());
            }
        }
    }

    let opts = discovery::DiscoverOptions {
        skip_systemd: args.skip_systemd,
        skip_host_listeners: args.skip_host_listeners,
    };

    let profile = discovery::discover(&resolved.name, &target, opts)
        .with_context(|| format!("setup '{}'", resolved.name))?;

    if args.format.is_json() {
        print_json(&profile, "discovered");
    } else {
        print_human(&profile, "discovered");
    }
    Ok(ExitKind::Success)
}

fn drift_only(namespace: &str, target: &SshTarget, as_json: bool) -> anyhow::Result<ExitKind> {
    let status = run_drift_check(namespace, target)?;
    let label = match &status {
        DriftStatus::NoCache => "no-cache",
        DriftStatus::ProbeFailed => "probe-failed",
        DriftStatus::Fresh => "fresh",
        DriftStatus::Drifted { .. } => "drifted",
    };
    if as_json {
        let body = match &status {
            DriftStatus::Drifted { current, cached } => format!(
                "{{\"schema_version\":1,\"namespace\":{ns},\"drift\":{lbl},\"current_fingerprint\":{c},\"cached_fingerprint\":{p}}}",
                ns = json_string(namespace),
                lbl = json_string(label),
                c = json_string(current),
                p = json_string(cached),
            ),
            _ => format!(
                "{{\"schema_version\":1,\"namespace\":{ns},\"drift\":{lbl}}}",
                ns = json_string(namespace),
                lbl = json_string(label),
            ),
        };
        println!("{body}");
    } else {
        println!("SUMMARY: drift check for '{namespace}': {label}");
        if let DriftStatus::Drifted { current, cached } = &status {
            println!("DATA:");
            println!("  cached:  {cached}");
            println!("  current: {current}");
            println!("NEXT:    inspect setup {namespace} --force");
        }
    }
    Ok(ExitKind::Success)
}

fn print_existing(p: &Profile, as_json: bool) -> anyhow::Result<ExitKind> {
    if as_json {
        print_json(p, "cache-hit");
    } else {
        print_human(p, "cache-hit");
    }
    Ok(ExitKind::Success)
}

fn print_human(p: &Profile, status: &str) {
    let containers = p
        .services
        .iter()
        .filter(|s| matches!(s.kind, ServiceKind::Container))
        .count();
    let host_lst = p
        .services
        .iter()
        .filter(|s| matches!(s.kind, ServiceKind::HostListener))
        .count();
    let units = p
        .services
        .iter()
        .filter(|s| matches!(s.kind, ServiceKind::Systemd))
        .count();

    println!(
        "SUMMARY: profile for '{}' ({status}): {} container(s), {} host listener(s), {} systemd unit(s)",
        p.namespace, containers, host_lst, units
    );
    println!("DATA:");
    println!("  host:           {}", p.host);
    println!("  discovered_at:  {}", p.discovered_at);
    println!(
        "  remote_tooling: rg={} jq={} sed={} grep={} ss={} netstat={} systemctl={} docker={} journalctl={}",
        b(p.remote_tooling.rg),
        b(p.remote_tooling.jq),
        b(p.remote_tooling.sed),
        b(p.remote_tooling.grep),
        b(p.remote_tooling.ss),
        b(p.remote_tooling.netstat),
        b(p.remote_tooling.systemctl),
        b(p.remote_tooling.docker),
        b(p.remote_tooling.journalctl),
    );
    println!("  volumes:        {}", p.volumes.len());
    println!("  networks:       {}", p.networks.len());
    println!("  images:         {}", p.images.len());
    if !p.warnings.is_empty() {
        println!("WARNINGS:");
        for w in &p.warnings {
            println!("  - {w}");
        }
    }
    println!("NEXT:    inspect profile {}    inspect setup {} --check-drift", p.namespace, p.namespace);
}

fn print_json(p: &Profile, status: &str) {
    // We don't emit the full profile here — that's what `inspect profile`
    // is for. We emit a stable summary envelope.
    let containers = p
        .services
        .iter()
        .filter(|s| matches!(s.kind, ServiceKind::Container))
        .count();
    let host_lst = p
        .services
        .iter()
        .filter(|s| matches!(s.kind, ServiceKind::HostListener))
        .count();
    let units = p
        .services
        .iter()
        .filter(|s| matches!(s.kind, ServiceKind::Systemd))
        .count();
    println!(
        "{{\"schema_version\":1,\"namespace\":{ns},\"status\":{st},\"host\":{h},\"discovered_at\":{ts},\
         \"counts\":{{\"containers\":{c},\"host_listeners\":{hl},\"systemd_units\":{u},\"volumes\":{v},\"networks\":{n},\"images\":{i}}},\
         \"warnings\":{w}}}",
        ns = json_string(&p.namespace),
        st = json_string(status),
        h = json_string(&p.host),
        ts = json_string(&p.discovered_at),
        c = containers,
        hl = host_lst,
        u = units,
        v = p.volumes.len(),
        n = p.networks.len(),
        i = p.images.len(),
        w = serde_json::to_string(&p.warnings).unwrap_or_else(|_| "[]".into()),
    );
}

fn b(x: bool) -> char {
    if x {
        'y'
    } else {
        'n'
    }
}
