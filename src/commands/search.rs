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
        crate::error::emit("empty query");
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
        // Field pitfall §5.2: an unbounded `inspect search '...'`
        // query against a chatty cluster can saturate SSH and OOM
        // the local process before any output appears. Apply a
        // best-effort cap (default 100k records, override via
        // `INSPECT_MAX_RECORDS=N`, set to 0 to disable). An
        // explicit `--tail N` always wins so power users keep the
        // ergonomics they expect.
        record_limit: args.tail.unwrap_or_else(default_record_limit),
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
                crate::error::emit("{e}");
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
    let cap = args.tail.unwrap_or_else(default_record_limit);
    let truncated = cap > 0 && records.len() >= cap;
    println!(
        "SUMMARY: {}{} record(s) from `{}`",
        if truncated { "first " } else { "" },
        records.len(),
        truncate(&args.query, 80)
    );
    println!("DATA:");
    // L7 (v0.1.3): redact lines before printing. PEM tracking is
    // per-search-invocation; records are k-way merged across servers
    // by the upstream pipeline, so a multi-line PEM body that crosses
    // server boundaries is rare — but if a single server's records
    // contain one, the BEGIN/body/END lines arrive contiguously
    // because they came from the same source verb (logs / file).
    let redactor = crate::redact::OutputRedactor::new(args.show_secrets, false);
    for r in records {
        let server = r.label("server").unwrap_or("?");
        let service = r.label("service").unwrap_or("_");
        let source = r.label("source").unwrap_or("?");
        match &r.line {
            Some(l) => match redactor.mask_line(l) {
                Some(masked) => println!("  {server}/{service} [{source}] {masked}"),
                None => continue,
            },
            None => println!("  {server}/{service} [{source}]"),
        }
    }
    println!("NEXT:");
    println!(
        "  inspect search '{}' --json   (machine-readable)",
        args.query
    );
    if args.tail.is_none() {
        println!("  inspect search '{}' --tail 50", args.query);
    }
    if truncated {
        // Field pitfall §5.2: be loud about the cap so operators know
        // their result was bounded -- silent truncation is the
        // failure mode the doc warns about.
        println!(
            "  # NOTE: result capped at {cap} records (INSPECT_MAX_RECORDS); \
             pass `--tail N` for an explicit bound or `INSPECT_MAX_RECORDS=0` to disable"
        );
    }
}

/// Field pitfall §5.2: default upper bound on records returned by a
/// single `inspect search` invocation. Operators can override via
/// `INSPECT_MAX_RECORDS=N` (0 disables the cap entirely).
fn default_record_limit() -> usize {
    match std::env::var("INSPECT_MAX_RECORDS") {
        Ok(s) => s.parse::<usize>().unwrap_or(100_000),
        Err(_) => 100_000,
    }
}

fn emit_log_json(args: &SearchArgs, records: &[exec::Record]) {
    // L7 (v0.1.3): redact lines on the JSON path too, so consumers
    // (LLM agents, jq pipelines, log shippers) never see verbatim
    // secrets. PEM-block lines are dropped from the record stream
    // entirely (the BEGIN-line marker stays).
    let redactor = crate::redact::OutputRedactor::new(args.show_secrets, false);
    let data: Vec<Value> = records
        .iter()
        .filter_map(|r| {
            // `None` outer Option means "drop record" (PEM-interior
            // line — emitting an empty `line: ""` would silently
            // discard the L7 contract). `Some(None)` means the
            // record had no line to begin with — emit it as-is.
            let masked_line: Option<Option<String>> = match &r.line {
                None => Some(None),
                Some(l) => redactor.mask_line(l).map(|m| Some(m.into_owned())),
            };
            let masked_line = masked_line?;
            let source = r.label("source").unwrap_or("");
            let medium = source.split(':').next().unwrap_or("");
            Some(json!({
                "_source": source,
                "_medium": medium,
                "labels": r.labels,
                "fields": r.fields,
                "line": masked_line,
                "ts_ms": r.ts_ms,
            }))
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
    let next: Vec<Value> =
        crate::verbs::correlation::search_rules(server_hint.as_deref(), &services)
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
    println!(
        "  inspect search '{}' --json   (machine-readable)",
        args.query
    );
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
