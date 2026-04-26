//! `inspect search '<query>'` — Phase 7 surface.
//!
//! Parses the LogQL query, expands aliases, dispatches to medium readers,
//! runs the pipeline, and emits SUMMARY/DATA/NEXT (human) or a stable
//! JSON envelope (machine).

use anyhow::Result;
use serde_json::{json, Value};

use crate::alias;
use crate::cli::SearchArgs;
use crate::error::ExitKind;
use crate::exec::{self, ExecOutput};
use crate::logql;

pub fn run(args: SearchArgs) -> Result<ExitKind> {
    let query = args.query.trim();
    if query.is_empty() {
        eprintln!("error: empty query");
        eprintln!("hint: pass a LogQL query, e.g. inspect search '{{server=\"arte\", source=\"logs\"}} |= \"error\"'");
        return Ok(ExitKind::Error);
    }

    // Parse-only first so we can emit precise diagnostics with carets.
    if let Err(e) = logql::parse_with_aliases(query, |name| {
        alias::get(name).ok().flatten().map(|e| e.selector)
    }) {
        if args.format.is_json() {
            let env = json!({
                "schema_version": 1,
                "summary": format!("parse error: {}", e.message),
                "data": {
                    "error": {
                        "message": e.message,
                        "span": [e.span.start, e.span.end],
                        "hint": e.hint,
                    }
                },
                "next": [],
                "meta": {}
            });
            println!("{env}");
        } else {
            eprint!("{}", e.render(query));
        }
        return Ok(ExitKind::Error);
    }

    let opts = exec::ExecOpts {
        since: args.since.clone(),
        until: args.until.clone(),
        tail: args.tail,
        follow: args.follow,
        record_limit: args.tail.unwrap_or(0),
        ..Default::default()
    };

    let result = match exec::execute(query, opts) {
        Ok(out) => out,
        Err(e) => {
            // Cancellation (audit §2.2 + §5.4): if Ctrl+C tripped the
            // global flag, emit an envelope with `summary: "cancelled"`
            // so script consumers always see a terminator on the
            // stream — never a truncated, unwrapped record list.
            let cancelled = exec::cancel::is_cancelled();
            if args.format.is_json() {
                let env = json!({
                    "schema_version": 1,
                    "summary": if cancelled { "cancelled by signal" } else { "execution failed" },
                    "data": {
                        "kind": if cancelled { "cancelled" } else { "error" },
                        "error": { "message": e.to_string() }
                    },
                    "next": [],
                    "meta": { "query": args.query, "cancelled": cancelled }
                });
                println!("{env}");
            } else if cancelled {
                println!("SUMMARY: cancelled by signal");
                println!("DATA:");
                println!("NEXT:");
                println!("  inspect search '{}'   (re-run when ready)", args.query);
            } else {
                eprintln!("error: {e}");
                let mut src = e.source();
                while let Some(c) = src {
                    eprintln!("  caused by: {c}");
                    src = c.source();
                }
            }
            return Ok(ExitKind::Error);
        }
    };

    match result {
        ExecOutput::Log(r) => {
            if args.format.is_json() {
                emit_log_json(&args, &r.records);
            } else {
                emit_log_human(&args, &r.records);
            }
            Ok(if r.records.is_empty() {
                ExitKind::NoMatches
            } else {
                ExitKind::Success
            })
        }
        ExecOutput::Metric(samples) => {
            if args.format.is_json() {
                emit_metric_json(&args, &samples);
            } else {
                emit_metric_human(&args, &samples);
            }
            Ok(if samples.is_empty() {
                ExitKind::NoMatches
            } else {
                ExitKind::Success
            })
        }
    }
}

fn emit_log_human(args: &SearchArgs, records: &[exec::Record]) {
    println!(
        "SUMMARY: {} record(s) from `{}`",
        records.len(),
        truncate(&args.query, 80)
    );
    println!("DATA:");
    for r in records {
        let server = r.label("server").unwrap_or("?");
        let service = r.label("service").unwrap_or("_");
        let source = r.label("source").unwrap_or("?");
        match &r.line {
            Some(l) => println!("  {server}/{service} [{source}] {l}"),
            None => println!("  {server}/{service} [{source}]"),
        }
    }
    println!("NEXT:");
    println!("  inspect search '{}' --json   (machine-readable)", args.query);
    if args.tail.is_none() {
        println!("  inspect search '{}' --tail 50", args.query);
    }
}

fn emit_log_json(args: &SearchArgs, records: &[exec::Record]) {
    let data: Vec<Value> = records
        .iter()
        .map(|r| {
            let source = r.label("source").unwrap_or("");
            let medium = source.split(':').next().unwrap_or("");
            json!({
                "_source": source,
                "_medium": medium,
                "labels": r.labels,
                "fields": r.fields,
                "line": r.line,
                "ts_ms": r.ts_ms,
            })
        })
        .collect();
    // Phase 10 — correlation: dominant-service hint.
    let services: Vec<String> = records
        .iter()
        .filter_map(|r| r.label("service").map(|s| s.to_string()))
        .collect();
    let server_hint = records
        .iter()
        .find_map(|r| r.label("server").map(|s| s.to_string()));
    let next: Vec<Value> = crate::verbs::correlation::search_rules(
        server_hint.as_deref(),
        &services,
    )
    .into_iter()
    .map(|n| json!({"cmd": n.cmd, "rationale": n.rationale}))
    .collect();
    let env = json!({
        "schema_version": 1,
        "summary": format!("{} record(s)", records.len()),
        "data": { "kind": "log", "records": data },
        "next": next,
        "meta": {
            "query": args.query,
            "since": args.since,
            "until": args.until,
            "tail":  args.tail,
            "follow": args.follow,
            "phase": 10
        }
    });
    println!("{env}");
}

fn emit_metric_human(args: &SearchArgs, samples: &[exec::metric::MetricSample]) {
    println!(
        "SUMMARY: {} series from `{}`",
        samples.len(),
        truncate(&args.query, 80)
    );
    println!("DATA:");
    for s in samples {
        let labels: String = s
            .labels
            .iter()
            .map(|(k, v)| format!("{k}=\"{v}\""))
            .collect::<Vec<_>>()
            .join(", ");
        println!("  {{{labels}}} = {}", s.value);
    }
    println!("NEXT:");
    println!("  inspect search '{}' --json   (machine-readable)", args.query);
}

fn emit_metric_json(args: &SearchArgs, samples: &[exec::metric::MetricSample]) {
    let env = json!({
        "schema_version": 1,
        "summary": format!("{} series", samples.len()),
        "data": { "kind": "metric", "samples": samples },
        "next": [],
        "meta": { "query": args.query, "phase": 7 }
    });
    println!("{env}");
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n])
    }
}
