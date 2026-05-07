//! F6 (v0.1.3): `inspect compose ls <ns>` — list compose projects.
//!
//! Reads from the cached profile (populated by
//! `discovery::probes::probe_compose_projects` at `inspect setup`
//! time). With `--refresh`, re-probes the host live via `docker
//! compose ls --all --format json` so projects deployed
//! out-of-band become visible without waiting for the next
//! `inspect setup`.

use anyhow::Result;
use serde_json::{json, Value};

use crate::cli::ComposeLsArgs;
use crate::error::ExitKind;
use crate::profile::cache::load_profile;
use crate::profile::schema::ComposeProject;
use crate::selector::resolve::chosen_namespaces_for;
use crate::ssh::exec::RunOpts;
use crate::verbs::output::{NextStep, OutputDoc};
use crate::verbs::runtime::{current_runner, resolve_target, RemoteRunner};

pub fn run(args: ComposeLsArgs) -> Result<ExitKind> {
    let fmt = args.format.resolve()?;
    let namespaces = match chosen_namespaces_for(&args.selector) {
        Ok(ns) if !ns.is_empty() => ns,
        Ok(_) => {
            crate::error::emit(format!(
                "selector '{}' did not match any configured namespace",
                args.selector
            ));
            return Ok(ExitKind::Error);
        }
        Err(e) => {
            crate::error::emit(format!("invalid selector '{}': {e}", args.selector));
            return Ok(ExitKind::Error);
        }
    };

    let runner: Option<Box<dyn RemoteRunner>> = if args.refresh {
        Some(current_runner())
    } else {
        None
    };

    let mut data_lines: Vec<String> = Vec::new();
    let mut json_projects: Vec<Value> = Vec::new();
    let mut total = 0usize;
    let mut warnings: Vec<String> = Vec::new();

    for ns in &namespaces {
        let projects = if let Some(r) = runner.as_ref() {
            // Live re-probe via docker compose ls.
            match probe_live(r.as_ref(), ns) {
                Ok(p) => p,
                Err(e) => {
                    warnings.push(format!(
                        "{ns}: live refresh failed ({e}); falling back to cached profile"
                    ));
                    cached_projects(ns)
                }
            }
        } else {
            cached_projects(ns)
        };

        if projects.is_empty() {
            data_lines.push(format!("{ns}: (no compose projects)"));
            continue;
        }

        for p in &projects {
            total += 1;
            data_lines.push(format!(
                "{ns}/{name:<24} {running}/{total} running  {wd}",
                name = p.name,
                running = p.running_count,
                total = p.service_count,
                wd = p.working_dir,
            ));
            json_projects.push(json!({
                "namespace": ns,
                "name": p.name,
                "status": p.status,
                "compose_file": p.compose_file,
                "working_dir": p.working_dir,
                "service_count": p.service_count,
                "running_count": p.running_count,
            }));
        }
    }

    let summary = if namespaces.len() == 1 {
        format!("{total} compose project(s) on {ns}", ns = namespaces[0])
    } else {
        format!(
            "{total} compose project(s) across {n} namespace(s)",
            n = namespaces.len()
        )
    };

    let mut doc = OutputDoc::new(
        summary,
        json!({
            "compose_projects": json_projects,
            "namespaces": namespaces,
        }),
    )
    .with_meta("selector", args.selector.clone())
    .with_quiet(args.format.quiet);

    if total == 0 {
        // Chained next: when nothing is cached, point operator at
        // setup (cold cache) or at compose itself (host genuinely
        // runs no compose projects). Both are concrete commands.
        doc.push_next(NextStep::new(
            format!("inspect setup {}", namespaces[0]),
            "re-run discovery if a project was just deployed out-of-band",
        ));
        if !args.refresh {
            doc.push_next(NextStep::new(
                format!("inspect compose ls {} --refresh", args.selector),
                "skip the cache and probe the host live",
            ));
        }
    } else {
        doc.push_next(NextStep::new(
            format!("inspect compose ps {}/<project>", namespaces[0]),
            "show per-service status for one project",
        ));
    }

    for w in &warnings {
        crate::tee_eprintln!("warning: {w}");
    }

    crate::format::render::render_doc(&doc, &fmt, &data_lines, args.format.select_spec())
}

/// Read compose projects from the namespace's cached profile.
/// Returns an empty vector when the profile is missing — a more
/// detailed "run setup first" hint is surfaced via the doc's
/// `next` chain so we don't double-print.
fn cached_projects(namespace: &str) -> Vec<ComposeProject> {
    load_profile(namespace)
        .ok()
        .flatten()
        .map(|p| p.compose_projects)
        .unwrap_or_default()
}

/// Live re-probe via `docker compose ls --all --format json`. Used
/// when `--refresh` is passed. Errors propagate so the caller can
/// fall back to the cache and emit a degraded-mode warning.
fn probe_live(runner: &dyn RemoteRunner, namespace: &str) -> Result<Vec<ComposeProject>> {
    let (_resolved, target) = resolve_target(namespace)?;
    let out = runner.run(
        namespace,
        &target,
        "docker compose ls --all --format json 2>/dev/null",
        RunOpts::with_timeout(15),
    )?;
    if !out.ok() {
        anyhow::bail!(
            "docker compose ls exited {}: {}",
            out.exit_code,
            out.stderr.trim()
        );
    }
    Ok(ComposeProject::parse_ls_json(out.stdout.trim()))
}
