//! `inspect revert <audit-id>` (bible §8.2; F11 v0.1.3 universal contract).
//!
//! Restores a prior write by replaying the inverse the original verb
//! pre-staged at capture-before-apply time. Three flavours:
//!   * `revert.kind = state_snapshot` — restore content from the
//!     snapshot store (legacy v0.1.2 path; `cp`, `edit`, `rm`).
//!   * `revert.kind = command_pair` — run the captured inverse remote
//!     command (`chmod`, `chown`, `mkdir`, `touch`, lifecycle).
//!   * `revert.kind = unsupported` — refuse loudly with a chained
//!     hint; never silently no-op.
//!
//! Same safety contract as write verbs: dry-run by default, `--apply`
//! to execute, `--force` to override remote drift on snapshot reverts.

use std::time::Instant;

use anyhow::{Context, Result};
use base64::Engine as _;

use crate::cli::RevertArgs;
use crate::error::ExitKind;
use crate::safety::audit::RevertKind;
use crate::safety::gate::ConfirmResult;
use crate::safety::{
    diff::{diff_summary, unified_diff},
    snapshot::sha256_hex,
    AuditEntry, AuditStore, Confirm, SafetyGate, SnapshotStore,
};
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::plan;
use crate::verbs::output::Renderer;
use crate::verbs::quote::shquote;

pub fn run(args: RevertArgs) -> Result<ExitKind> {
    let store = AuditStore::open()?;

    // F11 (v0.1.3): `--last [N]` walks the N most recent applied
    // write entries in reverse chronological order. Each one is
    // dispatched through the same single-entry path so the dry-run /
    // confirmation contract is identical.
    if let Some(n) = args.last {
        return run_last(&args, n, &store);
    }
    let Some(audit_id) = args.audit_id.as_deref() else {
        crate::error::emit("revert requires <audit-id> or `--last [N]`");
        return Ok(ExitKind::Error);
    };
    let Some(entry) = store.find(audit_id)? else {
        crate::error::emit(format!("no audit entry matches id prefix '{audit_id}'"));
        return Ok(ExitKind::Error);
    };
    revert_one(&args, &entry, &store)
}

fn run_last(args: &RevertArgs, n: usize, store: &AuditStore) -> Result<ExitKind> {
    let n = n.max(1);
    let mut all = store.all()?;
    all.reverse();
    let candidates: Vec<AuditEntry> = all
        .into_iter()
        .filter(|e| !e.is_revert && e.applied.unwrap_or(true))
        .take(n)
        .collect();
    if candidates.is_empty() {
        crate::error::emit("no recent applied write entries to revert");
        return Ok(ExitKind::Error);
    }
    let mut last_exit = ExitKind::Success;
    for e in &candidates {
        let ek = revert_one(args, e, store)?;
        if matches!(ek, ExitKind::Error) {
            // Stop on the first failure / refusal so the operator
            // sees the chained hint and can decide whether to skip
            // and continue manually.
            return Ok(ExitKind::Error);
        }
        last_exit = ek;
    }
    Ok(last_exit)
}

fn revert_one(args: &RevertArgs, entry: &AuditEntry, store: &AuditStore) -> Result<ExitKind> {
    // F11 backward-compat: entries written before v0.1.3 carry no
    // `revert` field. Fall back to the legacy `previous_hash` +
    // `snapshot` revert path for those; refuse with a chained hint
    // for legacy entries that have neither (e.g. lifecycle pre-F11).
    let kind = entry
        .revert
        .as_ref()
        .map(|r| r.kind.clone())
        .unwrap_or_else(|| {
            if entry.previous_hash.is_some() {
                RevertKind::StateSnapshot
            } else {
                RevertKind::Unsupported
            }
        });
    match kind {
        RevertKind::StateSnapshot => revert_state_snapshot(args, entry, store),
        RevertKind::CommandPair => revert_command_pair(args, entry, store),
        RevertKind::Composite => {
            crate::error::emit(format!(
                "audit '{}' has revert.kind=composite; composite reverts are not yet \
                 implemented (v0.1.3). Replay the bundle's per-step entries individually.",
                entry.id
            ));
            Ok(ExitKind::Error)
        }
        RevertKind::Unsupported => revert_unsupported(entry),
    }
}

fn revert_unsupported(entry: &AuditEntry) -> Result<ExitKind> {
    let preview = entry
        .revert
        .as_ref()
        .map(|r| r.preview.as_str())
        .unwrap_or("");
    if entry.no_revert_acknowledged {
        crate::error::emit(format!(
            "audit '{}' was applied with --no-revert (verb '{}'); no inverse was \
             captured. {preview}",
            entry.id, entry.verb
        ));
    } else if entry.revert.is_none() {
        // Legacy v0.1.2 entry without the F11 contract.
        crate::error::emit(format!(
            "audit '{}' (verb '{}') predates the revert contract (v0.1.2 or earlier) \
             and has no captured inverse. Inspect the entry with `inspect audit show {}` \
             and revert manually.",
            entry.id, entry.verb, entry.id
        ));
    } else {
        crate::error::emit(format!(
            "audit '{}' (verb '{}') has revert.kind=unsupported: {preview}",
            entry.id, entry.verb
        ));
    }
    Ok(ExitKind::Error)
}

fn revert_command_pair(
    args: &RevertArgs,
    entry: &AuditEntry,
    store: &AuditStore,
) -> Result<ExitKind> {
    let revert = entry
        .revert
        .as_ref()
        .expect("kind=command_pair implies Some");
    let cmd = revert.payload.clone();
    if cmd.is_empty() {
        crate::error::emit(format!(
            "audit '{}' has revert.kind=command_pair but empty payload; cannot revert",
            entry.id
        ));
        return Ok(ExitKind::Error);
    }
    let (runner, nses, targets) = plan(&entry.selector)?;
    let steps: Vec<_> = crate::verbs::dispatch::iter_steps(&nses, &targets).collect();
    let Some(step) = steps.first() else {
        crate::error::emit(format!(
            "selector '{}' from audit '{}' no longer matches any target",
            entry.selector, entry.id
        ));
        return Ok(ExitKind::Error);
    };
    let label = format!(
        "{}{}",
        step.ns.namespace,
        step.service().map(|x| format!("/{x}")).unwrap_or_default()
    );
    let wrapped = match step.container() {
        Some(container) => format!("docker exec {} sh -c {}", shquote(container), shquote(&cmd)),
        None => cmd.clone(),
    };
    let gate = SafetyGate::new(args.apply, args.yes, args.yes_all);
    if !gate.should_apply() {
        let mut r = Renderer::new();
        r.summary(format!(
            "DRY RUN. Would revert audit {} ({})",
            entry.id, label
        ));
        r.data_line(format!("REVERT: {}", revert.preview));
        r.data_line(format!("  + {cmd}"));
        r.next("Re-run with --apply to execute");
        r.print();
        return Ok(ExitKind::Success);
    }
    if let ConfirmResult::Aborted(why) = gate.confirm(Confirm::Always, 1, "Revert?") {
        eprintln!("aborted: {why}");
        return Ok(ExitKind::Error);
    }
    let started = Instant::now();
    let out = runner.run(
        &step.ns.namespace,
        &step.ns.target,
        &wrapped,
        RunOpts::with_timeout(60),
    )?;
    let dur = started.elapsed().as_millis() as u64;

    let mut rev_entry = AuditEntry::new("revert", &label);
    rev_entry.is_revert = true;
    rev_entry.reverts = Some(entry.id.clone());
    rev_entry.args = cmd.clone();
    rev_entry.exit = out.exit_code;
    rev_entry.duration_ms = dur;
    rev_entry.applied = Some(out.ok());
    store.append(&rev_entry)?;

    let mut r = Renderer::new();
    if out.ok() {
        r.summary(format!(
            "reverted audit {} → {label} (audit {})",
            entry.id, rev_entry.id
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

fn revert_state_snapshot(
    args: &RevertArgs,
    entry: &AuditEntry,
    store: &AuditStore,
) -> Result<ExitKind> {
    let snaps = SnapshotStore::open()?;

    let Some(prev_hash) = entry.previous_hash.clone() else {
        crate::error::emit(format!(
            "audit '{}' has revert.kind=state_snapshot but no previous_hash; \
             nothing to restore",
            entry.id
        ));
        return Ok(ExitKind::Error);
    };
    let original = snaps
        .get(&prev_hash)
        .with_context(|| format!("loading snapshot for '{}'", entry.id))?;
    let original_text = String::from_utf8_lossy(&original).to_string();
    let recorded_new_hash = entry.new_hash.clone().unwrap_or_default();

    let (runner, nses, targets) = plan(&entry.selector)?;
    let steps: Vec<_> = crate::verbs::dispatch::iter_steps(&nses, &targets).collect();
    let Some(step) = steps.first() else {
        crate::error::emit(format!(
            "selector '{}' from audit no longer matches any target",
            entry.selector
        ));
        return Ok(ExitKind::Error);
    };
    let Some(path) = step.path.clone() else {
        crate::error::emit(format!("audit selector '{}' has no path", entry.selector));
        return Ok(ExitKind::Error);
    };

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
            "DRY RUN. Would revert audit {} ({label} → snapshot)",
            entry.id
        ));
        if let Some(rev) = entry.revert.as_ref() {
            r.data_line(format!("REVERT: {}", rev.preview));
        }
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
    let inner = crate::verbs::write::atomic::write_then_rename(&b64, &tmp, &path);
    let cmd = match step.service() {
        Some(svc) => format!("docker exec {} sh -c {}", shquote(svc), shquote(&inner)),
        None => format!("sh -c {}", shquote(&inner)),
    };
    let started = Instant::now();
    let out = runner.run(
        &step.ns.namespace,
        &step.ns.target,
        &cmd,
        RunOpts::with_timeout(60),
    )?;
    let dur = started.elapsed().as_millis() as u64;

    let mut rev_entry = AuditEntry::new("revert", &label);
    rev_entry.is_revert = true;
    rev_entry.reverts = Some(entry.id.clone());
    rev_entry.previous_hash = Some(current_hash.clone());
    rev_entry.new_hash = Some(format!("sha256:{restored_hash}"));
    rev_entry.snapshot = Some(
        snaps
            .path_for(&sha256_hex(current_text.as_bytes()))
            .display()
            .to_string(),
    );
    rev_entry.diff_summary = diff_summary(&[(current_text.clone(), original_text.clone())]);
    rev_entry.exit = out.exit_code;
    rev_entry.duration_ms = dur;
    rev_entry.applied = Some(out.ok());
    let _ = snaps.put(current_text.as_bytes());
    store.append(&rev_entry)?;

    let mut r = Renderer::new();
    if out.ok() {
        r.summary(format!(
            "reverted audit {} → {label} (audit {})",
            entry.id, rev_entry.id
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
        .run(
            &s.ns.namespace,
            &s.ns.target,
            &cmd,
            RunOpts::with_timeout(20),
        )
        .ok()?;
    if out.ok() {
        Some(out.stdout)
    } else {
        None
    }
}
