//! `inspect bundle` command handler (B9, v0.1.2).
//!
//! Thin shell over [`crate::bundle`]: load YAML, dispatch to plan or
//! apply.

use anyhow::{Context, Result};

use crate::bundle::{self, ApplyOpts, Bundle};
use crate::cli::{BundleApplyArgs, BundleArgs, BundleMode, BundlePlanArgs, BundleStatusArgs};
use crate::error::ExitKind;
use crate::safety::AuditStore;
use crate::verbs::output::{Envelope, JsonOut, Renderer};

pub fn run(args: BundleArgs) -> Result<ExitKind> {
    match args.mode {
        BundleMode::Plan(a) => plan(a),
        BundleMode::Apply(a) => apply(a),
        BundleMode::Status(a) => status(a),
    }
}

fn plan(args: BundlePlanArgs) -> Result<ExitKind> {
    let bundle = load(&args.file)?;
    bundle::plan(&bundle)
}

fn apply(args: BundleApplyArgs) -> Result<ExitKind> {
    let bundle = load(&args.file)?;
    // Reason is propagated via the bundle's own `reason:` field if the
    // YAML didn't already set one. The CLI flag is the override.
    let mut bundle = bundle;
    if let Some(r) = args.reason {
        let validated = crate::safety::validate_reason(Some(r.as_str()))?;
        bundle.reason = validated;
    }
    bundle::apply(
        &bundle,
        ApplyOpts {
            apply: args.apply,
            no_prompt: args.no_prompt,
        },
    )
}

fn load(path: &std::path::Path) -> Result<Bundle> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading bundle file `{}`", path.display()))?;
    let bundle: Bundle = serde_yaml::from_str(&raw)
        .with_context(|| format!("parsing bundle file `{}`", path.display()))?;
    Ok(bundle)
}

/// L6 (v0.1.3): `inspect bundle status <bundle_id>` — read every
/// audit entry tagged with this `bundle_id`, group by step, and
/// render the per-branch outcome table. The audit log is the
/// source of truth — a bundle that ran a year ago is queryable as
/// long as its entries haven't aged out per L5 retention.
fn status(args: BundleStatusArgs) -> Result<ExitKind> {
    // F19 (v0.1.3): activate the FormatArgs mutex check
    // (e.g. `--select` without `--json` → exit 2).
    args.format.resolve()?;
    let store = AuditStore::open()?;
    let entries = store.all()?;
    // Resolve prefix to a unique bundle_id.
    let mut matches: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for e in &entries {
        if let Some(id) = &e.bundle_id {
            if id.starts_with(&args.bundle_id) {
                matches.insert(id.clone());
            }
        }
    }
    if matches.is_empty() {
        crate::error::emit(format!(
            "no bundle invocation matches id prefix `{}`\nhint: see 'inspect audit ls' for recent bundles",
            args.bundle_id
        ));
        return Ok(ExitKind::NoMatches);
    }
    if matches.len() > 1 {
        crate::error::emit(format!(
            "bundle id prefix `{}` is ambiguous ({} matches): {:?}",
            args.bundle_id,
            matches.len(),
            matches
        ));
        return Ok(ExitKind::Error);
    }
    let bundle_id = matches.into_iter().next().unwrap();
    // Collect entries for this bundle, in audit-append order.
    let bundle_entries: Vec<_> = entries
        .iter()
        .filter(|e| e.bundle_id.as_deref() == Some(bundle_id.as_str()))
        .collect();

    // Group by step. Within each step, branches are deterministically
    // ordered by the entry that recorded them (earliest append wins).
    use std::collections::BTreeMap;
    let mut steps: BTreeMap<String, Vec<&crate::safety::AuditEntry>> = BTreeMap::new();
    let mut step_order: Vec<String> = Vec::new();
    for e in &bundle_entries {
        let step = e.bundle_step.clone().unwrap_or_else(|| "(none)".into());
        if !steps.contains_key(&step) {
            step_order.push(step.clone());
        }
        steps.entry(step).or_default().push(e);
    }

    let json = args.format.is_json();
    if json {
        let mut step_rows: Vec<serde_json::Value> = Vec::new();
        for step in &step_order {
            let es = &steps[step];
            let mut branches: Vec<serde_json::Value> = Vec::new();
            for e in es {
                if e.bundle_branch.is_some() {
                    branches.push(serde_json::json!({
                        "branch": e.bundle_branch,
                        "status": e.bundle_branch_status,
                        "audit_id": e.id,
                        "verb": e.verb,
                        "exit": e.exit,
                        "duration_ms": e.duration_ms,
                        "is_revert": e.is_revert,
                    }));
                }
            }
            let kind = if branches.is_empty() {
                "single"
            } else {
                "matrix"
            };
            let single = if branches.is_empty() {
                es.iter()
                    .map(|e| {
                        serde_json::json!({
                            "audit_id": e.id,
                            "verb": e.verb,
                            "exit": e.exit,
                            "duration_ms": e.duration_ms,
                            "is_revert": e.is_revert,
                        })
                    })
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            step_rows.push(serde_json::json!({
                "step": step,
                "kind": kind,
                "branches": branches,
                "entries": single,
            }));
        }
        let mut select = args.format.select_filter()?;
        JsonOut::write(
            &Envelope::new("local", "bundle", "bundle.status")
                .put("bundle_id", bundle_id.clone())
                .put("entries_total", bundle_entries.len())
                .put("steps", step_rows),
            select.as_mut(),
        )?;
        crate::verbs::output::flush_filter(select.as_mut())?;
        return Ok(ExitKind::Success);
    }

    let mut r = Renderer::new();
    r.summary(format!(
        "bundle status: id={bundle_id}  {} step(s),  {} audit entr{}",
        step_order.len(),
        bundle_entries.len(),
        if bundle_entries.len() == 1 {
            "y"
        } else {
            "ies"
        },
    ));
    for step in &step_order {
        let es = &steps[step];
        // Branches first, otherwise the single entry.
        let mut branch_lines: Vec<String> = Vec::new();
        for e in es {
            if let Some(b) = &e.bundle_branch {
                let mark = match e.bundle_branch_status.as_deref() {
                    Some("ok") if !e.is_revert => "✓",
                    Some("ok") if e.is_revert => "↶",
                    Some("failed") => "✗",
                    Some("skipped") => "·",
                    _ => "?",
                };
                let role = if e.is_revert {
                    " (revert)"
                } else if e.verb.starts_with("bundle.rollback") {
                    " (rollback)"
                } else {
                    ""
                };
                branch_lines.push(format!("  {mark} {b}{role}  ({}ms)", e.duration_ms));
            }
        }
        if branch_lines.is_empty() {
            // Non-matrix step.
            for e in es {
                let mark = if e.exit == 0 { "✓" } else { "✗" };
                let role = if e.is_revert { " (revert)" } else { "" };
                r.data_line(format!(
                    "step `{step}`: {mark} {} {}{role}  ({}ms)",
                    e.verb, e.selector, e.duration_ms,
                ));
            }
        } else {
            r.data_line(format!("step `{step}` (matrix):"));
            for l in branch_lines {
                r.data_line(l);
            }
        }
    }
    r.next("inspect audit show <id>          # zoom into a specific entry");
    r.next(format!(
        "inspect audit grep '{bundle_id}'   # match every entry tagged with this bundle"
    ));
    r.print();
    Ok(ExitKind::Success)
}
