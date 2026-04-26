//! Top-level execution engine: AST → records / metric samples.
//!
//! Pipeline (bible §9):
//! 1. Parse query (alias substitution → tokenize → parse → validate)
//! 2. For each selector branch in the union:
//!    a. Resolve `source=` to a [`Medium`].
//!    b. Resolve `server`/`service` to concrete (namespace, target) steps.
//!    c. Read records via the medium's [`Reader`].
//!    d. Apply remaining selector matchers (server/service/source
//!    regexes, plus user-defined labels).
//! 3. Concatenate all branch outputs.
//! 4. Apply the pipeline (filters + stages, including `map`).
//! 5. For metric queries: feed the log-query result into
//!    [`crate::exec::metric::execute`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use regex::Regex;

use crate::alias;
use crate::exec::medium::Medium;
use crate::exec::metric;
use crate::exec::pipeline;
use crate::exec::reader::{self, LineFilter, ReadOpts, ReadStep};
use crate::exec::record::Record;
use crate::exec::ExecCtx;
use crate::logql::ast::{
    Filter, FilterOp, LabelMatcher, LogQuery, MatchOp, PipelineOp, Query, Selector,
};
use crate::profile::cache::load_profile;
use crate::profile::schema::{Profile, Service};
use crate::ssh::options::SshTarget;
use crate::verbs::runtime::{current_runner, resolve_target};

pub struct LogResult {
    pub records: Vec<Record>,
}

pub enum ExecOutput {
    Log(LogResult),
    Metric(Vec<crate::exec::metric::MetricSample>),
}

/// Parse and execute a query end-to-end.
pub fn execute(query: &str, opts: crate::exec::ExecOpts) -> Result<ExecOutput> {
    let expanded = crate::logql::expand_aliases(query, |name| {
        alias::get(name).ok().flatten().map(|e| e.selector)
    })?;
    let ast = crate::logql::parse(&expanded)?;
    let runner = current_runner();
    let ctx = ExecCtx {
        source: &expanded,
        opts: &opts,
        runner: runner.as_ref(),
    };
    match ast {
        Query::Log(l) => {
            let recs = run_log(&ctx, &l)?;
            Ok(ExecOutput::Log(LogResult { records: recs }))
        }
        Query::Metric(m) => {
            let samples = metric::execute(&ctx, &m)?;
            Ok(ExecOutput::Metric(samples))
        }
    }
}

/// Execute a log query whose source text is `src`. Used by `map` and
/// metric range aggregations to avoid round-tripping the full
/// alias-expansion + validate pipeline for sub-queries.
pub(crate) fn execute_log(ctx: &ExecCtx<'_>, src: &str) -> Result<LogResult> {
    let ast = crate::logql::parse(src)
        .with_context(|| format!("parsing sub-query `{src}`"))?;
    let l = match ast {
        Query::Log(l) => l,
        Query::Metric(_) => {
            return Err(anyhow!("expected a log query, got a metric query"));
        }
    };
    // Make a fresh sub-context whose `source` matches the new text so
    // span-driven helpers (map_stage, metric range) keep working.
    let sub_ctx = ExecCtx {
        source: src,
        opts: ctx.opts,
        runner: ctx.runner,
    };
    let records = run_log(&sub_ctx, &l)?;
    Ok(LogResult { records })
}

fn run_log(ctx: &ExecCtx<'_>, l: &LogQuery) -> Result<Vec<Record>> {
    // Bible §9.10 — extract leading line filters from the pipeline so
    // readers can push them to remote `grep`. We keep the filters in
    // the pipeline too: re-applying contains/regex on already-filtered
    // records is idempotent, and readers that don't honor pushdown
    // would otherwise return unfiltered data.
    let pushdown_filters = collect_leading_line_filters(&l.pipeline);

    // Run branches in parallel (up to `max_parallel`) — each branch
    // resolves its own targets and reads concurrently across them.
    let branches = &l.selector.branches;
    let results: Result<Vec<Vec<Record>>> = parallel_map(
        ctx.opts.max_parallel,
        branches,
        |branch| run_branch(ctx, branch, &pushdown_filters),
    );
    let mut all: Vec<Record> = results?.into_iter().flatten().collect();

    all = pipeline::apply(ctx, &l.pipeline, all)?;
    if ctx.opts.record_limit > 0 && all.len() > ctx.opts.record_limit {
        all.truncate(ctx.opts.record_limit);
    }
    Ok(all)
}

/// Collect leading `Filter` ops from the pipeline (until the first
/// `Stage`) and convert them into reader-level [`LineFilter`]s. Only
/// runs of contains/regex filters are pushed; once a parsing or format
/// stage appears, line content may be rewritten and no further
/// pushdown is safe.
fn collect_leading_line_filters(pipeline: &[PipelineOp]) -> Vec<LineFilter> {
    let mut out = Vec::new();
    for op in pipeline {
        match op {
            PipelineOp::Filter(f) => out.push(filter_to_line_filter(f)),
            PipelineOp::Stage(_) => break,
        }
    }
    out
}

fn filter_to_line_filter(f: &Filter) -> LineFilter {
    match f.op {
        FilterOp::Contains => LineFilter {
            negated: false,
            regex: false,
            pattern: f.pattern.clone(),
        },
        FilterOp::NotContains => LineFilter {
            negated: true,
            regex: false,
            pattern: f.pattern.clone(),
        },
        FilterOp::Re => LineFilter {
            negated: false,
            regex: true,
            pattern: f.pattern.clone(),
        },
        FilterOp::Nre => LineFilter {
            negated: true,
            regex: true,
            pattern: f.pattern.clone(),
        },
    }
}

/// Run `f` on each input in parallel up to `max` workers, preserving
/// input order in the output. A `max` of 1 (or single input) runs
/// inline to avoid `thread::scope` overhead on the hot path.
fn parallel_map<I, T, R, F>(max: usize, items: I, f: F) -> Result<Vec<R>>
where
    I: IntoIterator<Item = T>,
    T: Send,
    R: Send,
    F: Fn(T) -> Result<R> + Sync,
{
    let items: Vec<T> = items.into_iter().collect();
    let n = items.len();
    if n == 0 {
        return Ok(Vec::new());
    }
    let parallel = max.max(1).min(n);
    if parallel == 1 {
        let mut out = Vec::with_capacity(n);
        for it in items {
            out.push(f(it)?);
        }
        return Ok(out);
    }
    // Slot-based collector preserves input ordering.
    let slots: Vec<Mutex<Option<Result<R>>>> = (0..n).map(|_| Mutex::new(None)).collect();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let f_ref = &f;
    let slots_ref = &slots;
    let next_ref = &next;
    // Move items into a single shared queue (one Option per index).
    let queue: Vec<Mutex<Option<T>>> = items.into_iter().map(|t| Mutex::new(Some(t))).collect();
    let queue_ref: &Vec<Mutex<Option<T>>> = &queue;
    std::thread::scope(|scope| {
        for _ in 0..parallel {
            scope.spawn(move || loop {
                // Cancellation (audit §2.2): wake up at every dequeue so
                // SIGINT lands within one work-item of the user pressing
                // Ctrl+C. Workers that haven't started yet just exit
                // without spawning new SSH children.
                if crate::exec::cancel::is_cancelled() {
                    return;
                }
                let idx = next_ref.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if idx >= n {
                    return;
                }
                let item = queue_ref[idx].lock().unwrap().take();
                if let Some(item) = item {
                    let r = f_ref(item);
                    *slots_ref[idx].lock().unwrap() = Some(r);
                }
            });
        }
    });
    let mut out = Vec::with_capacity(n);
    for slot in slots {
        match slot.into_inner().unwrap() {
            Some(r) => out.push(r?),
            None => {
                // A worker exited early without filling this slot. The
                // only legitimate cause is cancellation — propagate it
                // as a partial-result signal so callers can still emit
                // a `cancelled` envelope with whatever did complete.
                if crate::exec::cancel::is_cancelled() {
                    return Err(anyhow!("cancelled by signal"));
                }
                return Err(anyhow!("internal: worker dropped slot"));
            }
        }
    }
    Ok(out)
}

fn run_branch(
    ctx: &ExecCtx<'_>,
    sel: &Selector,
    line_filters: &[LineFilter],
) -> Result<Vec<Record>> {
    let medium_value = source_value(sel)?;
    let medium = Medium::parse(medium_value)
        .map_err(|e| anyhow!("invalid `source=\"{}\"`: {}", medium_value, e))?;
    let reader_impl: Arc<dyn reader::Reader + Send + Sync> = reader::for_medium_arc(&medium);

    // Resolve namespaces + services using the existing selector engine.
    let plan = build_plan(sel)?;
    let read_opts = ReadOpts {
        since: ctx.opts.since.clone(),
        until: ctx.opts.until.clone(),
        tail: ctx.opts.tail,
        line_filters: line_filters.to_vec(),
    };

    // Compile non-source/non-server/non-service matchers ("user matchers")
    // for in-memory filtering of returned records.
    let user_matchers: Vec<&LabelMatcher> = sel
        .matchers
        .iter()
        .filter(|m| !matches!(m.name.as_str(), "server" | "service" | "source"))
        .collect();
    // Also re-apply source matcher in case it was a regex (the medium
    // dispatch already used the raw value; if it was a regex against
    // `Medium::as_label` we still need to honor it).
    let source_matcher: Option<&LabelMatcher> = sel
        .matchers
        .iter()
        .find(|m| m.name == "source" && matches!(m.op, MatchOp::Re | MatchOp::Nre));

    // Run steps in parallel.
    let medium_label = medium.as_label();
    let steps = plan.steps;
    let per_step_results: Result<Vec<Vec<Record>>> = parallel_map(
        ctx.opts.max_parallel,
        steps.iter(),
        |step| -> Result<Vec<Record>> {
            let svc_def_owned: Option<Service> = step.service.as_ref().and_then(|svc| {
                step.profile
                    .services
                    .iter()
                    .find(|s| &s.name == svc)
                    .cloned()
            });
            let read_step = ReadStep {
                namespace: &step.namespace,
                target: &step.target,
                service: step.service.as_deref(),
                service_def: svc_def_owned.as_ref(),
            };
            reader_impl
                .read(ctx.runner, &read_step, &read_opts)
                .with_context(|| {
                    format!(
                        "reading source={} for {}/{}",
                        medium_label,
                        step.namespace,
                        step.service.as_deref().unwrap_or("_")
                    )
                })
        },
    );

    // Field pitfall §4.1: when fanning out to multiple steps and the
    // user opts in via `INSPECT_PARTIAL_OK=1`, downgrade individual
    // step failures (timeouts on one slow host, transient ssh errors)
    // into stderr warnings so the rest of the fleet still produces
    // results. Single-step queries keep strict propagation so a
    // typo'd selector doesn't silently return zero matches.
    let per_step_results = match per_step_results {
        Ok(v) => v,
        Err(e) => {
            if partial_ok_enabled() && steps.len() > 1 {
                eprintln!(
                    "warning: one or more steps failed and were skipped (INSPECT_PARTIAL_OK=1): {e}"
                );
                Vec::new()
            } else {
                return Err(e);
            }
        }
    };

    let mut out = Vec::new();
    for recs in per_step_results {
        for r in recs {
            if let Some(m) = source_matcher {
                let v = r.label("source").unwrap_or_default();
                if !match_label(m, v) {
                    continue;
                }
            }
            if user_matchers.iter().all(|m| {
                let v = r.label(&m.name).unwrap_or_default();
                match_label(m, v)
            }) {
                out.push(r);
            }
        }
    }
    Ok(out)
}

/// Field pitfall §4.1: opt-in soft-error mode. When set, per-step
/// reader failures inside a multi-step branch become stderr warnings
/// instead of aborting the whole query. Off by default to preserve
/// "errors are loud" behaviour for single-host queries and tests.
fn partial_ok_enabled() -> bool {
    matches!(
        std::env::var("INSPECT_PARTIAL_OK").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

fn match_label(m: &LabelMatcher, value: &str) -> bool {
    match m.op {
        MatchOp::Eq => value == m.value,
        MatchOp::Ne => value != m.value,
        MatchOp::Re => Regex::new(&m.value).map(|r| r.is_match(value)).unwrap_or(false),
        MatchOp::Nre => Regex::new(&m.value).map(|r| !r.is_match(value)).unwrap_or(false),
    }
}

/// The literal `source=` value (or the regex source for `=~`/`!~`),
/// used to dispatch a reader. For regex match ops we currently only
/// honor a literal prefix like `file:.*`; full regex source-set
/// expansion is documented as a future enhancement.
fn source_value(sel: &Selector) -> Result<&str> {
    let m = sel
        .matchers
        .iter()
        .find(|m| m.name == "source")
        .ok_or_else(|| anyhow!("selector has no `source` matcher (validator should have caught this)"))?;
    Ok(m.value.as_str())
}

/// Resolved plan for a single selector branch.
struct BranchPlan {
    steps: Vec<BranchStep>,
}

struct BranchStep {
    namespace: String,
    target: SshTarget,
    service: Option<String>,
    profile: Profile,
}

fn build_plan(sel: &Selector) -> Result<BranchPlan> {
    // Translate the LogQL selector's `server`/`service` matchers into a
    // single textual selector understood by `crate::selector::resolve`.
    let server_pat = matcher_for(sel, "server").map(|m| matcher_to_selector_atom(m, /*is_server=*/ true));
    let service_pat = matcher_for(sel, "service").map(|m| matcher_to_selector_atom(m, false));

    let server_text = server_pat.unwrap_or_else(|| "*".to_string());
    let service_text = service_pat.unwrap_or_else(|| "*".to_string());
    let combined = format!("{server_text}/{service_text}");

    // For mediums that don't need a real service (discovery without
    // service filter) we still want one step per namespace, so resolve
    // through the selector engine but tolerate empty results.
    let resolved = match crate::selector::resolve::resolve(&combined) {
        Ok(t) => t,
        Err(e) => return Err(anyhow!(e.to_string())),
    };

    // Dedup and load profile per namespace once.
    let mut by_ns: HashMap<String, (SshTarget, Profile)> = HashMap::new();
    for t in &resolved {
        if by_ns.contains_key(&t.namespace) {
            continue;
        }
        let (_, target) = resolve_target(&t.namespace)?;
        let profile = load_profile(&t.namespace)?
            .ok_or_else(|| anyhow!("no profile for namespace `{}`", t.namespace))?;
        by_ns.insert(t.namespace.clone(), (target, profile));
    }

    let mut steps = Vec::new();
    for t in resolved {
        let (target, profile) = match by_ns.get(&t.namespace) {
            Some(x) => x,
            None => continue,
        };
        let svc = match &t.kind {
            crate::selector::resolve::TargetKind::Service { name } => Some(name.clone()),
            crate::selector::resolve::TargetKind::Host => None,
        };
        steps.push(BranchStep {
            namespace: t.namespace.clone(),
            target: target.clone(),
            service: svc,
            profile: profile.clone(),
        });
    }
    Ok(BranchPlan { steps })
}

fn matcher_for<'a>(sel: &'a Selector, name: &str) -> Option<&'a LabelMatcher> {
    sel.matchers.iter().find(|m| m.name == name)
}

/// Translate one LogQL matcher into a syntactic atom that the verb
/// selector parser understands. We use:
///   - `=`  → literal
///   - `=~` on `service` → `/<pattern>/` (verb selector regex form)
///   - `=~` on `server`  → `*` (verb parser has no server-regex form);
///     the engine's per-record [`match_label`] then enforces the regex
///     post-resolution.
///   - `!=` / `!~` → `*`, enforced by [`match_label`].
fn matcher_to_selector_atom(m: &LabelMatcher, is_server: bool) -> String {
    match m.op {
        MatchOp::Eq => m.value.clone(),
        MatchOp::Re => {
            if is_server {
                "*".to_string()
            } else {
                format!("/{}/", m.value)
            }
        }
        MatchOp::Ne | MatchOp::Nre => "*".to_string(),
    }
}


