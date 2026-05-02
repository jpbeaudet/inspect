//! F18 (v0.1.3): `inspect history show|list|clear|rotate`.
//!
//! Management surface for the per-namespace per-day transcript files
//! at `~/.inspect/history/<ns>-<YYYY-MM-DD>.log[.gz]`. The transcript
//! itself is written by `src/transcript.rs`; this module reads it
//! back, filters fenced blocks, lists files, clears old ones, and
//! invokes the rotation pass.

use anyhow::Result;
use chrono::NaiveDate;

use crate::cli::{HistoryArgs, HistoryCommand};
use crate::error::ExitKind;
use crate::transcript::history_dir;
use crate::transcript::rotate::{list_transcripts, read_transcript, run_rotate, RotateReport};
use crate::verbs::output::{Envelope, JsonOut, Renderer};

pub fn run(args: HistoryArgs) -> Result<ExitKind> {
    match args.command {
        HistoryCommand::Show(o) => show(&o),
        HistoryCommand::List(o) => list(&o),
        HistoryCommand::Clear(o) => clear(&o),
        HistoryCommand::Rotate(o) => rotate(&o),
    }
}

fn show(o: &crate::cli::HistoryShowArgs) -> Result<ExitKind> {
    let dir = history_dir();
    let target_date = match o.date.as_deref() {
        Some(d) => match NaiveDate::parse_from_str(d, "%Y-%m-%d") {
            Ok(date) => Some(date),
            Err(_) => {
                crate::error::emit(format!(
                    "--date '{d}': expected YYYY-MM-DD\nhint: see 'inspect history --help'"
                ));
                return Ok(ExitKind::Error);
            }
        },
        None => None,
    };
    let entries = list_transcripts(&dir)?;
    let mut candidates: Vec<&_> = entries
        .iter()
        .filter(|e| match (&o.namespace, target_date) {
            (Some(ns), Some(d)) => e.namespace == *ns && e.date == d,
            (Some(ns), None) => e.namespace == *ns,
            (None, Some(d)) => e.date == d,
            (None, None) => true,
        })
        .collect();
    if candidates.is_empty() {
        let mut r = Renderer::new();
        let scope = match (&o.namespace, target_date) {
            (Some(ns), Some(d)) => format!("namespace '{ns}' on {d}"),
            (Some(ns), None) => format!("namespace '{ns}'"),
            (None, Some(d)) => format!("date {d}"),
            (None, None) => "any namespace or date".into(),
        };
        r.summary(format!("history show: no transcript files for {scope}"));
        r.next("inspect history list                     # see available files");
        r.print();
        return Ok(ExitKind::NoMatches);
    }
    // Newest-first ordering so `inspect history show <ns>` lands on
    // today's file by default.
    candidates.sort_by_key(|e| std::cmp::Reverse(e.date));
    if target_date.is_none() && o.namespace.is_some() {
        candidates.truncate(1);
    }

    let needle = o.grep.as_deref();
    let audit_filter = o.audit_id.as_deref();
    let json = o.format.is_json();

    let mut hit_blocks = 0usize;
    let mut json_emitted = false;
    let mut human_buf: Vec<String> = Vec::new();

    for entry in &candidates {
        let bytes = read_transcript(&entry.path)?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        for block in iter_blocks(&text) {
            if let Some(needle) = needle {
                if !block.full.contains(needle) {
                    continue;
                }
            }
            if let Some(want_id) = audit_filter {
                match &block.audit_id {
                    Some(id) if id.contains(want_id) => {}
                    _ => continue,
                }
            }
            hit_blocks += 1;
            if json {
                JsonOut::write(
                    &Envelope::new("local", "history", "history.show")
                        .put("namespace", entry.namespace.clone())
                        .put("date", entry.date.format("%Y-%m-%d").to_string())
                        .put("verb_token", block.verb_token.clone())
                        .put("started_at", block.header_ts.clone())
                        .put("argv_line", block.argv_line.clone())
                        .put("body", block.body.clone())
                        .put(
                            "audit_id",
                            block
                                .audit_id
                                .clone()
                                .map(serde_json::Value::String)
                                .unwrap_or(serde_json::Value::Null),
                        )
                        .put(
                            "exit",
                            block
                                .exit
                                .map(|n| serde_json::Value::Number(n.into()))
                                .unwrap_or(serde_json::Value::Null),
                        )
                        .put(
                            "duration_ms",
                            block
                                .duration_ms
                                .map(|n| serde_json::Value::Number(n.into()))
                                .unwrap_or(serde_json::Value::Null),
                        ),
                );
                json_emitted = true;
            } else {
                human_buf.push(block.full.clone());
            }
        }
    }

    if !json {
        if hit_blocks == 0 {
            let mut r = Renderer::new();
            let scope = match (&o.namespace, target_date) {
                (Some(ns), Some(d)) => format!("namespace '{ns}' on {d}"),
                (Some(ns), None) => format!("namespace '{ns}'"),
                (None, Some(d)) => format!("date {d}"),
                (None, None) => "any namespace or date".into(),
            };
            let what = match (needle, audit_filter) {
                (Some(p), _) => format!("--grep '{p}' against {scope}"),
                (None, Some(id)) => format!("--audit-id '{id}' against {scope}"),
                (None, None) => format!("transcripts under {scope}"),
            };
            r.summary(format!("history show: 0 blocks match {what}"));
            r.print();
            return Ok(ExitKind::NoMatches);
        }
        // Print blocks verbatim (already include the fence delimiters,
        // argv line, body, and footer). No SUMMARY/NEXT envelope —
        // operators expect raw transcript output for `less` / `grep`.
        for block in &human_buf {
            print!("{block}");
            // Each block already terminates with "\n\n" so successive
            // blocks render with one blank line between them.
        }
    }
    let _ = json_emitted; // explicit-side-effect tracker; no NEXT for json
    Ok(ExitKind::Success)
}

fn list(o: &crate::cli::HistoryListArgs) -> Result<ExitKind> {
    let dir = history_dir();
    let mut entries = list_transcripts(&dir)?;
    if let Some(ns) = &o.namespace {
        entries.retain(|e| &e.namespace == ns);
    }
    entries.sort_by(|a, b| a.namespace.cmp(&b.namespace).then(b.date.cmp(&a.date)));

    if o.format.is_json() {
        for e in &entries {
            let bytes = std::fs::metadata(&e.path).map(|m| m.len()).unwrap_or(0);
            JsonOut::write(
                &Envelope::new("local", "history", "history.list")
                    .put("namespace", e.namespace.clone())
                    .put("date", e.date.format("%Y-%m-%d").to_string())
                    .put("compressed", e.compressed)
                    .put("bytes", bytes)
                    .put(
                        "path",
                        e.path
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or("")
                            .to_string(),
                    ),
            );
        }
        return Ok(ExitKind::Success);
    }
    let mut r = Renderer::new();
    let total: u64 = entries
        .iter()
        .map(|e| std::fs::metadata(&e.path).map(|m| m.len()).unwrap_or(0))
        .sum();
    let header = match &o.namespace {
        Some(ns) => format!(
            "history list ({} transcript file(s) for '{ns}', {} total)",
            entries.len(),
            format_bytes(total)
        ),
        None => format!(
            "history list ({} transcript file(s) across {} namespace(s), {} total)",
            entries.len(),
            entries
                .iter()
                .map(|e| &e.namespace)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            format_bytes(total),
        ),
    };
    r.summary(header);
    for e in &entries {
        let bytes = std::fs::metadata(&e.path).map(|m| m.len()).unwrap_or(0);
        let suffix = if e.compressed { " (gz)" } else { "" };
        r.data_line(format!(
            "{ns}  {date}  {bytes:>10}  {name}{suffix}",
            ns = e.namespace,
            date = e.date.format("%Y-%m-%d"),
            bytes = format_bytes(bytes),
            name = e.path.file_name().and_then(|s| s.to_str()).unwrap_or("?"),
        ));
    }
    if entries.is_empty() {
        r.data_line("no transcript files yet — run an `inspect <verb>` to create one");
    }
    r.next("inspect history show <ns>      # render today's transcript");
    r.next("inspect history rotate         # apply [history] retention now");
    r.print();
    Ok(ExitKind::Success)
}

fn clear(o: &crate::cli::HistoryClearArgs) -> Result<ExitKind> {
    let cutoff = match NaiveDate::parse_from_str(&o.before, "%Y-%m-%d") {
        Ok(d) => d,
        Err(_) => {
            crate::error::emit(format!(
                "--before '{}': expected YYYY-MM-DD\nhint: see 'inspect history --help'",
                o.before
            ));
            return Ok(ExitKind::Error);
        }
    };
    let dir = history_dir();
    let entries = list_transcripts(&dir)?;
    let to_delete: Vec<_> = entries
        .into_iter()
        .filter(|e| e.namespace == o.namespace && e.date < cutoff)
        .collect();
    if to_delete.is_empty() {
        let mut r = Renderer::new();
        r.summary(format!(
            "history clear {}: no transcript files older than {cutoff} (nothing to delete)",
            o.namespace
        ));
        r.print();
        return Ok(ExitKind::Success);
    }
    let total_bytes: u64 = to_delete
        .iter()
        .map(|e| std::fs::metadata(&e.path).map(|m| m.len()).unwrap_or(0))
        .sum();

    if !o.yes {
        eprintln!(
            "history clear {}: would delete {} file(s) ({}) older than {cutoff}.",
            o.namespace,
            to_delete.len(),
            format_bytes(total_bytes)
        );
        eprintln!("hint: rerun with --yes to confirm.");
        return Ok(ExitKind::Error);
    }
    let mut deleted_paths: Vec<String> = Vec::new();
    let mut deleted_bytes: u64 = 0;
    for e in &to_delete {
        let len = std::fs::metadata(&e.path).map(|m| m.len()).unwrap_or(0);
        if std::fs::remove_file(&e.path).is_ok() {
            deleted_bytes = deleted_bytes.saturating_add(len);
            if let Some(name) = e.path.file_name().and_then(|s| s.to_str()) {
                deleted_paths.push(name.to_string());
            }
        }
    }
    if o.format.is_json() {
        JsonOut::write(
            &Envelope::new("local", "history", "history.clear")
                .put("namespace", o.namespace.clone())
                .put("before", o.before.clone())
                .put("deleted_count", deleted_paths.len())
                .put("deleted_bytes", deleted_bytes)
                .put("deleted_files", deleted_paths.clone()),
        );
        return Ok(ExitKind::Success);
    }
    let mut r = Renderer::new();
    r.summary(format!(
        "history clear {ns}: deleted {n} file(s), freed {bytes}",
        ns = o.namespace,
        n = deleted_paths.len(),
        bytes = format_bytes(deleted_bytes),
    ));
    for name in deleted_paths.iter().take(10) {
        r.data_line(format!("deleted: {name}"));
    }
    if deleted_paths.len() > 10 {
        r.data_line(format!(
            "  … and {} more files (use --json for the full list)",
            deleted_paths.len() - 10
        ));
    }
    r.print();
    Ok(ExitKind::Success)
}

fn rotate(o: &crate::cli::HistoryRotateArgs) -> Result<ExitKind> {
    let report = run_rotate(None)?;
    if o.format.is_json() {
        emit_rotate_json(&report);
        return Ok(ExitKind::Success);
    }
    render_rotate_human(&report);
    Ok(ExitKind::Success)
}

fn emit_rotate_json(r: &RotateReport) {
    JsonOut::write(
        &Envelope::new("local", "history", "history.rotate")
            .put("deleted_count", r.deleted_count)
            .put("deleted_bytes", r.deleted_bytes)
            .put("compressed_count", r.compressed_count)
            .put("compressed_bytes_before", r.compressed_bytes_before)
            .put("compressed_bytes_after", r.compressed_bytes_after)
            .put("evicted_count", r.evicted_count)
            .put("evicted_bytes", r.evicted_bytes)
            .put("total_bytes_after", r.total_bytes_after)
            .put("deleted_files", r.deleted_files.clone())
            .put("compressed_files", r.compressed_files.clone())
            .put("evicted_files", r.evicted_files.clone()),
    );
}

fn render_rotate_human(r: &RotateReport) {
    let mut renderer = Renderer::new();
    renderer.summary(format!(
        "history rotate: deleted {} (freed {}), gzipped {} ({} → {}), evicted {} ({}); total now {}",
        r.deleted_count,
        format_bytes(r.deleted_bytes),
        r.compressed_count,
        format_bytes(r.compressed_bytes_before),
        format_bytes(r.compressed_bytes_after),
        r.evicted_count,
        format_bytes(r.evicted_bytes),
        format_bytes(r.total_bytes_after),
    ));
    if r.deleted_count == 0 && r.compressed_count == 0 && r.evicted_count == 0 {
        renderer.data_line("nothing to do (every file is within the retention window)");
    } else {
        for name in r.deleted_files.iter().take(5) {
            renderer.data_line(format!("deleted:    {name}"));
        }
        for name in r.compressed_files.iter().take(5) {
            renderer.data_line(format!("compressed: {name}"));
        }
        for name in r.evicted_files.iter().take(5) {
            renderer.data_line(format!("evicted:    {name} (cap exceeded)"));
        }
    }
    renderer.print();
}

fn format_bytes(b: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if b >= GB {
        format!("{:.2} GiB", b as f64 / GB as f64)
    } else if b >= MB {
        format!("{:.2} MiB", b as f64 / MB as f64)
    } else if b >= KB {
        format!("{:.2} KiB", b as f64 / KB as f64)
    } else {
        format!("{b} B")
    }
}

#[derive(Debug, Clone)]
struct Block {
    full: String,
    header_ts: String,
    verb_token: String,
    argv_line: String,
    body: String,
    audit_id: Option<String>,
    exit: Option<i64>,
    duration_ms: Option<i64>,
}

/// Parse a transcript file's bytes into ordered fenced blocks.
fn iter_blocks(text: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut iter = text.split_inclusive('\n').peekable();
    let mut current: Option<(String, String, String, Vec<String>)> = None; // (header_ts, verb_token, argv_line_pending, body_lines)

    while let Some(line) = iter.next() {
        let trimmed = line.trim_end_matches('\n');
        // Header: `── <ts> <ns> <token> ──...`
        if current.is_none() && trimmed.starts_with("── ") && !trimmed.starts_with("── exit=")
        {
            // Parse: "── <ts> <ns> <token> ─...─"
            let inner = trimmed.trim_start_matches("── ");
            let inner = inner.trim_end_matches('─').trim_end();
            let mut parts = inner.split_whitespace();
            let ts = parts.next().unwrap_or("").to_string();
            let _ns = parts.next().unwrap_or("");
            let tok = parts.next().unwrap_or("").to_string();
            current = Some((ts, tok, String::new(), vec![line.to_string()]));
            // Next line should be the argv
            if let Some(next) = iter.peek() {
                if next.starts_with("$ ") {
                    let argv = iter.next().unwrap();
                    if let Some(c) = current.as_mut() {
                        c.2 = argv.trim_end_matches('\n').to_string();
                        c.3.push(argv.to_string());
                    }
                }
            }
            continue;
        }
        // Footer: `── exit=N duration=Mms [audit_id=ID] ──`
        if let Some(c) = current.as_mut() {
            c.3.push(line.to_string());
            if trimmed.starts_with("── exit=") && trimmed.ends_with("──") {
                let mut audit_id: Option<String> = None;
                let mut exit_v: Option<i64> = None;
                let mut dur_v: Option<i64> = None;
                for token in trimmed.split_whitespace() {
                    if let Some(rest) = token.strip_prefix("exit=") {
                        exit_v = rest.parse().ok();
                    } else if let Some(rest) = token.strip_prefix("duration=") {
                        if let Some(num) = rest.strip_suffix("ms") {
                            dur_v = num.parse().ok();
                        }
                    } else if let Some(rest) = token.strip_prefix("audit_id=") {
                        audit_id = Some(rest.to_string());
                    }
                }
                let body =
                    c.3.iter()
                        .skip(2) // header + argv
                        .take(c.3.len().saturating_sub(3)) // exclude the final footer line
                        .cloned()
                        .collect::<String>();
                let full = c.3.join("");
                blocks.push(Block {
                    full,
                    header_ts: c.0.clone(),
                    verb_token: c.1.clone(),
                    argv_line: c.2.clone(),
                    body,
                    audit_id,
                    exit: exit_v,
                    duration_ms: dur_v,
                });
                current = None;
            }
        }
    }

    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iter_blocks_parses_one_block() {
        let text = "\
── 2026-04-28T14:32:11Z arte #b8e3a1 ──────────────────────────
$ inspect run arte -- 'docker ps'
arte | atlas-vault
arte | atlas-pg
── exit=0 duration=423ms audit_id=01HXR9Q5YQK2 ──

";
        let blocks = iter_blocks(text);
        assert_eq!(blocks.len(), 1);
        let b = &blocks[0];
        assert_eq!(b.exit, Some(0));
        assert_eq!(b.duration_ms, Some(423));
        assert_eq!(b.audit_id.as_deref(), Some("01HXR9Q5YQK2"));
        assert_eq!(b.verb_token, "#b8e3a1");
        assert!(b.argv_line.contains("docker ps"));
        assert!(b.body.contains("atlas-vault"));
    }

    #[test]
    fn iter_blocks_handles_no_audit_id() {
        let text = "\
── 2026-04-28T14:32:11Z arte #b8e3a1 ──────────────────────────
$ inspect status arte
arte | atlas-vault: ok
── exit=0 duration=12ms ──

";
        let blocks = iter_blocks(text);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].audit_id.is_none());
        assert_eq!(blocks[0].exit, Some(0));
    }

    #[test]
    fn iter_blocks_parses_multiple_blocks_in_order() {
        let text = "\
── 2026-04-28T14:32:11Z arte #aaa1111 ──────────────────────────
$ inspect status arte
ok
── exit=0 duration=10ms ──

── 2026-04-28T14:32:15Z arte #bbb2222 ──────────────────────────
$ inspect run arte -- 'true'
── exit=0 duration=20ms audit_id=01HXR9XXX ──

";
        let blocks = iter_blocks(text);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].verb_token, "#aaa1111");
        assert_eq!(blocks[1].verb_token, "#bbb2222");
        assert_eq!(blocks[1].audit_id.as_deref(), Some("01HXR9XXX"));
    }
}
