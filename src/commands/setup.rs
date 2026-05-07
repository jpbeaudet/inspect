//! `inspect setup <ns>` — auto-discover and persist a server profile.
//!
//! Also serves `inspect discover` (alias) and `inspect setup --check-drift`.

use anyhow::Context;

use crate::cli::SetupArgs;
use crate::config::namespace::validate_namespace_name;
use crate::config::resolver;
use crate::discovery::{
    self,
    drift::{format_diff_human, format_diff_json, run_drift_check, DriftStatus},
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
        return drift_only(&resolved.name, &target, &args.format);
    }

    // P13: --retry-failed re-runs discovery and merges *only* services
    // that were previously flagged `discovery_incomplete`. Containers
    // whose previous probe succeeded keep their cached entry, so we
    // don't pay the cost of a full re-discovery just to fix one
    // wedged container.
    if args.retry_failed {
        let prev = load_profile(&resolved.name)?
            .context("--retry-failed: no cached profile found; run `inspect setup` first")?;
        // B1: fail fast on auth before re-probing.
        precheck_or_bail(&resolved.name, &target)?;
        let opts = discovery::DiscoverOptions {
            skip_systemd: args.skip_systemd,
            skip_host_listeners: args.skip_host_listeners,
        };
        let fresh = discovery::discover(&resolved.name, &target, opts)
            .with_context(|| format!("setup --retry-failed '{}'", resolved.name))?;
        let merged = merge_retry(&prev, &fresh);
        if args.format.is_json() {
            print_json(&merged, "retry-failed", args.format.select_spec())?;
        } else {
            print_human(&merged, "retry-failed");
        }
        return Ok(ExitKind::Success);
    }

    // Honor TTL unless --force.
    if !args.force {
        if let Ok(Some(prev)) = load_profile(&resolved.name) {
            if !is_stale(&prev) {
                return print_existing(&prev, &args.format);
            }
        }
    }

    let opts = discovery::DiscoverOptions {
        skip_systemd: args.skip_systemd,
        skip_host_listeners: args.skip_host_listeners,
    };

    // B1: fail fast on auth instead of producing a half-empty profile
    // full of "Permission denied" warnings.
    precheck_or_bail(&resolved.name, &target)?;

    let profile = discovery::discover(&resolved.name, &target, opts)
        .with_context(|| format!("setup '{}'", resolved.name))?;

    if args.format.is_json() {
        print_json(&profile, "discovered", args.format.select_spec())?;
    } else {
        print_human(&profile, "discovered");
    }
    Ok(ExitKind::Success)
}

fn drift_only(
    namespace: &str,
    target: &SshTarget,
    format: &crate::format::FormatArgs,
) -> anyhow::Result<ExitKind> {
    // Skip the SSH precheck when there's no cached profile: drift_only
    // will short-circuit to NoCache without ever touching the network,
    // and we shouldn't burn an ssh round-trip (or fail tests on hosts
    // that aren't reachable) for a path that is fully local.
    if load_profile(namespace)?.is_some() {
        precheck_or_bail(namespace, target)?;
    }
    let status = run_drift_check(namespace, target)?;
    let label = match &status {
        DriftStatus::NoCache => "no-cache",
        DriftStatus::ProbeFailed => "probe-failed",
        DriftStatus::Fresh => "fresh",
        DriftStatus::Drifted { .. } => "drifted",
    };
    if format.is_json() {
        // F19 (v0.1.3): build the envelope as a `serde_json::Value` so
        // `--select` can be applied through the shared
        // `print_json_value` chokepoint. `format_diff_json` returns an
        // already-serialized JSON string, which we re-parse so it can
        // ride inside the structured envelope.
        let env = match &status {
            DriftStatus::Drifted {
                current,
                cached,
                diff,
            } => {
                let diff_json: serde_json::Value = serde_json::from_str(&format_diff_json(diff))
                    .map_err(|e| anyhow::anyhow!("internal: drift diff json reparse: {e}"))?;
                serde_json::json!({
                    "schema_version": 1,
                    "namespace": namespace,
                    "drift": label,
                    "current_fingerprint": current,
                    "cached_fingerprint": cached,
                    "diff": diff_json,
                })
            }
            _ => serde_json::json!({
                "schema_version": 1,
                "namespace": namespace,
                "drift": label,
            }),
        };
        crate::verbs::output::print_json_value(&env, format.select_spec())?;
    } else {
        println!("SUMMARY: drift check for '{namespace}': {label}");
        if let DriftStatus::Drifted { diff, .. } = &status {
            println!("DATA:");
            println!("{}", format_diff_human(diff));
            println!("NEXT:    inspect setup {namespace} --force");
        }
    }
    Ok(ExitKind::Success)
}

fn print_existing(p: &Profile, format: &crate::format::FormatArgs) -> anyhow::Result<ExitKind> {
    if format.is_json() {
        print_json(p, "cache-hit", format.select_spec())?;
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
    let incomplete: Vec<&str> = p
        .services
        .iter()
        .filter(|s| s.discovery_incomplete)
        .map(|s| s.name.as_str())
        .collect();
    if !incomplete.is_empty() {
        println!(
            "INCOMPLETE: {} service(s) flagged discovery_incomplete (per-container `docker inspect` failed): {}",
            incomplete.len(),
            incomplete.join(", "),
        );
        println!(
            "HINT:    inspect setup {} --retry-failed   # re-probe just the flagged services",
            p.namespace
        );
    }
    println!(
        "NEXT:    inspect profile {}    inspect setup {} --check-drift",
        p.namespace, p.namespace
    );
}

fn print_json(
    p: &Profile,
    status: &str,
    select: Option<(&str, bool, bool)>,
) -> anyhow::Result<ExitKind> {
    // We don't emit the full profile here — that's what `inspect profile`
    // is for. We emit a stable summary envelope.
    //
    // F19 (v0.1.3): build the envelope as a `serde_json::Value` and
    // route through `print_json_value` so `--select` works the same way
    // it does on every other JSON-emitting verb. The pre-fix form
    // hand-rolled JSON via `format!`/`println!` and bypassed both the
    // transcript chokepoint and the filter.
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
    let env = serde_json::json!({
        "schema_version": 1,
        "namespace": p.namespace,
        "status": status,
        "host": p.host,
        "discovered_at": p.discovered_at,
        "counts": {
            "containers": containers,
            "host_listeners": host_lst,
            "systemd_units": units,
            "volumes": p.volumes.len(),
            "networks": p.networks.len(),
            "images": p.images.len(),
        },
        "warnings": p.warnings,
    });
    crate::verbs::output::print_json_value(&env, select)
}

fn b(x: bool) -> char {
    if x {
        'y'
    } else {
        'n'
    }
}

/// P13: build a merged profile for `--retry-failed`. We start from
/// `prev` and, for every service that was flagged
/// `discovery_incomplete`, swap in the corresponding entry from
/// `fresh` if (and only if) the fresh probe succeeded for that
/// container. Services not flagged incomplete in `prev` are kept
/// verbatim. Warnings from `fresh` are appended.
fn merge_retry(prev: &Profile, fresh: &Profile) -> Profile {
    let mut merged = prev.clone();
    let fresh_by_id: std::collections::HashMap<&str, &crate::profile::schema::Service> = fresh
        .services
        .iter()
        .filter_map(|s| s.container_id.as_deref().map(|id| (id, s)))
        .collect();
    let fresh_by_name: std::collections::HashMap<&str, &crate::profile::schema::Service> = fresh
        .services
        .iter()
        .map(|s| (s.name.as_str(), s))
        .collect();
    for svc in &mut merged.services {
        if !svc.discovery_incomplete {
            continue;
        }
        let candidate = svc
            .container_id
            .as_deref()
            .and_then(|id| fresh_by_id.get(id).copied())
            .or_else(|| fresh_by_name.get(svc.name.as_str()).copied());
        if let Some(repl) = candidate {
            if !repl.discovery_incomplete {
                *svc = repl.clone();
            }
        }
    }
    merged.warnings.extend(fresh.warnings.iter().cloned());
    merged.discovered_at = fresh.discovered_at.clone();
    merged
}

/// B1 (v0.1.2): run [`discovery::ssh_precheck`] and translate any
/// failure into a fatal `anyhow::Error` carrying a chained,
/// human-readable hint. The error message is shaped so that
/// `error::topic_for_message()` will append `see: inspect help ssh`.
fn precheck_or_bail(namespace: &str, target: &SshTarget) -> anyhow::Result<()> {
    use crate::discovery::ssh_precheck::{
        auth_failed_hint, host_key_changed_hint, run as run_precheck, unreachable_hint,
        PrecheckOutcome,
    };
    match run_precheck(namespace, target) {
        PrecheckOutcome::Ok => Ok(()),
        PrecheckOutcome::AuthFailed { .. } => {
            Err(anyhow::anyhow!(auth_failed_hint(namespace, target)))
        }
        PrecheckOutcome::HostKeyChanged { .. } => {
            Err(anyhow::anyhow!(host_key_changed_hint(namespace, target)))
        }
        PrecheckOutcome::Unreachable { .. } => {
            Err(anyhow::anyhow!(unreachable_hint(namespace, target)))
        }
        PrecheckOutcome::Other { stderr, exit_code } => Err(anyhow::anyhow!(
            "ssh precheck for '{ns}' failed (exit {exit_code}): {stderr}\n  \
             → check: ssh {host} (manual connection)\n  \
             → then retry: inspect setup {ns}",
            ns = namespace,
            host = target.host,
            exit_code = exit_code,
            stderr = stderr.trim()
        )),
    }
}
