//! `inspect audit ls|show|grep|verify|gc` (bible §8.2; gc = L5
//! v0.1.3).

use anyhow::Result;

use serde_json::{json, Value};

use crate::cli::{AuditArgs, AuditCommand, AuditGcArgs};
use crate::error::ExitKind;
use crate::safety::gc::{parse_retention, run_gc, GcReport};
use crate::safety::snapshot::sha256_hex;
use crate::safety::{AuditStore, SnapshotStore};
use crate::verbs::output::{OutputDoc, Renderer};

pub fn run(args: AuditArgs) -> Result<ExitKind> {
    // The `gc` path opens its own store implicitly via the path
    // helpers — it walks files directly so we don't pre-load every
    // entry just to discard most of them.
    if let AuditCommand::Gc(o) = &args.command {
        // F19 (v0.1.3): exercise the format mutex (e.g.
        // `--select` without `--json`) before walking the
        // audit tree — same shape as the other subcommands.
        o.format.resolve()?;
        return gc(o);
    }
    let store = AuditStore::open()?;
    let entries = store.all()?;
    match args.command {
        AuditCommand::Ls(o) => {
            let format = o.format.clone();
            // F19 (v0.1.3): activate the FormatArgs mutex check
            // (e.g. `--select` without `--json` → exit 2).
            format.resolve()?;
            list(&entries, &format, Some(o.limit), o.reason.as_deref())
        }
        AuditCommand::Show(o) => {
            let format = o.format.clone();
            format.resolve()?;
            show(&entries, &o.id, &format)
        }
        AuditCommand::Grep(o) => {
            let format = o.format.clone();
            format.resolve()?;
            grep(&entries, &o.pattern, &format)
        }
        AuditCommand::Verify(o) => {
            let format = o.format.clone();
            format.resolve()?;
            verify(&entries, &format)
        }
        AuditCommand::Gc(_) => unreachable!("handled above"),
    }
}

fn list(
    entries: &[crate::safety::AuditEntry],
    format: &crate::format::FormatArgs,
    limit: Option<usize>,
    reason_filter: Option<&str>,
) -> Result<ExitKind> {
    let json = format.is_json();
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
        // v0.1.3 envelope discipline (smoke find): `audit ls --json` is
        // now a single `{schema_version, summary, data, next, meta}`
        // document with `data.entries` as a top-level array. Pre-fix
        // shape was bare-object NDJSON, which broke `jq '.[0]'` (no
        // array), conflicted with the shared `--json` flag's
        // "line-delimited JSON" promise, and forced agents to
        // round-trip every "newest entry" check through `head -1`.
        // The projection now also includes `revert.{kind,preview}` so
        // agents don't have to follow up with `audit show` for the
        // common "what kind of inverse does this audit have?"
        // question. Full `revert` block (with `payload` and
        // `previous_hash`) lives on `audit show <id> --json`.
        let entries: Vec<Value> = view
            .iter()
            .map(|e| {
                json!({
                    "id": e.id,
                    "ts": e.ts.to_rfc3339(),
                    "server": e.host,
                    "verb": e.verb,
                    "selector": e.selector,
                    "exit": e.exit,
                    "diff_summary": e.diff_summary,
                    "is_revert": e.is_revert,
                    "reason": e.reason,
                    "revert": e.revert.as_ref().map(|r| json!({
                        "kind": r.kind.as_str(),
                        "preview": r.preview,
                    })),
                })
            })
            .collect();
        let summary = match reason_filter {
            Some(p) => format!(
                "{n} audit entry/entries matching --reason '{p}' (of {n_total} total; showing {take})"
            ),
            None => format!("{n_total} audit entry/entries (showing {take})"),
        };
        return OutputDoc::new(summary, json!({ "entries": entries }))
            .with_meta("count", take)
            .with_meta("total", n_total)
            .with_meta("order", "newest_first")
            .print_json(format.select_spec());
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

fn show(
    entries: &[crate::safety::AuditEntry],
    id_prefix: &str,
    format: &crate::format::FormatArgs,
) -> Result<ExitKind> {
    let Some(e) = entries.iter().find(|e| e.id.starts_with(id_prefix)) else {
        crate::error::emit(format!("no audit entry matches id prefix '{id_prefix}'"));
        return Ok(ExitKind::Error);
    };
    if format.is_json() {
        // v0.1.3 envelope discipline: `audit show <id> --json` now
        // emits the standard `{schema_version, summary, data, next,
        // meta}` envelope with the full `AuditEntry` (revert block
        // included) under `.data.entry`. Pre-fix shape was a bare
        // pretty-printed `AuditEntry`, inconsistent with every other
        // envelope verb.
        let entry_value = serde_json::to_value(e)?;
        return OutputDoc::new(
            format!("audit {} ({})", e.id, e.verb),
            json!({ "entry": entry_value }),
        )
        .print_json(format.select_spec());
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
    // v0.1.3 (smoke find): the text `audit show` rendering used to
    // omit the F11 `revert` block entirely, forcing agents to
    // round-trip via `--json` to see whether the audit had a
    // capturable inverse. Now rendered inline.
    if let Some(rv) = &e.revert {
        r.data_line("");
        r.data_line(format!("revert.kind:    {}", rv.kind.as_str()));
        r.data_line(format!("revert.preview: {}", rv.preview));
        if !rv.payload.is_empty() {
            // Truncate long payloads (composite JSON, snapshot hashes
            // with metadata) so the human view stays scannable; full
            // payload remains in `--json`.
            let trunc = if rv.payload.len() > 160 {
                format!("{}…", &rv.payload[..159])
            } else {
                rv.payload.clone()
            };
            r.data_line(format!("revert.payload: {trunc}"));
        }
        r.data_line(format!(
            "revert.captured_at: {}",
            rv.captured_at.to_rfc3339()
        ));
    }
    r.next("inspect revert <id>");
    r.print();
    Ok(ExitKind::Success)
}

fn grep(
    entries: &[crate::safety::AuditEntry],
    pat: &str,
    format: &crate::format::FormatArgs,
) -> Result<ExitKind> {
    let needle = pat.to_lowercase();
    let mut r = Renderer::new();
    // Newest-first to match `audit ls`.
    let mut sorted: Vec<&crate::safety::AuditEntry> = entries.iter().collect();
    sorted.sort_by_key(|e| std::cmp::Reverse(e.ts));
    let matches: Vec<&crate::safety::AuditEntry> = sorted
        .into_iter()
        .filter(|e| {
            let blob = format!(
                "{} {} {} {} {}",
                e.id, e.verb, e.selector, e.args, e.diff_summary
            )
            .to_lowercase();
            blob.contains(&needle)
        })
        .collect();
    let hits = matches.len();
    if format.is_json() {
        // v0.1.3 envelope discipline: same shape as `audit ls --json`
        // — single envelope with `data.matches` as the array. Same
        // projection (id/ts/verb/selector/exit + revert summary) so
        // ls and grep are interchangeable for filtering.
        let arr: Vec<Value> = matches
            .iter()
            .map(|e| {
                json!({
                    "id": e.id,
                    "ts": e.ts.to_rfc3339(),
                    "server": e.host,
                    "verb": e.verb,
                    "selector": e.selector,
                    "exit": e.exit,
                    "is_revert": e.is_revert,
                    "revert": e.revert.as_ref().map(|r| json!({
                        "kind": r.kind.as_str(),
                        "preview": r.preview,
                    })),
                })
            })
            .collect();
        let exit = OutputDoc::new(
            format!("audit grep '{pat}': {hits} match(es)"),
            json!({ "matches": arr }),
        )
        .with_meta("count", hits)
        .with_meta("order", "newest_first")
        .with_meta("pattern", pat)
        .print_json(format.select_spec())?;
        // Preserve the "no matches → exit 1" semantic: even if the
        // filter swallows everything, the underlying-data hits-count
        // signal still applies.
        return Ok(if hits > 0 && matches!(exit, ExitKind::Success) {
            ExitKind::Success
        } else {
            ExitKind::NoMatches
        });
    } else {
        for e in &matches {
            r.data_line(format!(
                "{} {} {} {}",
                e.id,
                e.ts.format("%Y-%m-%d %H:%M:%S"),
                e.verb,
                e.selector
            ));
        }
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
fn verify(
    entries: &[crate::safety::AuditEntry],
    format: &crate::format::FormatArgs,
) -> Result<ExitKind> {
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
    if format.is_json() {
        // v0.1.3 envelope discipline.
        let summary = format!(
            "audit verify: {} entries, {checked} with snapshots, {} missing, {} mismatched",
            entries.len(),
            missing.len(),
            mismatched.len()
        );
        let exit = OutputDoc::new(
            summary,
            json!({
                "entries_total": entries.len(),
                "entries_with_snapshot": checked,
                "missing_count": missing.len(),
                "mismatched_count": mismatched.len(),
                "missing_ids": missing.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
                "mismatched_ids": mismatched.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
                "ok": bad == 0,
            }),
        )
        .print_json(format.select_spec())?;
        // The verify-result exit class (Error if any snapshot mismatched
        // or missing) takes precedence over filter-class exit codes.
        return Ok(if bad != 0 { ExitKind::Error } else { exit });
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
        return emit_gc_json(&report, &args.format);
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

fn emit_gc_json(r: &GcReport, format: &crate::format::FormatArgs) -> Result<ExitKind> {
    // v0.1.3 envelope discipline.
    let prefix = if r.dry_run { "would delete" } else { "deleted" };
    let summary = format!(
        "audit gc (--keep {}): {prefix} {} entries / {} snapshots / {} bytes; {} entries kept",
        r.policy,
        r.deleted_entries,
        r.deleted_snapshots,
        format_bytes(r.freed_bytes),
        r.entries_kept,
    );
    OutputDoc::new(
        summary,
        json!({
            "dry_run": r.dry_run,
            "policy": r.policy,
            "entries_total": r.entries_total,
            "entries_kept": r.entries_kept,
            "deleted_entries": r.deleted_entries,
            "deleted_snapshots": r.deleted_snapshots,
            "freed_bytes": r.freed_bytes,
            "deleted_ids": r.deleted_ids,
            "deleted_snapshot_hashes": r.deleted_snapshot_hashes,
        }),
    )
    .print_json(format.select_spec())
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
