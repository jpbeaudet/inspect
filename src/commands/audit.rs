//! `inspect audit ls|show|grep` (bible §8.2).

use anyhow::Result;

use crate::cli::{AuditArgs, AuditCommand};
use crate::error::ExitKind;
use crate::safety::AuditStore;
use crate::verbs::output::{Envelope, JsonOut, Renderer};

pub fn run(args: AuditArgs) -> Result<ExitKind> {
    let store = AuditStore::open()?;
    let entries = store.all()?;
    match args.command {
        AuditCommand::Ls(o) => list(&entries, o.format.is_json(), Some(o.limit)),
        AuditCommand::Show(o) => show(&entries, &o.id, o.format.is_json()),
        AuditCommand::Grep(o) => grep(&entries, &o.pattern, o.format.is_json()),
    }
}

fn list(entries: &[crate::safety::AuditEntry], json: bool, limit: Option<usize>) -> Result<ExitKind> {
    let n = entries.len();
    // Newest first.
    let mut sorted: Vec<_> = entries.iter().collect();
    sorted.sort_by_key(|e| std::cmp::Reverse(e.ts));
    let take = limit.unwrap_or(50).min(sorted.len());
    let view = &sorted[..take];

    if json {
        for e in view {
            JsonOut::write(
                &Envelope::new(&e.host, "audit", "audit")
                    .put("id", e.id.clone())
                    .put("ts", e.ts.to_rfc3339())
                    .put("verb", e.verb.clone())
                    .put("selector", e.selector.clone())
                    .put("exit", e.exit)
                    .put("diff_summary", e.diff_summary.clone())
                    .put("is_revert", e.is_revert),
            );
        }
        return Ok(ExitKind::Success);
    }

    let mut r = Renderer::new();
    r.summary(format!("{n} audit entry/entries (showing {take})"));
    for e in view {
        let badge = if e.exit == 0 { "ok " } else { "ERR" };
        let revert = if e.is_revert { " (revert)" } else { "" };
        r.data_line(format!(
            "{} [{badge}] {} {} {}{revert} — {}",
            e.id,
            e.ts.format("%Y-%m-%d %H:%M:%S"),
            e.verb,
            e.selector,
            if e.diff_summary.is_empty() { "" } else { &e.diff_summary },
        ));
    }
    r.next("inspect audit show <id>");
    r.next("inspect revert <id>");
    r.print();
    Ok(ExitKind::Success)
}

fn show(entries: &[crate::safety::AuditEntry], id_prefix: &str, json: bool) -> Result<ExitKind> {
    let Some(e) = entries.iter().find(|e| e.id.starts_with(id_prefix)) else {
        eprintln!("error: no audit entry matches id prefix '{id_prefix}'");
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
