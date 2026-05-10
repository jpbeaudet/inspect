//! — cache orchestrator for read verbs.
//!
//! Single entry point [`get_runtime`] that the read verbs (`status`,
//! `health`, `why`, …) call to get a [`RuntimeSnapshot`] tagged with
//! [`SourceInfo`] provenance. The verb itself never touches the cache
//! files, never decides "live vs cached", and never builds the remote
//! command — that's all here.
//!
//! Decision tree:
//!
//! ```text
//! if force_refresh → fetch_live()
//!     on success → SourceMode::Live, save snapshot
//!     on failure → fall through to cached snapshot, mark Stale w/ reason
//! else
//!   if cached snapshot exists and !is_runtime_stale → SourceMode::Cached
//!   else (cold or expired)
//!     fetch_live()
//!       on success → SourceMode::Live, save snapshot
//!       on failure → if cached exists, serve it as Stale; else propagate
//! ```
//!
//! Failure handling is graceful: a failed live refresh on top of an
//! existing snapshot serves the snapshot with `mode = Stale` and a
//! `reason` describing the refresh failure. The verb emits a clear
//! `SOURCE: cached … — stale` line plus a chained hint pointing at
//! `inspect connectivity <ns>` (added by the verb after consulting
//! [`SourceInfo::stale`]).
//!
//! On a stone-cold cache with a failed refresh the orchestrator
//! returns the original error so the verb can present its existing
//! "no profile" / "namespace not reachable" diagnostic — degraded
//! mode does not invent data.

use anyhow::{anyhow, Result};

use crate::profile::runtime::{
    self, inventory_age, is_runtime_stale, load_runtime, RuntimeSnapshot, ServiceRuntime,
    SourceInfo, SourceMode,
};
use crate::profile::schema::{HealthStatus, ServiceKind};
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::NsCtx;
use crate::verbs::runtime::RemoteRunner;

/// Caller-controlled options.
#[derive(Debug, Clone, Copy, Default)]
pub struct GetOpts {
    /// `--refresh` / `--live` — bypass cache, force a live fetch.
    pub force_refresh: bool,
}

/// Get a runtime snapshot for one namespace, returning the snapshot
/// alongside the [`SourceInfo`] the verb must surface to the operator.
///
/// On a completely cold cache **and** a refresh failure, returns the
/// underlying error rather than fabricating an empty snapshot — the
/// verb already has a "no profile" / "host unreachable" diagnostic
/// that's better than a misleading empty inventory.
pub fn get_runtime(
    runner: &dyn RemoteRunner,
    ns: &NsCtx,
    opts: GetOpts,
) -> Result<(RuntimeSnapshot, SourceInfo)> {
    // Fast path: check the cache outside the lock. If the cache is
    // fresh and the caller didn't force-refresh, return immediately
    // without touching `flock` — keeps the read hot path
    // contention-free.
    let cached = load_runtime(&ns.namespace);
    let must_refresh = opts.force_refresh
        || match &cached {
            None => true,
            Some(snap) => is_runtime_stale(snap),
        };
    if !must_refresh {
        let snap = cached.expect("must_refresh = false ⇒ cached.is_some()");
        let info = build_source_info(SourceMode::Cached, &snap, &ns.namespace, None);
        return Ok((snap, info));
    }

    // Slow path: serialize concurrent refreshers under an advisory
    // file lock so two parallel `inspect status arte` invocations
    // don't double-fetch. The second waiter re-checks the cache
    // post-lock (double-checked locking) and may skip its own fetch
    // entirely if the first refresher already populated it.
    runtime::with_runtime_lock(&ns.namespace, || {
        // Re-check inside the lock: another process may have just
        // refreshed.
        let cached = load_runtime(&ns.namespace);
        let still_must_refresh = opts.force_refresh
            || match &cached {
                None => true,
                Some(snap) => is_runtime_stale(snap),
            };
        if !still_must_refresh {
            let snap = cached.expect("just checked Some");
            let info = build_source_info(SourceMode::Cached, &snap, &ns.namespace, None);
            return Ok((snap, info));
        }

        match fetch_live(runner, ns) {
            Ok(snap) => {
                // `save_refreshed` increments `refresh_count` on top of
                // the prior on-disk value. Save failure is best-effort.
                let saved = match runtime::save_refreshed(snap.clone()) {
                    Ok(_) => {
                        // Re-load to pick up the bumped refresh_count
                        // (cheap: one small JSON read on the local FS).
                        load_runtime(&ns.namespace).unwrap_or(snap)
                    }
                    Err(_) => snap,
                };
                let info = build_source_info(SourceMode::Live, &saved, &ns.namespace, None);
                Ok((saved, info))
            }
            Err(e) => {
                if let Some(snap) = cached {
                    let reason = short_error(&e);
                    let info =
                        build_source_info(SourceMode::Stale, &snap, &ns.namespace, Some(reason));
                    Ok((snap, info))
                } else {
                    Err(e)
                }
            }
        }
    })
}

/// Render the human SOURCE: line directly to stdout, but **only**
/// for formats that retain decoration (Human/Table/Md). Machine
/// formats (json/csv/tsv/yaml/raw/format) carry the same data in
/// `meta.source` and must not have a free-floating leading line that
/// would corrupt their grammar (a CSV reader doesn't expect a
/// `SOURCE: live` row before the header). Verbs call this just before
/// `format::render::render_doc`.
pub fn print_source_line(info: &SourceInfo, fmt: &crate::format::OutputFormat) {
    if fmt.shows_envelope() {
        crate::tee_println!("{}", info.human_line());
    }
}

// -----------------------------------------------------------------------------
// Internals
// -----------------------------------------------------------------------------

fn build_source_info(
    mode: SourceMode,
    snap: &RuntimeSnapshot,
    namespace: &str,
    reason: Option<String>,
) -> SourceInfo {
    let runtime_age_s = snap.age().map(|d| d.as_secs());
    let inventory_age_s = inventory_age(namespace).map(|d| d.as_secs());
    let stale = matches!(mode, SourceMode::Stale);
    SourceInfo {
        mode,
        runtime_age_s,
        inventory_age_s,
        stale,
        reason,
    }
}

fn short_error(e: &anyhow::Error) -> String {
    // Take the first line of the chain; SourceInfo.reason is rendered
    // inside parens on the human line so a multi-line cause spam would
    // wreck the layout.
    e.to_string()
        .lines()
        .next()
        .unwrap_or("refresh failed")
        .to_string()
}

/// Live-fetch the runtime tier. Strategy: one batched `docker ps` to
/// learn the running set, one batched `docker inspect --format` for
/// health + restart count over the union of running and inventory
/// containers. Both calls go through the [`RemoteRunner`] so tests
/// drive them with `INSPECT_MOCK_REMOTE_FILE`.
fn fetch_live(runner: &dyn RemoteRunner, ns: &NsCtx) -> Result<RuntimeSnapshot> {
    // Container-name set from inventory: containers we want runtime
    // facts for even if `docker ps` doesn't list them (so a stopped
    // service still appears as `running: false` rather than missing).
    let inventory_containers: Vec<(String, ServiceKind)> = ns
        .profile
        .as_ref()
        .map(|p| {
            p.services
                .iter()
                .filter(|s| matches!(s.kind, ServiceKind::Container))
                .map(|s| (s.container_name.clone(), s.kind))
                .collect()
        })
        .unwrap_or_default();

    // 1. docker ps — running set.
    let ps_out = runner
        .run(
            &ns.namespace,
            &ns.target,
            "docker ps --format '{{.Names}}'",
            RunOpts::with_timeout(15),
        )
        .map_err(|e| anyhow!("runtime refresh failed (docker ps): {e}"))?;
    let running_set: std::collections::HashSet<String> = if ps_out.exit_code == 0 {
        ps_out.stdout.lines().map(|s| s.to_string()).collect()
    } else {
        // Non-zero exit — refresh has failed; surface a clean reason.
        return Err(anyhow!(
            "runtime refresh failed (docker ps exit {}): {}",
            ps_out.exit_code,
            ps_out.stderr.trim()
        ));
    };

    // 2. docker inspect — health + restart count for the union of
    // (running, inventory). One batched call; output is a TSV-ish
    // line-per-container so the parser is trivial.
    //
    // Format:
    //   <name>\t<health>\t<restart_count>
    //
    // Health values from docker: starting | healthy | unhealthy | none
    // (last is the no-healthcheck case).
    let mut union: std::collections::BTreeSet<String> = inventory_containers
        .iter()
        .map(|(name, _)| name.clone())
        .collect();
    union.extend(running_set.iter().cloned());

    let mut by_name: std::collections::HashMap<String, (Option<HealthStatus>, u32)> =
        std::collections::HashMap::new();
    if !union.is_empty() {
        let names_quoted = union
            .iter()
            .map(|n| crate::verbs::quote::shquote(n))
            .collect::<Vec<_>>()
            .join(" ");
        let cmd = format!(
            "docker inspect --format '{{{{.Name}}}}\t{{{{if .State.Health}}}}{{{{.State.Health.Status}}}}{{{{else}}}}none{{{{end}}}}\t{{{{.RestartCount}}}}' {names_quoted}"
        );
        let out = runner
            .run(&ns.namespace, &ns.target, &cmd, RunOpts::with_timeout(15))
            .map_err(|e| anyhow!("runtime refresh failed (docker inspect): {e}"))?;
        // Even on partial failure (e.g. one container removed
        // between ps and inspect), parse whatever lines did come back.
        for line in out.stdout.lines() {
            if line.is_empty() {
                continue;
            }
            let mut parts = line.splitn(3, '\t');
            let raw_name = parts.next().unwrap_or("").trim_start_matches('/').trim();
            let health = parts.next().unwrap_or("none").trim();
            let restarts = parts
                .next()
                .unwrap_or("0")
                .trim()
                .parse::<u32>()
                .unwrap_or(0);
            if raw_name.is_empty() {
                continue;
            }
            let hs = match health {
                "healthy" => Some(HealthStatus::Ok),
                "unhealthy" => Some(HealthStatus::Unhealthy),
                "starting" => Some(HealthStatus::Starting),
                "none" | "" => None,
                _ => Some(HealthStatus::Unknown),
            };
            by_name.insert(raw_name.to_string(), (hs, restarts));
        }
    }

    // 3. Assemble per-service runtime states.
    let mut services: Vec<ServiceRuntime> = Vec::new();
    for name in &union {
        let (health_status, restart_count) = by_name.get(name).cloned().unwrap_or((None, 0));
        services.push(ServiceRuntime {
            container_name: name.clone(),
            running: running_set.contains(name),
            health_status,
            restart_count,
        });
    }
    // Stable order (matches BTreeSet iteration above), but force
    // deterministic output regardless of HashMap insertion order.
    services.sort_by(|a, b| a.container_name.cmp(&b.container_name));

    Ok(RuntimeSnapshot::new(&ns.namespace, services))
}

/// Convenience: explicit cache invalidation for write verbs. Wraps
/// [`runtime::clear_runtime`] so callers don't have to reach across
/// the `profile::runtime` boundary.
pub fn invalidate(namespace: &str) {
    runtime::clear_runtime(namespace);
}

/// Collapse per-namespace [`SourceInfo`]s into a single
/// "best representative" entry for the SOURCE: prose line and
/// `meta.source` JSON field.
///
/// Rules:
/// - any `Stale` → aggregate is `Stale` (operator must see degradation);
/// - else all `Live` → `Live`;
/// - else → `Cached`.
///
/// Ages are reduced to the **maximum** observed (worst case wins);
/// the first available `reason` is surfaced.
pub fn aggregate_sources(sources: &[SourceInfo]) -> SourceInfo {
    if sources.is_empty() {
        return SourceInfo {
            mode: SourceMode::Live,
            runtime_age_s: None,
            inventory_age_s: None,
            stale: false,
            reason: None,
        };
    }
    let any_stale = sources.iter().any(|s| matches!(s.mode, SourceMode::Stale));
    let all_live = sources.iter().all(|s| matches!(s.mode, SourceMode::Live));
    let mode = if any_stale {
        SourceMode::Stale
    } else if all_live {
        SourceMode::Live
    } else {
        SourceMode::Cached
    };
    SourceInfo {
        mode,
        runtime_age_s: sources.iter().filter_map(|s| s.runtime_age_s).max(),
        inventory_age_s: sources.iter().filter_map(|s| s.inventory_age_s).max(),
        stale: any_stale,
        reason: sources.iter().find_map(|s| s.reason.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::runtime::{runtime_ttl, RUNTIME_TTL_ENV};

    #[test]
    fn short_error_strips_chain() {
        let e = anyhow!("first line\ncause: details");
        assert_eq!(short_error(&e), "first line");
    }

    #[test]
    fn ttl_zero_makes_every_cached_snapshot_stale() {
        // Sanity guard: if someone changes runtime_ttl semantics, this
        // test calls it out before tests in the integration suite do.
        std::env::set_var(RUNTIME_TTL_ENV, "0");
        assert_eq!(runtime_ttl(), None);
        let snap = RuntimeSnapshot::new("arte", vec![]);
        assert!(is_runtime_stale(&snap));
        std::env::remove_var(RUNTIME_TTL_ENV);
    }

    #[test]
    fn duration_max_is_never_stale() {
        std::env::set_var(RUNTIME_TTL_ENV, "never");
        assert_eq!(runtime_ttl(), Some(std::time::Duration::MAX));
        let mut snap = RuntimeSnapshot::new("arte", vec![]);
        snap.fetched_at_unix_secs = 0; // ancient
        assert!(!is_runtime_stale(&snap));
        std::env::remove_var(RUNTIME_TTL_ENV);
    }
}
