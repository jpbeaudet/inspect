//! `inspect search '<query>'` — Phase 6 surface (parse + diagnose).
//!
//! Phase 6 ships parsing only. The verb validates the query against the
//! LogQL grammar (bible §9), expands aliases, prints a structured
//! preview of the resolved query, and exits. Phase 7 plugs in the
//! actual execution backends.

use anyhow::Result;
use serde_json::json;

use crate::alias;
use crate::cli::SearchArgs;
use crate::error::ExitKind;
use crate::logql;

pub fn run(args: SearchArgs) -> Result<ExitKind> {
    let query = args.query.trim();
    if query.is_empty() {
        eprintln!("error: empty query");
        eprintln!("hint: pass a LogQL query, e.g. inspect search '{{server=\"arte\", source=\"logs\"}} |= \"error\"'");
        return Ok(ExitKind::Error);
    }

    let parsed = logql::parse_with_aliases(query, |name| {
        alias::get(name).ok().flatten().map(|e| e.selector)
    });

    match parsed {
        Ok(ast) => {
            if args.json {
                emit_json(&args, &ast);
            } else {
                emit_human(&args, &ast);
            }
            Ok(ExitKind::Success)
        }
        Err(e) => {
            if args.json {
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
            Ok(ExitKind::Error)
        }
    }
}

fn emit_human(args: &SearchArgs, ast: &logql::Query) {
    let kind = if ast.is_metric() { "metric" } else { "log" };
    println!(
        "SUMMARY: parsed {} query OK ({} branch(es), {} stage(s))",
        kind,
        selector_branch_count(ast),
        stage_count(ast),
    );
    println!("DATA:");
    println!("  query: {}", args.query);
    if let Some(since) = &args.since {
        println!("  since: {since}");
    }
    if let Some(until) = &args.until {
        println!("  until: {until}");
    }
    if let Some(tail) = args.tail {
        println!("  tail:  {tail}");
    }
    if args.follow {
        println!("  follow: true");
    }
    println!("NEXT:");
    println!("  inspect search '{}' --json   (machine-readable parse tree)", args.query);
    println!("  (execution backend lands in Phase 7)");
}

fn emit_json(args: &SearchArgs, ast: &logql::Query) {
    let env = json!({
        "schema_version": 1,
        "summary": format!(
            "parsed {} query OK",
            if ast.is_metric() { "metric" } else { "log" }
        ),
        "data": {
            "kind": if ast.is_metric() { "metric" } else { "log" },
            "branches": selector_branch_count(ast),
            "stages": stage_count(ast),
            "query": args.query,
        },
        "next": [
            { "cmd": "inspect search '...' --since 5m", "rationale": "narrow time window" },
        ],
        "meta": {
            "since": args.since,
            "until": args.until,
            "tail":  args.tail,
            "follow": args.follow,
            "phase": 6
        }
    });
    println!("{env}");
}

fn selector_branch_count(q: &logql::Query) -> usize {
    match q {
        logql::Query::Log(l) => l.selector.branches.len(),
        logql::Query::Metric(_) => 1,
    }
}
fn stage_count(q: &logql::Query) -> usize {
    match q {
        logql::Query::Log(l) => l.pipeline.len(),
        logql::Query::Metric(_) => 0,
    }
}
