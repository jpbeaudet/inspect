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

use anyhow::{anyhow, Context, Result};
use regex::Regex;

use crate::alias;
use crate::exec::medium::Medium;
use crate::exec::metric;
use crate::exec::pipeline;
use crate::exec::reader::{self, ReadOpts, ReadStep};
use crate::exec::record::Record;
use crate::exec::ExecCtx;
use crate::logql::ast::{LabelMatcher, LogQuery, MatchOp, Query, Selector};
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
    let mut all = Vec::new();
    for branch in &l.selector.branches {
        let mut recs = run_branch(ctx, branch)?;
        all.append(&mut recs);
    }
    all = pipeline::apply(ctx, &l.pipeline, all)?;
    if ctx.opts.record_limit > 0 && all.len() > ctx.opts.record_limit {
        all.truncate(ctx.opts.record_limit);
    }
    Ok(all)
}

fn run_branch(ctx: &ExecCtx<'_>, sel: &Selector) -> Result<Vec<Record>> {
    let medium_value = source_value(sel)?;
    let medium = Medium::parse(medium_value).map_err(|e| {
        anyhow!(
            "invalid `source=\"{}\"`: {}",
            medium_value,
            e
        )
    })?;
    let reader_impl = reader::for_medium(&medium);

    // Resolve namespaces + services using the existing selector engine.
    let plan = build_plan(sel)?;
    let read_opts = ReadOpts {
        since: ctx.opts.since.clone(),
        until: ctx.opts.until.clone(),
        tail: ctx.opts.tail,
        line_filters: Vec::new(),
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

    let mut out = Vec::new();
    for step in &plan.steps {
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
        let recs = reader_impl
            .read(ctx.runner, &read_step, &read_opts)
            .with_context(|| {
                format!(
                    "reading source={} for {}/{}",
                    medium.as_label(),
                    step.namespace,
                    step.service.as_deref().unwrap_or("_")
                )
            })?;
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
///   - `=~` → `re:<pattern>`
///   - `!=` / `!~` → not directly representable in the verb selector
///     parser; we fall back to `*` and rely on per-record filtering
///     (`match_label`) to enforce.
fn matcher_to_selector_atom(m: &LabelMatcher, _is_server: bool) -> String {
    match m.op {
        MatchOp::Eq => m.value.clone(),
        MatchOp::Re => format!("re:{}", m.value),
        MatchOp::Ne | MatchOp::Nre => "*".to_string(),
    }
}


