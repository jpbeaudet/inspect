use serde_json::{json, Value};

use super::{
    compile, eval, eval_compiled, eval_slurp, ndjson, render_compact, render_raw, QueryErrorKind,
};

#[test]
fn identity_returns_input() {
    let input = json!({"a": 1, "b": [2, 3]});
    let out = eval(".", &input).expect("identity should never fail");
    assert_eq!(out, vec![input]);
}

#[test]
fn path_extraction() {
    let input = json!({"foo": {"bar": "baz"}});
    let out = eval(".foo.bar", &input).unwrap();
    assert_eq!(out, vec![json!("baz")]);
}

#[test]
fn array_iteration() {
    let input = json!([10, 20, 30]);
    let out = eval(".[]", &input).unwrap();
    assert_eq!(out, vec![json!(10), json!(20), json!(30)]);
}

#[test]
fn select_with_predicate() {
    let input = json!([
        {"name": "a", "healthy": true},
        {"name": "b", "healthy": false},
        {"name": "c", "healthy": true},
    ]);
    let out = eval(".[] | select(.healthy)", &input).unwrap();
    assert_eq!(
        out,
        vec![
            json!({"name": "a", "healthy": true}),
            json!({"name": "c", "healthy": true}),
        ]
    );
}

#[test]
fn length_on_array_and_object() {
    let arr = json!([1, 2, 3, 4]);
    let obj = json!({"a": 1, "b": 2});
    assert_eq!(eval("length", &arr).unwrap(), vec![json!(4)]);
    assert_eq!(eval("length", &obj).unwrap(), vec![json!(2)]);
}

#[test]
fn keys_sorted() {
    let input = json!({"b": 1, "a": 2, "c": 3});
    let out = eval("keys", &input).unwrap();
    assert_eq!(out, vec![json!(["a", "b", "c"])]);
}

#[test]
fn map_then_unique() {
    let input = json!([
        {"name": "alpha"},
        {"name": "beta"},
        {"name": "alpha"},
    ]);
    let out = eval("map(.name) | unique", &input).unwrap();
    assert_eq!(out, vec![json!(["alpha", "beta"])]);
}

#[test]
fn null_safe_path() {
    let input = json!({"present": 1});
    let out = eval(".missing?", &input).unwrap();
    assert_eq!(out, vec![Value::Null]);
}

#[test]
fn parse_error_classified() {
    let input = json!({});
    let err = eval(".[", &input).expect_err("'.[' is not a valid filter");
    assert_eq!(err.kind, QueryErrorKind::Parse);
    assert!(!err.message.is_empty(), "parse error must carry a message");
}

#[test]
fn runtime_error_classified() {
    let input = json!(null);
    let err = eval("1 + \"x\"", &input).expect_err("number + string is a runtime error");
    assert_eq!(err.kind, QueryErrorKind::Runtime);
    assert!(
        !err.message.is_empty(),
        "runtime error must carry a message"
    );
}

#[test]
fn slurp_collects_all() {
    let inputs = vec![json!(1), json!(2), json!(3)];
    let out = eval_slurp("length", &inputs).unwrap();
    assert_eq!(out, vec![json!(3)]);
}

#[test]
fn compile_then_eval_three_lines() {
    let compiled = compile(".line").expect("filter must compile");
    let lines = vec![
        json!({"line": "a"}),
        json!({"line": "b"}),
        json!({"line": "c"}),
    ];
    let mut all = Vec::new();
    for line in &lines {
        all.extend(eval_compiled(&compiled, line).unwrap());
    }
    assert_eq!(all, vec![json!("a"), json!("b"), json!("c")]);
}

#[test]
fn render_raw_string_unquoted() {
    let input = json!({"summary": "ok"});
    let values = eval(".summary", &input).unwrap();
    let raw = render_raw(&values).unwrap();
    assert_eq!(raw, "ok\n");
}

#[test]
fn render_raw_non_string_errors() {
    let input = json!({"count": 3});
    let values = eval(".count", &input).unwrap();
    let err = render_raw(&values).expect_err("number must not render-raw");
    assert_eq!(err.kind, QueryErrorKind::RawNonString);
    assert!(err.message.contains("non-string"));
    assert!(err.message.contains("number"));
}

#[test]
fn render_compact_one_per_line() {
    let input = json!([1, 2, 3]);
    let values = eval(".[]", &input).unwrap();
    let compact = render_compact(&values);
    assert_eq!(compact, "1\n2\n3\n");
}

#[test]
fn render_compact_empty_yields_empty_string() {
    let input = json!([]);
    let values = eval(".[]", &input).unwrap();
    assert!(values.is_empty());
    assert_eq!(render_compact(&values), "");
}

#[test]
fn ndjson_per_frame_compact() {
    let mut filter = ndjson::Filter::new(".line", false, false).unwrap();
    let frames = vec![json!({"line": "alpha"}), json!({"line": "beta"})];
    let mut out = String::new();
    for f in &frames {
        out.push_str(&filter.on_line(f).unwrap());
    }
    out.push_str(&filter.finish().unwrap());
    assert_eq!(out, "\"alpha\"\n\"beta\"\n");
}

#[test]
fn ndjson_per_frame_raw() {
    let mut filter = ndjson::Filter::new(".line", true, false).unwrap();
    let frames = vec![json!({"line": "alpha"}), json!({"line": "beta"})];
    let mut out = String::new();
    for f in &frames {
        out.push_str(&filter.on_line(f).unwrap());
    }
    out.push_str(&filter.finish().unwrap());
    assert_eq!(out, "alpha\nbeta\n");
}

#[test]
fn ndjson_slurp_length() {
    let mut filter = ndjson::Filter::new("length", false, true).unwrap();
    for f in [json!(1), json!(2), json!(3)].iter() {
        let s = filter.on_line(f).unwrap();
        assert!(s.is_empty(), "slurp mode buffers, no per-line output");
    }
    let final_out = filter.finish().unwrap();
    assert_eq!(final_out, "3\n");
}

#[test]
fn ndjson_parse_error_at_construction() {
    let err = ndjson::Filter::new(".[", false, false).expect_err("invalid filter");
    assert_eq!(err.kind, QueryErrorKind::Parse);
}

#[test]
fn recipe_audit_ls_first_id() {
    let envelope = json!({
        "schema_version": 1,
        "summary": "3 entries",
        "data": {
            "entries": [
                {"id": "sha256:aaaaaaa", "verb": "put"},
                {"id": "sha256:bbbbbbb", "verb": "chmod"},
                {"id": "sha256:ccccccc", "verb": "delete"},
            ]
        }
    });
    let out = eval(".data.entries[0].id", &envelope).unwrap();
    assert_eq!(out, vec![json!("sha256:aaaaaaa")]);
}

#[test]
fn recipe_status_state_and_count() {
    let envelope = json!({
        "schema_version": 1,
        "summary": "12 services healthy",
        "data": {
            "state": "healthy",
            "services": [{"name": "a"}, {"name": "b"}, {"name": "c"}],
        },
        "meta": {"source": {"mode": "live"}},
    });
    let filter = "{state: .data.state, services_count: (.data.services | length), \
         summary, source_mode: .meta.source.mode}";
    let out = eval(filter, &envelope).unwrap();
    assert_eq!(
        out,
        vec![json!({
            "state": "healthy",
            "services_count": 3,
            "summary": "12 services healthy",
            "source_mode": "live",
        })]
    );
}

#[test]
fn recipe_compose_ls_project_names() {
    let envelope = json!({
        "schema_version": 1,
        "summary": "2 compose projects",
        "data": {
            "compose_projects": [
                {"name": "atlas", "services": 4},
                {"name": "luminary", "services": 7},
            ]
        }
    });
    let out = eval(".data.compose_projects[].name", &envelope).unwrap();
    assert_eq!(out, vec![json!("atlas"), json!("luminary")]);
}
