//! Post-parse semantic checks (bible §9.2, §9.7).

use super::ast::*;
use super::error::ParseError;

/// Reserved labels that carry inspect-specific semantics.
pub const RESERVED_LABELS: &[&str] = &["server", "service", "source"];

pub fn validate(q: &Query) -> Result<(), ParseError> {
    match q {
        Query::Log(l) => validate_log(l),
        Query::Metric(m) => validate_metric(m),
    }
}

fn validate_log(l: &LogQuery) -> Result<(), ParseError> {
    validate_selector_union(&l.selector)?;
    for op in &l.pipeline {
        if let PipelineOp::Stage(Stage::Map { sub, .. }) = op {
            validate_log(sub)?;
        }
    }
    Ok(())
}

fn validate_selector_union(u: &SelectorUnion) -> Result<(), ParseError> {
    if u.branches.is_empty() {
        return Err(ParseError::new(
            "selector union has no branches",
            u.span.clone(),
        ));
    }
    for s in &u.branches {
        validate_selector(s)?;
    }
    Ok(())
}

fn validate_selector(s: &Selector) -> Result<(), ParseError> {
    if s.matchers.is_empty() {
        return Err(ParseError::new(
            "selector must have at least one label matcher",
            s.span.clone(),
        )
        .with_hint("e.g. `{server=\"arte\", source=\"logs\"}`"));
    }
    // Every selector must include `source` (it tells us *which medium* to read).
    let has_source = s.matchers.iter().any(|m| m.name == "source");
    if !has_source {
        return Err(ParseError::new(
            "selector is missing required `source` label",
            s.span.clone(),
        )
        .with_hint(
            "add e.g. `source=\"logs\"`, `source=~\"file:.*\"`, `source=\"discovery\"`, etc.",
        ));
    }
    Ok(())
}

fn validate_metric(m: &MetricQuery) -> Result<(), ParseError> {
    match m {
        MetricQuery::Range(r) => validate_log(&r.inner),
        MetricQuery::Vector(v) => {
            if v.func.requires_param() && v.param.is_none() {
                return Err(ParseError::new(
                    format!("`{}` requires an integer parameter", v.func.as_str()),
                    v.span.clone(),
                ));
            }
            validate_metric(&v.inner)
        }
    }
}
