//! Integration tests for `inspect query <FILTER>` — the standalone
//! stdin-reading verb that wraps the `query::` abstraction.

use assert_cmd::Command;
use predicates::str::contains;

fn cmd() -> Command {
    Command::cargo_bin("inspect").unwrap()
}

#[test]
fn f19_query_identity_roundtrip() {
    let out = cmd()
        .args(["query", "."])
        .write_stdin(r#"{"a":1,"b":[2,3]}"#)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(out).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(s.trim()).unwrap();
    assert_eq!(parsed, serde_json::json!({"a": 1, "b": [2, 3]}));
}

#[test]
fn f19_query_path_extraction() {
    cmd()
        .args(["query", ".a.b"])
        .write_stdin(r#"{"a":{"b":42}}"#)
        .assert()
        .success()
        .stdout("42\n");
}

#[test]
fn f19_query_raw_string() {
    cmd()
        .args(["query", "-r", ".s"])
        .write_stdin(r#"{"s":"hi"}"#)
        .assert()
        .success()
        .stdout("hi\n");
}

#[test]
fn f19_query_raw_non_string_exit_1() {
    cmd()
        .args(["query", "-r", ".n"])
        .write_stdin(r#"{"n":3}"#)
        .assert()
        .code(1)
        .stderr(contains("non-string"));
}

#[test]
fn f19_query_ndjson_per_frame() {
    let stdin = "{\"line\":\"a\"}\n{\"line\":\"b\"}\n{\"line\":\"c\"}\n";
    cmd()
        .args(["query", "--ndjson", ".line"])
        .write_stdin(stdin)
        .assert()
        .success()
        .stdout("\"a\"\n\"b\"\n\"c\"\n");
}

#[test]
fn f19_query_slurp_length() {
    let stdin = "1\n2\n3\n";
    cmd()
        .args(["query", "--slurp", "length"])
        .write_stdin(stdin)
        .assert()
        .success()
        .stdout("3\n");
}

#[test]
fn f19_query_slurp_add() {
    let stdin = "10\n20\n30\n";
    cmd()
        .args(["query", "--slurp", "add"])
        .write_stdin(stdin)
        .assert()
        .success()
        .stdout("60\n");
}

#[test]
fn f19_query_parse_error_exit_2() {
    cmd()
        .args(["query", ".["])
        .write_stdin("{}")
        .assert()
        .code(2)
        .stderr(contains("parse error"));
}

#[test]
fn f19_query_zero_results_exit_1() {
    cmd()
        .args(["query", ".[] | select(false)"])
        .write_stdin("[1,2,3]")
        .assert()
        .code(1)
        .stdout("");
}

#[test]
fn f19_query_runtime_error_exit_1() {
    cmd()
        .args(["query", "1 + \"x\""])
        .write_stdin("null")
        .assert()
        .code(1)
        .stderr(contains("runtime"));
}

#[test]
fn f19_query_empty_stdin_exit_2() {
    cmd()
        .args(["query", "."])
        .write_stdin("")
        .assert()
        .code(2)
        .stderr(contains("no JSON on stdin"));
}

#[test]
fn f19_query_invalid_json_stdin_exit_2() {
    cmd()
        .args(["query", "."])
        .write_stdin("not json at all")
        .assert()
        .code(2);
}

#[test]
fn f19_query_envelope_recipe_audit_first_id() {
    let envelope = r#"{
        "schema_version": 1,
        "summary": "3 entries",
        "data": {
            "entries": [
                {"id": "sha256:aaa", "verb": "put"},
                {"id": "sha256:bbb", "verb": "chmod"}
            ]
        }
    }"#;
    cmd()
        .args(["query", "-r", ".data.entries[0].id"])
        .write_stdin(envelope)
        .assert()
        .success()
        .stdout("sha256:aaa\n");
}

#[test]
fn f19_query_envelope_recipe_compose_project_names() {
    let envelope = r#"{
        "schema_version": 1,
        "summary": "2",
        "data": {
            "compose_projects": [
                {"name": "atlas"},
                {"name": "luminary"}
            ]
        }
    }"#;
    cmd()
        .args(["query", "-r", ".data.compose_projects[].name"])
        .write_stdin(envelope)
        .assert()
        .success()
        .stdout("atlas\nluminary\n");
}

#[test]
fn f19_query_help_renders_contract() {
    cmd()
        .args(["query", "--help"])
        .assert()
        .success()
        .stdout(contains("jq-language filter"))
        .stdout(contains("EXIT CODES"))
        .stdout(contains("--slurp"));
}
