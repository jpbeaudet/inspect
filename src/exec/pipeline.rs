//! Apply a parsed pipeline to a vector of records.

use anyhow::Result;
use regex::Regex;

use crate::exec::field_filter;
use crate::exec::format;
use crate::exec::map_stage;
use crate::exec::parsers;
use crate::exec::record::Record;
use crate::exec::ExecCtx;
use crate::logql::ast::{Filter, FilterOp, PipelineOp, Stage};

/// Apply the full `[filters | stages]*` pipeline of a log query.
pub fn apply(
    ctx: &ExecCtx<'_>,
    pipeline: &[PipelineOp],
    records: Vec<Record>,
) -> Result<Vec<Record>> {
    let mut current = records;
    for op in pipeline {
        // Cancellation checkpoint between pipeline ops (audit §2.2).
        // Every stage is a vector pass; checking once per stage gives
        // sub-second SIGINT response without per-record overhead.
        crate::exec::cancel::check()?;
        match op {
            PipelineOp::Filter(f) => {
                current = apply_line_filter(f, current)?;
            }
            PipelineOp::Stage(s) => {
                current = apply_stage(ctx, s, current)?;
            }
        }
        if ctx.opts.record_limit > 0 && current.len() > ctx.opts.record_limit {
            current.truncate(ctx.opts.record_limit);
        }
    }
    Ok(current)
}

fn apply_line_filter(f: &Filter, recs: Vec<Record>) -> Result<Vec<Record>> {
    let re_holder = if matches!(f.op, FilterOp::Re | FilterOp::Nre) {
        Some(
            Regex::new(&f.pattern)
                .map_err(|e| anyhow::anyhow!("invalid line-filter regex: {e}"))?,
        )
    } else {
        None
    };
    let pred = |line: &str| -> bool {
        match f.op {
            FilterOp::Contains => line.contains(&f.pattern),
            FilterOp::NotContains => !line.contains(&f.pattern),
            FilterOp::Re => re_holder.as_ref().unwrap().is_match(line),
            FilterOp::Nre => !re_holder.as_ref().unwrap().is_match(line),
        }
    };
    Ok(recs
        .into_iter()
        .filter(|r| match r.line.as_deref() {
            Some(line) => pred(line),
            None => false,
        })
        .collect())
}

fn apply_stage(ctx: &ExecCtx<'_>, stage: &Stage, mut recs: Vec<Record>) -> Result<Vec<Record>> {
    match stage {
        Stage::Json => {
            for r in &mut recs {
                parsers::parse_json(r);
            }
            Ok(recs)
        }
        Stage::Logfmt => {
            for r in &mut recs {
                parsers::parse_logfmt(r);
            }
            Ok(recs)
        }
        Stage::Pattern { template, .. } => {
            for r in &mut recs {
                parsers::parse_pattern(r, template).map_err(|e| anyhow::anyhow!(e))?;
            }
            Ok(recs)
        }
        Stage::Regexp { pattern, .. } => {
            for r in &mut recs {
                parsers::parse_regexp(r, pattern).map_err(|e| anyhow::anyhow!(e))?;
            }
            Ok(recs)
        }
        Stage::LineFormat { template, .. } => {
            for r in &mut recs {
                r.line = Some(format::render(template, r));
            }
            Ok(recs)
        }
        Stage::LabelFormat { assignments, .. } => {
            for r in &mut recs {
                for a in assignments {
                    let v = format::render(&a.template, r);
                    r.labels.insert(a.name.clone(), v);
                }
            }
            Ok(recs)
        }
        Stage::Drop { labels, .. } => {
            for r in &mut recs {
                for l in labels {
                    r.labels.remove(l);
                }
            }
            Ok(recs)
        }
        Stage::Keep { labels, .. } => {
            for r in &mut recs {
                r.labels.retain(|k, _| labels.iter().any(|l| l == k));
            }
            Ok(recs)
        }
        Stage::FieldFilter { expr, .. } => Ok(recs
            .into_iter()
            .filter(|r| field_filter::eval(expr, r))
            .collect()),
        Stage::Map { sub, .. } => map_stage::execute(ctx, sub, recs),
    }
}
