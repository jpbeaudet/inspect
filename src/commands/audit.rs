//! `inspect audit ls|show|grep|verify|gc` (bible §8.2; gc = L5
//! v0.1.3).

use anyhow::Result;

use crate::cli::{AuditArgs, AuditCommand, AuditGcArgs};
use crate::error::ExitKind;
use crate::safety::gc::{parse_retention, run_gc, GcReport};
use crate::safety::snapshot::sha256_hex;
use crate::safety::{AuditStore, SnapshotStore};
use crate::verbs::output::{Envelope, JsonOut, Renderer};

pub fn run(args: AuditArgs) -> Result<ExitKind> {
    // The `gc` path opens its own store implicitly via the path
    // helpers — it walks files directly so we don't pre-load every
    // entry just to discard most of them.
    if let AuditCommand::Gc(o) = &args.command {
        return gc(o);
    }
    let store = AuditStore::open()?;
    let entries = store.all()?;
    match args.command {
        AuditCommand::Ls(o) => list(
            &entries,
            o.format.is_json(),
            Some(o.limit),
            o.reason.as_deref(),
        ),
        AuditCommand::Show(o) => show(&entries, &o.id, o.format.is_json()),
        AuditCommand::Grep(o) => grep(&entries, &o.pattern, o.format.is_json()),
        AuditCommand::Verify(o) => verify(&entries, o.format.is_json()),
        AuditCommand::Gc(_) => unreachable!("handled above"),
    }
}

fn list(
    entries: &[crate::safety::AuditEntry],
    json: bool,
    limit: Option<usize>,
    reason_filter: Option<&str>,
) -> Result<ExitKind> {
    let n_total = entries.len();
    // Newest first.
    let mut sorted: Vec<_> = entries.iter().collect();
    sorted.sort_by_key(|e| std::cmp::Reverse(e.ts));

    // P12: optional case-insensitive substring filter on `reason`.
    let needle = reason_filter.map(|s| s.to_lowercase());
    if let Some(needle) = &needle {
        sorted.retain(|e| {
            e.reason
                .as_ref()
                .map(|r| r.to_lowercase().contains(needle))
                .unwrap_or(false)
        });
    }
    let n = sorted.len();
    let take = limit.unwrap_or(50).min(sorted.len());
    let view = &sorted[..take];

    if json {
        for e in view {
            let mut env = Envelope::new(&e.host, "audit", "audit")
                .put("id", e.id.clone())
                .put("ts", e.ts.to_rfc3339())
                .put("verb", e.verb.clone())
                .put("selector", e.selector.clone())
                .put("exit", e.exit)
                .put("diff_summary", e.diff_summary.clone())
                .put("is_revert", e.is_revert);
            // P12: always emit `reason` in JSON (null when absent).
            env = match &e.reason {
                Some(r) => env.put("reason", r.clone()),
                None => env.put("reason", serde_json::Value::Null),
            };
            JsonOut::write(&env);
        }
        return Ok(ExitKind::Success);
    }

    let mut r = Renderer::new();
    let header = match reason_filter {
        Some(p) => format!(
            "{n} audit entry/entries matching --reason '{p}' (of {n_total} total; showing {take})"
        ),
        None => format!("{n_total} audit entry/entries (showing {take})"),
    };
    r.summary(header);
    for e in view {
        let badge = if e.exit == 0 { "ok " } else { "ERR" };
        let revert = if e.is_revert { " (revert)" } else { "" };
        // P12: append the reason as the trailing column when present.
        let reason_cell = match &e.reason {
            Some(text) => format!("  reason: {}", truncate_reason(text, 60)),
            None => String::new(),
        };
        r.data_line(format!(
            "{} [{badge}] {} {} {}{revert} — {}{reason_cell}",
            e.id,
            e.ts.format("%Y-%m-%d %H:%M:%S"),
            e.verb,
            e.selector,
            if e.diff_summary.is_empty() {
                ""
            } else {
                &e.diff_summary
            },
        ));
    }
    r.next("inspect audit show <id>");
    r.next("inspect revert <id>");
    r.print();
    Ok(ExitKind::Success)
}

/// Truncate a `--reason` for the audit-ls human view. Char-aware so a
/// non-ASCII reason doesn't get sliced mid-codepoint.
fn truncate_reason(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        return s.to_string();
    }
    let mut out: String = chars
        .into_iter()
        .take(max_chars.saturating_sub(1))
        .collect();
    out.push('…');
    out
}

fn show(entries: &[crate::safety::AuditEntry], id_prefix: &str, json: bool) -> Result<ExitKind> {
    let Some(e) = entries.iter().find(|e| e.id.starts_with(id_prefix)) else {
        crate::error::emit("no audit entry matches id prefix '{id_prefix}'");
        return Ok(ExitKind::Error);
    };
    if json {
        println!("{}", serde_json::to_string_pretty(e)?);
        return Ok(ExitKind::Success);
    }
    let mut r = Renderer::new();
    r.summary(format!("audit {} ({})", e.id, e.verb));
    r.data_line(format!("ts:        {}", e.ts.to_rfc3339()));
    r.data_line(format!("user:      {}", e.user));
    r.data_line(format!("host:      {}", e.host));
    r.data_line(format!("selector:  {}", e.selector));
    if !e.args.is_empty() {
        r.data_line(format!("args:      {}", e.args));
    }
    if !e.diff_summary.is_empty() {
        r.data_line(format!("diff:      {}", e.diff_summary));
    }
    if let Some(h) = &e.previous_hash {
        r.data_line(format!("prev hash: {h}"));
    }
    if let Some(h) = &e.new_hash {
        r.data_line(format!("new hash:  {h}"));
    }
    if let Some(s) = &e.snapshot {
        r.data_line(format!("snapshot:  {s}"));
    }
    r.data_line(format!("exit:      {}", e.exit));
    r.data_line(format!("duration:  {} ms", e.duration_ms));
    if let Some(reason) = &e.reason {
        r.data_line(format!("reason:    {reason}"));
    }
    if e.is_revert {
        r.data_line("(this entry is a revert)");
    }
    if let Some(rev) = &e.reverts {
        r.data_line(format!("reverts:   {rev}"));
    }
    r.next("inspect revert <id>");
    r.print();
    Ok(ExitKind::Success)
}

fn grep(entries: &[crate::safety::AuditEntry], pat: &str, json: bool) -> Result<ExitKind> {
    let needle = pat.to_lowercase();
    let mut hits = 0usize;
    let mut r = Renderer::new();
    for e in entries {
        let blob = format!(
            "{} {} {} {} {}",
            e.id, e.verb, e.selector, e.args, e.diff_summary
        )
        .to_lowercase();
        if !blob.contains(&needle) {
            continue;
        }
        hits += 1;
        if json {
            JsonOut::write(
                &Envelope::new(&e.host, "audit", "audit")
                    .put("id", e.id.clone())
                    .put("ts", e.ts.to_rfc3339())
                    .put("verb", e.verb.clone())
                    .put("selector", e.selector.clone())
                    .put("exit", e.exit),
            );
        } else {
            r.data_line(format!(
                "{} {} {} {}",
                e.id,
                e.ts.format("%Y-%m-%d %H:%M:%S"),
                e.verb,
                e.selector
            ));
        }
    }
    if !json {
        r.summary(format!("audit grep '{pat}': {hits} match(es)"));
        r.print();
    }
    Ok(if hits > 0 {
        ExitKind::Success
    } else {
        ExitKind::NoMatches
    })
}

/// Field pitfall §3.4: best-effort integrity check.
///
/// Walks every audit entry and confirms:
///   1. each entry's referenced `snapshot` file exists, and
///   2. the file's on-disk sha256 matches the `previous_hash` recorded
///      in the entry (modulo the optional `sha256:` prefix).
///
/// Honest scope: this catches accidental loss/truncation of snapshot
/// files and silent on-disk corruption. It does **not** prove the
/// JSONL log itself was not rewritten — a privileged local user can
/// always edit `~/.inspect/audit/*.jsonl` and recompute matching
/// snapshots. For tamper-evidence, forward audit entries to an
/// append-only sink (syslog, journald, or a remote collector).
fn verify(entries: &[crate::safety::AuditEntry], json: bool) -> Result<ExitKind> {
    let snaps = SnapshotStore::open()?;
    let mut checked = 0usize;
    let mut missing: Vec<(String, String)> = Vec::new(); // (id, snapshot path)
    let mut mismatched: Vec<(String, String)> = Vec::new(); // (id, expected hash)
    for e in entries {
        let Some(snap_path_str) = e.snapshot.as_ref() else {
            continue;
        };
        let Some(prev_hash) = e.previous_hash.as_ref() else {
            continue;
        };
        checked += 1;
        let expected = prev_hash.strip_prefix("sha256:").unwrap_or(prev_hash);
        // Resolve via the store's canonical path so we tolerate audit
        // logs copied between hosts where the absolute snapshot path
        // recorded in `e.snapshot` no longer exists locally.
        let resolved = snaps.path_for(expected);
        let path = if resolved.exists() {
            resolved
        } else {
            std::path::PathBuf::from(snap_path_str)
        };
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => {
                missing.push((e.id.clone(), path.display().to_string()));
                continue;
            }
        };
        let actual = sha256_hex(&bytes);
        if actual != expected {
            mismatched.push((e.id.clone(), expected.to_string()));
        }
    }

    let bad = missing.len() + mismatched.len();
    if json {
        JsonOut::write(
            &Envelope::new("local", "audit", "audit.verify")
                .put("entries_total", entries.len())
                .put("entries_with_snapshot", checked)
                .put("missing_count", missing.len())
                .put("mismatched_count", mismatched.len())
                .put(
                    "missing_ids",
                    missing.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
                )
                .put(
                    "mismatched_ids",
                    mismatched
                        .iter()
                        .map(|(id, _)| id.clone())
                        .collect::<Vec<_>>(),
                )
                .put("ok", bad == 0),
        );
        return Ok(if bad == 0 {
            ExitKind::Success
        } else {
            ExitKind::Error
        });
    }

    let mut r = Renderer::new();
    r.summary(format!(
        "audit verify: {} entries, {checked} with snapshots, {} missing, {} mismatched",
        entries.len(),
        missing.len(),
        mismatched.len()
    ));
    for (id, p) in &missing {
        r.data_line(format!("MISSING  {id}  snapshot not on disk: {p}"));
    }
    for (id, expected) in &mismatched {
        r.data_line(format!(
            "MISMATCH {id}  on-disk content sha256 != recorded {expected}"
        ));
    }
    if bad == 0 {
        r.data_line("ok: every referenced snapshot is present and hashes match");
    }
    r.next("note: this checks snapshot integrity, not JSONL-log tampering");
    r.next("forward audit entries to an append-only sink for tamper-evidence");
    r.print();
    Ok(if bad == 0 {
        ExitKind::Success
    } else {
        ExitKind::Error
    })
}

/// L5 (v0.1.3): dispatch for `inspect audit gc --keep <X>`.
fn gc(args: &AuditGcArgs) -> Result<ExitKind> {
    let policy = match parse_retention(&args.keep) {
        Ok(p) => p,
        Err(e) => {
            crate::error::emit(format!(
                "{e}\nhint: see 'inspect audit --help' for the GC + RETENTION section"
            ));
            return Ok(ExitKind::Error);
        }
    };
    let report = run_gc(&policy, args.dry_run)?;
    if args.format.is_json() {
        emit_gc_json(&report);
        return Ok(ExitKind::Success);
    }
    let mut r = Renderer::new();
    let prefix = if report.dry_run {
        "would delete"
    } else {
        "deleted"
    };
    r.summary(format!(
        "audit gc (--keep {}): {prefix} {} entries / {} snapshots / {} bytes; {} entries kept",
        report.policy,
        report.deleted_entries,
        report.deleted_snapshots,
        format_bytes(report.freed_bytes),
        report.entries_kept,
    ));
    if report.deleted_entries == 0 && report.deleted_snapshots == 0 {
        r.data_line("nothing to delete (every entry + snapshot is within the retention window)");
    } else {
        for id in report.deleted_ids.iter().take(10) {
            r.data_line(format!("entry  {prefix}: {id}"));
        }
        if report.deleted_ids.len() > 10 {
            r.data_line(format!(
                "  … and {} more entries (use --json for the full list)",
                report.deleted_ids.len() - 10
            ));
        }
        for h in report.deleted_snapshot_hashes.iter().take(10) {
            r.data_line(format!("snapshot {prefix}: sha256-{h}"));
        }
        if report.deleted_snapshot_hashes.len() > 10 {
            r.data_line(format!(
                "  … and {} more snapshots",
                report.deleted_snapshot_hashes.len() - 10
            ));
        }
    }
    if report.dry_run {
        r.next("inspect audit gc --keep <X>   # rerun without --dry-run to apply");
    } else {
        r.next("inspect audit verify          # confirm remaining entries are intact");
    }
    r.print();
    Ok(ExitKind::Success)
}

fn emit_gc_json(r: &GcReport) {
    JsonOut::write(
        &Envelope::new("local", "audit", "audit.gc")
            .put("dry_run", r.dry_run)
            .put("policy", r.policy.clone())
            .put("entries_total", r.entries_total)
            .put("entries_kept", r.entries_kept)
            .put("deleted_entries", r.deleted_entries)
            .put("deleted_snapshots", r.deleted_snapshots)
            .put("freed_bytes", r.freed_bytes)
            .put("deleted_ids", r.deleted_ids.clone())
            .put("deleted_snapshot_hashes", r.deleted_snapshot_hashes.clone()),
    );
}

fn format_bytes(b: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    if b >= MB {
        format!("{:.2} MiB", b as f64 / MB as f64)
    } else if b >= KB {
        format!("{:.2} KiB", b as f64 / KB as f64)
    } else {
        format!("{b} B")
    }
}
