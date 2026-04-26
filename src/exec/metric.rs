//! Metric query execution: range aggregations + vector aggregations.
//!
//! Bible §9.7. Range aggregations evaluate a log query, gather records,
//! then reduce them per (server,service,...) into a single numeric
//! sample. Vector aggregations group/reduce those samples further.
//!
//! Output is one [`MetricSample`] per resulting series.

use std::collections::BTreeMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::exec::ExecCtx;
use crate::logql::ast::{
    AggFn, Grouping, GroupingMode, MetricQuery, RangeAggregation, RangeFn, VectorAggregation,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricSample {
    pub labels: BTreeMap<String, String>,
    pub value: f64,
}

pub fn execute(ctx: &ExecCtx<'_>, m: &MetricQuery) -> Result<Vec<MetricSample>> {
    match m {
        MetricQuery::Range(r) => execute_range(ctx, r),
        MetricQuery::Vector(v) => execute_vector(ctx, v),
    }
}

fn execute_range(ctx: &ExecCtx<'_>, r: &RangeAggregation) -> Result<Vec<MetricSample>> {
    let inner_src = &ctx.source[r.inner.span.clone()];
    let result = crate::exec::engine::execute_log(ctx, inner_src)?;
    // Group by all addressing labels (server, service, source) — that's
    // the natural series key. Stages can have rewritten labels by now,
    // so we use whatever labels survived.
    let groups = group_by_all_labels(&result.records);
    let mut samples = Vec::new();
    let range_secs = (r.range_ms as f64) / 1000.0;
    for (labels, recs) in groups {
        let count = recs.len() as f64;
        let bytes = recs
            .iter()
            .map(|x| x.line.as_deref().map(|s| s.len()).unwrap_or(0) as f64)
            .sum::<f64>();
        let value = match r.func {
            RangeFn::CountOverTime => count,
            RangeFn::Rate => {
                if range_secs > 0.0 {
                    count / range_secs
                } else {
                    count
                }
            }
            RangeFn::BytesOverTime => bytes,
            RangeFn::BytesRate => {
                if range_secs > 0.0 {
                    bytes / range_secs
                } else {
                    bytes
                }
            }
            RangeFn::AbsentOverTime => {
                if recs.is_empty() {
                    1.0
                } else {
                    0.0
                }
            }
        };
        samples.push(MetricSample { labels, value });
    }
    Ok(samples)
}

fn execute_vector(ctx: &ExecCtx<'_>, v: &VectorAggregation) -> Result<Vec<MetricSample>> {
    let inner_samples = execute(ctx, &v.inner)?;
    // Apply grouping: collapse labels not in `by` (or all but `without` set).
    let grouped = group_samples(&inner_samples, v.grouping.as_ref());

    let mut out = Vec::with_capacity(grouped.len());
    for (labels, vals) in grouped {
        let value = match v.func {
            AggFn::Sum => vals.iter().sum(),
            AggFn::Avg => {
                if vals.is_empty() {
                    0.0
                } else {
                    vals.iter().sum::<f64>() / vals.len() as f64
                }
            }
            AggFn::Min => vals.iter().cloned().fold(f64::INFINITY, f64::min),
            AggFn::Max => vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            AggFn::Count => vals.len() as f64,
            AggFn::Stddev => stddev(&vals),
            AggFn::Stdvar => {
                let s = stddev(&vals);
                s * s
            }
            AggFn::Topk | AggFn::Bottomk => {
                // For topk/bottomk we don't aggregate inside the group;
                // we keep individual samples and rank globally below.
                continue;
            }
        };
        out.push(MetricSample { labels, value });
    }

    if matches!(v.func, AggFn::Topk | AggFn::Bottomk) {
        let mut ranked = inner_samples.clone();
        ranked.sort_by(|a, b| {
            if matches!(v.func, AggFn::Topk) {
                b.value.partial_cmp(&a.value).unwrap_or(std::cmp::Ordering::Equal)
            } else {
                a.value.partial_cmp(&b.value).unwrap_or(std::cmp::Ordering::Equal)
            }
        });
        let k = v.param.unwrap_or(0).max(0) as usize;
        ranked.truncate(k);
        // If a grouping was supplied, re-key the samples to that grouping.
        if let Some(g) = &v.grouping {
            for s in &mut ranked {
                s.labels = filter_labels(&s.labels, g);
            }
        }
        return Ok(ranked);
    }

    Ok(out)
}

fn group_by_all_labels(
    records: &[crate::exec::record::Record],
) -> Vec<(BTreeMap<String, String>, Vec<&crate::exec::record::Record>)> {
    let mut buckets: BTreeMap<Vec<(String, String)>, Vec<&crate::exec::record::Record>> =
        BTreeMap::new();
    for r in records {
        // Bible §9.7: parsed fields participate in metric grouping just
        // like labels (Loki "parsed labels"). Promote them here so that
        // `sum by (status)` after `| json` finds the right keys.
        let mut merged: BTreeMap<String, String> = r.labels.clone();
        for (k, v) in &r.fields {
            // Don't shadow a reserved label that's already set.
            if !merged.contains_key(k) {
                merged.insert(k.clone(), crate::exec::record::value_as_string(v));
            }
        }
        let key: Vec<(String, String)> = merged.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        buckets.entry(key).or_default().push(r);
    }
    buckets
        .into_iter()
        .map(|(k, v)| (k.into_iter().collect(), v))
        .collect()
}

fn group_samples(
    samples: &[MetricSample],
    grouping: Option<&Grouping>,
) -> Vec<(BTreeMap<String, String>, Vec<f64>)> {
    let mut buckets: BTreeMap<Vec<(String, String)>, Vec<f64>> = BTreeMap::new();
    for s in samples {
        let labels = match grouping {
            Some(g) => filter_labels(&s.labels, g),
            None => BTreeMap::new(), // no grouping: collapse to a single series
        };
        let key: Vec<(String, String)> =
            labels.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        buckets.entry(key).or_default().push(s.value);
    }
    buckets
        .into_iter()
        .map(|(k, v)| (k.into_iter().collect(), v))
        .collect()
}

fn filter_labels(
    labels: &BTreeMap<String, String>,
    g: &Grouping,
) -> BTreeMap<String, String> {
    match g.mode {
        GroupingMode::By => labels
            .iter()
            .filter(|(k, _)| g.labels.iter().any(|l| l == *k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        GroupingMode::Without => labels
            .iter()
            .filter(|(k, _)| !g.labels.iter().any(|l| l == *k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    }
}

fn stddev(vals: &[f64]) -> f64 {
    if vals.is_empty() {
        return 0.0;
    }
    let mean = vals.iter().sum::<f64>() / vals.len() as f64;
    let var = vals.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / vals.len() as f64;
    var.sqrt()
}
