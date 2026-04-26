//! Round-trip tests against the canonical examples in bible §9.12.

use super::ast::*;
use super::*;

fn ok(input: &str) -> Query {
    parse(input).unwrap_or_else(|e| panic!("expected ok, got: {}\n{}", e, e.render(input)))
}

fn err(input: &str) -> ParseError {
    parse(input).expect_err("expected parse error")
}

#[test]
fn simple_selector_with_filter() {
    let q = ok(r#"{server=~"prod-.*", service="storage", source="logs"} |= "error""#);
    assert!(matches!(q, Query::Log(_)));
    let Query::Log(l) = q else { unreachable!() };
    assert_eq!(l.selector.branches.len(), 1);
    let s = &l.selector.branches[0];
    assert_eq!(s.matchers.len(), 3);
    assert_eq!(s.matchers[0].name, "server");
    assert_eq!(s.matchers[0].op, MatchOp::Re);
    assert_eq!(l.pipeline.len(), 1);
}

#[test]
fn json_stage_then_field_filter() {
    let q = ok(r#"{server="arte", source="logs"} | json | status >= 500"#);
    let Query::Log(l) = q else { unreachable!() };
    assert_eq!(l.pipeline.len(), 2);
    assert!(matches!(
        l.pipeline[0],
        PipelineOp::Stage(Stage::Json)
    ));
    assert!(matches!(
        l.pipeline[1],
        PipelineOp::Stage(Stage::FieldFilter { .. })
    ));
}

#[test]
fn count_over_time_metric() {
    let q = ok(r#"count_over_time({server="arte", source="logs"} |= "error" [5m])"#);
    assert!(matches!(q, Query::Metric(_)));
}

#[test]
fn topk_with_grouping() {
    let q = ok(
        r#"topk(5, sum by (service) (rate({server="arte", source="logs"} |= "error" [1h])))"#,
    );
    let Query::Metric(MetricQuery::Vector(v)) = q else {
        panic!("expected vector aggregation")
    };
    assert_eq!(v.func, AggFn::Topk);
    assert_eq!(v.param, Some(5));
    let MetricQuery::Vector(inner) = *v.inner else {
        panic!("expected nested vector aggregation")
    };
    assert_eq!(inner.func, AggFn::Sum);
    assert!(matches!(
        inner.grouping,
        Some(Grouping {
            mode: GroupingMode::By,
            ..
        })
    ));
}

#[test]
fn map_stage_with_subquery() {
    let q = ok(
        r#"{server="arte", source="logs"} |= "milvus" | json | map { {server="arte", service="$service", source=~"file:.*"} |~ "milvus" }"#,
    );
    let Query::Log(l) = q else { unreachable!() };
    let last = l.pipeline.last().unwrap();
    let PipelineOp::Stage(Stage::Map { sub, .. }) = last else {
        panic!("expected map stage")
    };
    assert_eq!(sub.selector.branches.len(), 1);
}

#[test]
fn selector_union_with_or() {
    let q = ok(
        r#"{server="arte", service="atlas", source="logs"} or {server="arte", service="atlas", source="file:/etc/atlas.conf"} |= "milvus""#,
    );
    let Query::Log(l) = q else { unreachable!() };
    assert_eq!(l.selector.branches.len(), 2);
}

#[test]
fn field_filter_boolean() {
    let q = ok(
        r#"{server="arte", source="logs"} | json | status >= 500 and method == "POST" or path =~ "/api/.*""#,
    );
    let Query::Log(l) = q else { unreachable!() };
    let PipelineOp::Stage(Stage::FieldFilter { expr, .. }) = &l.pipeline[1] else {
        panic!("expected field filter")
    };
    // top is `or`
    assert!(matches!(expr, FieldExpr::Or(_, _)));
}

#[test]
fn label_format_and_drop() {
    let q = ok(
        r#"{server="arte", source="logs"} | json | label_format svc="{{.service}}" | drop tmp, debug"#,
    );
    let Query::Log(l) = q else { unreachable!() };
    assert!(matches!(l.pipeline[1], PipelineOp::Stage(Stage::LabelFormat { .. })));
    assert!(matches!(l.pipeline[2], PipelineOp::Stage(Stage::Drop { .. })));
}

#[test]
fn alias_substitution() {
    let q = parse_with_aliases(r#"@plogs |= "x""#, |n| {
        if n == "plogs" {
            Some(r#"{server="arte", source="logs"}"#.into())
        } else {
            None
        }
    })
    .unwrap();
    assert!(matches!(q, Query::Log(_)));
}

#[test]
fn alias_union_or() {
    let q = parse_with_aliases(r#"@plogs or @atlas |= "milvus""#, |n| match n {
        "plogs" => Some(r#"{server="arte", source="logs"}"#.into()),
        "atlas" => Some(r#"{server="arte", source="file:/etc/atlas.conf"}"#.into()),
        _ => None,
    })
    .unwrap();
    let Query::Log(l) = q else { unreachable!() };
    assert_eq!(l.selector.branches.len(), 2);
}

#[test]
fn unknown_alias_errors() {
    let e = err("@nope");
    assert!(e.message.contains("unknown alias"));
}

#[test]
fn missing_source_label_errors() {
    let e = err(r#"{server="arte"} |= "x""#);
    assert!(e.message.contains("source"));
}

#[test]
fn rejects_unterminated_string() {
    let e = err(r#"{server="arte}"#);
    assert!(e.message.contains("unterminated") || e.message.contains("string"));
}

#[test]
fn topk_requires_integer_param() {
    let e = err(r#"topk(sum by (service) (rate({source="logs"} [1h])))"#);
    assert!(e.message.contains("integer"));
}

#[test]
fn empty_selector_errors() {
    let e = err(r#"{} |= "x""#);
    assert!(e.message.contains("at least one") || e.message.contains("source"));
}

#[test]
fn duration_units() {
    let q = ok(r#"rate({source="logs"} [2h])"#);
    let Query::Metric(MetricQuery::Range(r)) = q else {
        panic!("expected range agg")
    };
    assert_eq!(r.range_ms, 2 * 3_600_000);
}

#[test]
fn render_diagnostic_has_carat() {
    let e = err(r#"{server=}"#);
    let r = e.render(r#"{server=}"#);
    assert!(r.contains("error:"));
    assert!(r.contains("^"));
}

#[test]
fn trailing_garbage_errors() {
    let e = err(r#"{source="logs"} junk"#);
    assert!(e.message.contains("trailing"));
}

#[test]
fn diagnostic_uses_friendly_token_names_not_debug() {
    // Bad: missing string after match operator. Error must say `}`,
    // not the debug form `RBrace`.
    let e = err(r#"{server=}"#);
    assert!(e.message.contains("`}`"), "got: {}", e.message);
    assert!(!e.message.contains("RBrace"), "leaked Debug repr: {}", e.message);
}

#[test]
fn diagnostic_includes_actionable_hint() {
    let e = err(r#"{server=}"#);
    assert!(e.hint.is_some(), "expected a hint, got none");
    let h = e.hint.unwrap();
    assert!(h.contains("double-quoted") || h.contains("\""), "hint not actionable: {h}");
}

#[test]
fn diagnostic_for_bad_duration_suggests_format() {
    let e = err(r#"count_over_time({source="logs"}[5xx])"#);
    let h = e.hint.expect("expected a hint");
    assert!(h.contains("5m") || h.contains("30s"), "hint should suggest duration format: {h}");
}

#[test]
fn diagnostic_for_topk_non_integer_suggests_form() {
    let e = err(r#"topk("five", sum({source="logs"}))"#);
    assert!(e.message.contains("topk"));
    let h = e.hint.expect("hint");
    assert!(h.contains("topk(5"), "hint should give example: {h}");
}

#[test]
fn diagnostic_for_missing_open_brace_suggests_selector_form() {
    let e = err(r#"foobar"#);
    let h = e.hint.expect("hint");
    assert!(h.contains("{") && h.contains("source"), "hint should sketch selector: {h}");
}

// ---------------------------------------------------------------------------
// Audit P2 — alias-error wrapping (§1.7), zero-duration rejection (§1.4),
// and negative-test matrix (§1.1).
// ---------------------------------------------------------------------------

#[test]
fn alias_expansion_error_wraps_alias_name() {
    // Body is malformed; without wrapping the user would see a span
    // pointing into the substituted text and no mention of which
    // alias caused it.
    let e = parse_with_aliases("@bad", |n| (n == "bad").then(|| "{server=}".to_string()))
        .expect_err("malformed alias body must error");
    assert!(
        e.message.contains("@bad"),
        "expected alias name in error, got: {}",
        e.message
    );
    assert!(e.message.contains("expansion of"), "got: {}", e.message);
    // span should snap back to the original `@bad` reference (0..4)
    assert_eq!(e.span, 0..4, "span should re-frame to original site");
}

#[test]
fn zero_range_duration_is_rejected() {
    let e = err(r#"rate({source="logs"} [0s])"#);
    assert!(
        e.message.contains("greater than zero") || e.message.contains("must be"),
        "expected zero-range error, got: {}",
        e.message
    );
}

#[test]
fn negative_unclosed_selector() {
    let e = err(r#"{server="arte""#);
    // any of: expected `,` / `}`, unterminated selector
    assert!(
        e.message.contains("`}`")
            || e.message.contains("unterminated")
            || e.message.contains("expected"),
        "got: {}",
        e.message
    );
}

#[test]
fn negative_dangling_filter_pipe() {
    let e = err(r#"{source="logs"} |="#);
    assert!(
        e.message.contains("expected") || e.message.contains("string"),
        "got: {}",
        e.message
    );
}

#[test]
fn negative_metric_of_metric_rejected() {
    let e = err(r#"rate(sum({source="logs"})[5m])"#);
    assert!(
        e.message.contains("expected") || e.message.contains("log"),
        "got: {}",
        e.message
    );
}

#[test]
fn negative_map_without_braces() {
    let e = err(r#"{source="logs"} | map "x""#);
    assert!(e.message.contains("{") || e.message.contains("expected"), "got: {}", e.message);
}

#[test]
fn negative_random_inputs_never_panic() {
    // Tiny sanity fuzz: a handful of adversarial strings must all
    // return an Err without ever panicking. Real fuzz target lives
    // in fuzz/ (added separately).
    let bad = [
        "",
        "{",
        "}",
        "{}",
        "{=}",
        "{=\"x\"}",
        r#"{source=}"#,
        r#"{source="logs"} |"#,
        r#"{source="logs"} | json |"#,
        r#"rate("#,
        r#"rate({source="logs"} [5m"#,
        r#"topk(,)"#,
        "@",
        "@@",
        r#"@x or"#,
        "\u{0}\u{1}\u{2}",
    ];
    for q in bad {
        let _ = parse(q); // must not panic; Err vs Ok both acceptable
    }
}
