//! `inspect revert <audit-id>` (bible §8.2).
//!
//! Restores the file at the recorded selector to the snapshotted content.
//! Same safety contract as write verbs: dry-run by default with reverse
//! diff, `--apply` to execute. If the current remote hash != recorded
//! `new_hash`, requires `--force`.

use std::time::Instant;

use anyhow::{Context, Result};
use base64::Engine as _;

use crate::cli::RevertArgs;
use crate::error::ExitKind;
use crate::safety::{
    diff::{diff_summary, unified_diff},
    snapshot::sha256_hex,
    AuditEntry, AuditStore, Confirm, SafetyGate, SnapshotStore,
};
use crate::safety::gate::ConfirmResult;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::plan;
use crate::verbs::output::Renderer;
use crate::verbs::quote::shquote;

pub fn run(args: RevertArgs) -> Result<ExitKind> {
    let store = AuditStore::open()?;
    let snaps = SnapshotStore::open()?;

    let Some(entry) = store.find(&args.audit_id)? else {
        eprintln!("error: no audit entry matches id prefix '{}'", args.audit_id);
        return Ok(ExitKind::Error);
    };
    let Some(prev_hash) = entry.previous_hash.clone() else {
        eprintln!(
            "error: audit '{}' has no previous_hash; nothing to restore",
            entry.id
        );
        return Ok(ExitKind::Error);
    };
    let original = snaps
        .get(&prev_hash)
        .with_context(|| format!("loading snapshot for '{}'", entry.id))?;
    let original_text = String::from_utf8_lossy(&original).to_string();
    let recorded_new_hash = entry.new_hash.clone().unwrap_or_default();

    // Re-resolve the selector to fetch current remote content for the diff.
    let (runner, nses, targets) = plan(&entry.selector)?;
    let steps: Vec<_> = crate::verbs::dispatch::iter_steps(&nses, &targets).collect();
    let Some(step) = steps.first() else {
        eprintln!(
            "error: selector '{}' from audit no longer matches any target",
            entry.selector
        );
        return Ok(ExitKind::Error);
    };
    let Some(path) = step.path.clone() else {
        eprintln!("error: audit selector '{}' has no path", entry.selector);
        return Ok(ExitKind::Error);
    };

    // Read current remote content.
    let current_text = read_remote(&*runner, step, &path).unwrap_or_default();
    let current_hash = format!("sha256:{}", sha256_hex(current_text.as_bytes()));
    let drift = !recorded_new_hash.is_empty() && current_hash != recorded_new_hash;
    if drift && !args.force {
        eprintln!(
            "error: remote content has changed since audit '{}' (current {}, expected {}). \
             Re-run with --force to override.",
            entry.id, current_hash, recorded_new_hash
        );
        return Ok(ExitKind::Error);
    }

    let label = format!(
        "{}{}:{}",
        step.ns.namespace,
        step.service().map(|x| format!("/{x}")).unwrap_or_default(),
        path
    );

    let gate = SafetyGate::new(args.apply, args.yes, args.yes_all);
    if !gate.should_apply() {
        let mut r = Renderer::new();
        let block = unified_diff(
            &current_text,
            &original_text,
            &format!("{label} (current)"),
            &format!("{label} (after revert)"),
        );
        r.summary(format!(
            "DRY RUN. Would revert audit {} ({} -> snapshot)",
            entry.id, label
        ));
        if drift {
            r.data_line(format!(
                "WARNING: remote drifted since this audit (current {current_hash})"
            ));
        }
        if !block.is_empty() {
            r.data_line(block);
        } else {
            r.data_line("(file already matches the snapshot — nothing to do)");
        }
        r.next("Re-run with --apply to execute");
        r.print();
        return Ok(ExitKind::Success);
    }

    if let ConfirmResult::Aborted(why) = gate.confirm(Confirm::Always, 1, "Revert?") {
        eprintln!("aborted: {why}");
        return Ok(ExitKind::Error);
    }

    if current_text == original_text {
        let mut r = Renderer::new();
        r.summary("nothing to do — file already matches the snapshot");
        r.print();
        return Ok(ExitKind::Success);
    }

    let restored_hash = sha256_hex(&original);
    let b64 = base64::engine::general_purpose::STANDARD.encode(&original);
    let tmp = format!("{path}.inspect.{}.tmp", &restored_hash[..8]);
    let inner = format!(
        "set -e; printf %s {b64_q} | base64 -d > {tmp_q} && mv {tmp_q} {path_q}",
        b64_q = shquote(&b64),
        tmp_q = shquote(&tmp),
        path_q = shquote(&path),
    );
    let cmd = match step.service() {
        Some(svc) => format!("docker exec {} sh -c {}", shquote(svc), shquote(&inner)),
        None => format!("sh -c {}", shquote(&inner)),
    };
    let started = Instant::now();
    let out = runner.run(&step.ns.namespace, &step.ns.target, &cmd, RunOpts::with_timeout(60))?;
    let dur = started.elapsed().as_millis() as u64;

    // Audit-log the revert itself.
    let mut rev_entry = AuditEntry::new("revert", &label);
    rev_entry.is_revert = true;
    rev_entry.reverts = Some(entry.id.clone());
    rev_entry.previous_hash = Some(current_hash.clone());
    rev_entry.new_hash = Some(format!("sha256:{restored_hash}"));
    rev_entry.snapshot = Some(snaps.path_for(&sha256_hex(current_text.as_bytes())).display().to_string());
    rev_entry.diff_summary =
        diff_summary(&[(current_text.clone(), original_text.clone())]);
    rev_entry.exit = out.exit_code;
    rev_entry.duration_ms = dur;
    // Also store the pre-revert state as a fresh snapshot so a "revert of
    // a revert" works.
    let _ = snaps.put(current_text.as_bytes());
    store.append(&rev_entry)?;

    let mut r = Renderer::new();
    if out.ok() {
        r.summary(format!(
            "reverted audit {} → {} (audit {})",
            entry.id, label, rev_entry.id
        ));
    } else {
        r.summary(format!(
            "revert FAILED on {label} (exit {}): {}",
            out.exit_code,
            out.stderr.trim()
        ));
    }
    r.next("inspect audit show <id>");
    r.print();
    Ok(if out.ok() {
        ExitKind::Success
    } else {
        ExitKind::Error
    })
}

fn read_remote(
    runner: &dyn crate::verbs::runtime::RemoteRunner,
    s: &crate::verbs::dispatch::Step<'_>,
    path: &str,
) -> Option<String> {
    let inner = format!("cat -- {}", shquote(path));
    let cmd = match s.service() {
        Some(svc) => format!("docker exec {} sh -c {}", shquote(svc), shquote(&inner)),
        None => inner,
    };
    let out = runner
        .run(&s.ns.namespace, &s.ns.target, &cmd, RunOpts::with_timeout(20))
        .ok()?;
    if out.ok() {
        Some(out.stdout)
    } else {
        None
    }
}
