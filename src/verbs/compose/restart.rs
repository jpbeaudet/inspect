//! `Inspect compose restart <ns>/<project>/<service>`
//! — audited single-service restart inside a compose project.
//!
//! Audit shape: `verb=compose.restart`, with three compose-specific tag
//! groups stamped into the entry's `args` field for `audit grep`
//! discoverability:
//!
//!   `[project=<name>]` `[service=<name>]` `[compose_file_hash=<sha256-12>]`
//!
//! `compose_file_hash` is the SHA-256 prefix of the project's
//! compose file as fetched at audit time. A post-mortem can verify
//! the file did not change between the audit and a re-run by
//! re-hashing the file and comparing prefixes — without storing
//! the file body.
//!
//! Revert: restart has no clean inverse, so `revert.kind =
//! unsupported` (mirrors the existing `inspect restart` write
//! verb).

use std::time::Instant;

use anyhow::Result;
use serde_json::json;

use crate::cli::ComposeRestartArgs;
use crate::error::ExitKind;
use crate::profile::schema::ComposeProject;
use crate::safety::gate::ConfirmResult;
use crate::safety::{AuditEntry, AuditStore, Confirm, Revert, SafetyGate};
use crate::ssh::exec::RunOpts;
use crate::ssh::options::SshTarget;
use crate::verbs::output::{NextStep, OutputDoc};
use crate::verbs::quote::shquote;
use crate::verbs::runtime::{current_runner, resolve_target, RemoteRunner};

use super::resolve::{project_in_profile, Parsed};

pub fn run(args: ComposeRestartArgs) -> Result<ExitKind> {
    let fmt = args.format.resolve()?;
    let parsed = match Parsed::parse(&args.selector) {
        Ok(p) => p,
        Err(e) => {
            crate::error::emit(format!("{e}"));
            return Ok(ExitKind::Error);
        }
    };
    let project_name = match parsed.project.as_deref() {
        Some(p) => p,
        None => {
            crate::error::emit(format!(
                "selector '{}' is missing the project portion — \
                 expected '<ns>/<project>[/<service>]'",
                args.selector
            ));
            return Ok(ExitKind::Error);
        }
    };
    // Defensive default: a project-only selector requires `--all`.
    // The spec calls this out explicitly so accidentally restarting
    // every service in a project takes a *deliberate* second flag.
    if parsed.service.is_none() && !args.all {
        crate::error::emit(format!(
            "selector '{sel}' targets the whole project — pass --all to confirm restarting \
             every service, or narrow to '{sel}/<service>'.\n\
             hint: `inspect compose ps {sel}` lists the services in this project.",
            sel = args.selector
        ));
        return Ok(ExitKind::Error);
    }

    let (_profile, project) = match project_in_profile(&parsed.namespace, project_name) {
        Ok(t) => t,
        Err(e) => {
            crate::error::emit(format!("{e}"));
            return Ok(ExitKind::NoMatches);
        }
    };

    let runner = current_runner();
    let (_resolved, target) = resolve_target(&parsed.namespace)?;

    // Compute the per-project compose-file hash once. Used in the
    // audit entry's args tags so a post-mortem can verify the file
    // body didn't change between the audit and a re-run. Best-effort:
    // a fetch failure leaves the hash empty rather than blocking the
    // whole verb.
    let compose_hash =
        compose_file_sha_short(runner.as_ref(), &parsed.namespace, &target, &project);

    // Resolve the target service set. With a service portion in the
    // selector, exactly one entry; with --all, fan out.
    let services = if let Some(svc) = parsed.service.as_deref() {
        vec![svc.to_string()]
    } else {
        match list_project_services(runner.as_ref(), &parsed.namespace, &target, &project) {
            Ok(v) if !v.is_empty() => v,
            Ok(_) => {
                crate::error::emit(format!(
                    "compose project '{}/{}' reports zero services — nothing to restart.",
                    parsed.namespace, project.name
                ));
                return Ok(ExitKind::NoMatches);
            }
            Err(e) => {
                crate::error::emit(format!(
                    "could not list services for {}/{}: {e}",
                    parsed.namespace, project.name
                ));
                return Ok(ExitKind::Error);
            }
        }
    };

    let gate = SafetyGate::new(args.apply, args.yes, args.yes_all);
    if !gate.should_apply() {
        // Dry-run preview — same shape as `inspect restart`'s.
        let mut data_lines: Vec<String> = Vec::new();
        for svc in &services {
            data_lines.push(format!(
                "{ns}/{p}/{svc}",
                ns = parsed.namespace,
                p = project.name
            ));
        }
        let doc = OutputDoc::new(
            format!(
                "DRY RUN. Would restart {n} service(s) in {ns}/{p}",
                n = services.len(),
                ns = parsed.namespace,
                p = project.name
            ),
            json!({
                "namespace": parsed.namespace,
                "project": project.name,
                "services": services,
                "would_apply": false,
            }),
        )
        .with_meta("selector", args.selector.clone())
        .with_quiet(args.format.quiet);
        let exit =
            crate::format::render::render_doc(&doc, &fmt, &data_lines, args.format.select_spec())?;
        eprintln!("Re-run with --apply to execute");
        return Ok(exit);
    }

    match gate.confirm(Confirm::LargeFanout, services.len(), "Continue?") {
        ConfirmResult::Aborted(why) => {
            eprintln!("aborted: {why}");
            return Ok(ExitKind::Error);
        }
        ConfirmResult::DryRun => unreachable!("apply branch already taken"),
        ConfirmResult::Apply => {}
    }

    let store = AuditStore::open()?;
    let mut ok = 0usize;
    let mut bad = 0usize;
    let mut data_lines: Vec<String> = Vec::new();
    for svc in &services {
        let cmd = format!(
            "cd {wd} && docker compose -p {p} restart {svc}",
            wd = shquote(&project.working_dir),
            p = shquote(&project.name),
            svc = shquote(svc),
        );
        let started = Instant::now();
        let out = runner.run(&parsed.namespace, &target, &cmd, RunOpts::with_timeout(120))?;
        let dur = started.elapsed().as_millis() as u64;

        let mut entry = AuditEntry::new(
            "compose.restart",
            &format!("{}/{}/{}", parsed.namespace, project.name, svc),
        );
        entry.exit = out.exit_code;
        entry.duration_ms = dur;
        entry.reason = crate::safety::validate_reason(args.reason.as_deref())?;
        entry.applied = Some(out.ok());
        // Restart has no clean inverse — match the existing
        // lifecycle verb's `Revert::unsupported` shape.
        entry.revert = Some(Revert::unsupported(format!(
            "compose restart has no inverse; re-run \
             `inspect compose restart {ns}/{p}/{svc} --apply` to repeat",
            ns = parsed.namespace,
            p = project.name
        )));
        // audit-tag stamps. Bracketed-tag style matches the
        // CLAUDE.md audit schema convention so `inspect audit grep`
        // can filter on `[project=…]` / `[service=…]` /
        // `[compose_file_hash=…]` substring matches.
        entry.args = format!(
            "[project={p}] [service={svc}]{hash}",
            p = project.name,
            hash = if compose_hash.is_empty() {
                String::new()
            } else {
                format!(" [compose_file_hash={compose_hash}]")
            },
        );
        store.append(&entry)?;

        if out.ok() {
            ok += 1;
            data_lines.push(format!(
                "{ns}/{p}/{svc}: restarted (audit_id={id})",
                ns = parsed.namespace,
                p = project.name,
                id = entry.id,
            ));
        } else {
            bad += 1;
            data_lines.push(format!(
                "{ns}/{p}/{svc}: FAILED (exit {exit}): {err}",
                ns = parsed.namespace,
                p = project.name,
                exit = out.exit_code,
                err = out.stderr.trim()
            ));
        }
    }

    // Invalidate the runtime cache for the namespace so the
    // next `inspect status arte` reflects the post-restart state.
    crate::verbs::cache::invalidate(&parsed.namespace);

    let mut doc = OutputDoc::new(
        format!(
            "compose restart {ns}/{p}: {ok} ok, {bad} failed",
            ns = parsed.namespace,
            p = project.name
        ),
        json!({
            "namespace": parsed.namespace,
            "project": project.name,
            "services": services,
            "ok": ok,
            "failed": bad,
            "compose_file_hash": compose_hash,
        }),
    )
    .with_meta("selector", args.selector.clone())
    .with_quiet(args.format.quiet);
    doc.push_next(NextStep::new(
        "inspect audit ls --limit 5",
        "review the audit entries this verb just appended",
    ));
    let exit =
        crate::format::render::render_doc(&doc, &fmt, &data_lines, args.format.select_spec())?;

    // Per-step failure exit class takes precedence over filter-class.
    Ok(if bad == 0 { exit } else { ExitKind::Error })
}

/// Fetch the project's compose file body and return the first 12
/// hex chars of its SHA-256. Returns `String::new()` on any
/// failure — the audit entry still records project + service.
fn compose_file_sha_short(
    runner: &dyn RemoteRunner,
    ns: &str,
    target: &SshTarget,
    project: &ComposeProject,
) -> String {
    if project.compose_file.is_empty() {
        return String::new();
    }
    let cmd = format!("cat {f} 2>/dev/null", f = shquote(&project.compose_file));
    let out = match runner.run(ns, target, &cmd, RunOpts::with_timeout(15)) {
        Ok(o) if o.ok() => o,
        _ => return String::new(),
    };
    let hex = crate::safety::snapshot::sha256_hex(out.stdout.as_bytes());
    hex.chars().take(12).collect()
}

/// Enumerate the services declared by a compose project. Used when
/// the operator passed `--all` without a service portion. Falls
/// back to the cached `running_count`-derived total only when the
/// live probe fails.
fn list_project_services(
    runner: &dyn RemoteRunner,
    ns: &str,
    target: &SshTarget,
    project: &ComposeProject,
) -> Result<Vec<String>> {
    let cmd = format!(
        "cd {wd} && docker compose -p {p} config --services 2>/dev/null",
        wd = shquote(&project.working_dir),
        p = shquote(&project.name),
    );
    let out = runner.run(ns, target, &cmd, RunOpts::with_timeout(15))?;
    if !out.ok() {
        anyhow::bail!(
            "docker compose config --services exited {}: {}",
            out.exit_code,
            out.stderr.trim()
        );
    }
    Ok(out
        .stdout
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}
