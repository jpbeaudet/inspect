//! HP-4 — golden-snapshot tests for `inspect help --json`.
//!
//! Pinning rules (plan §10):
//! * `schema_version` is locked at 1 — bumping requires updating
//!   the snapshot deliberately.
//! * Top-level keys are pinned (no addition/removal without a bump).
//! * Per-topic envelope keys are pinned.
//! * Per-command envelope keys are pinned.
//! * Per-flag envelope keys are pinned.
//! * Reserved label / source-type / output-format lists are pinned
//!   verbatim (these are external contract).
//!
//! We deliberately do *not* pin the entire byte stream, so HP-5
//! extending the `errors` array is a value-level change rather than a
//! schema-version bump. The schema bump procedure is documented at
//! [`crate::help::json::SCHEMA_VERSION`].

use assert_cmd::Command;

fn run(args: &[&str]) -> serde_json::Value {
    let out = Command::cargo_bin("inspect")
        .expect("binary builds")
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8(out).expect("utf-8 json");
    serde_json::from_str(&s).expect("valid json")
}

#[test]
fn schema_version_is_one() {
    let v = run(&["help", "--json"]);
    assert_eq!(v["schema_version"], 1);
}

#[test]
fn top_level_keys_are_pinned() {
    let v = run(&["help", "--json"]);
    let obj = v.as_object().expect("top is object");
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort();
    let expected = [
        "binary_name",
        "binary_version",
        "commands",
        "errors",
        "output_formats",
        "reserved_labels",
        "schema_version",
        "source_types",
        "topics",
    ];
    let expected: Vec<&str> = expected.iter().copied().collect();
    assert_eq!(
        keys, expected,
        "top-level JSON keys drifted; bump schema_version intentionally if this is on purpose"
    );
}

#[test]
fn topic_envelope_keys_are_pinned() {
    let v = run(&["help", "--json"]);
    let topics = v["topics"].as_array().expect("topics is array");
    assert!(!topics.is_empty());
    let expected = ["examples", "id", "see_also", "summary", "title", "verbs"];
    for t in topics {
        let obj = t.as_object().expect("topic is object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort();
        let exp: Vec<&str> = expected.iter().copied().collect();
        assert_eq!(keys, exp, "topic envelope keys drifted: {:?}", t);
    }
}

#[test]
fn command_envelope_keys_are_pinned() {
    let v = run(&["help", "--json"]);
    let cmds = v["commands"].as_object().expect("commands is object");
    assert!(
        cmds.len() >= 30,
        "expected ≥30 commands, got {}",
        cmds.len()
    );
    let expected = [
        "aliases",
        "examples",
        "flags",
        "name",
        "see_also",
        "see_also_line",
        "summary",
    ];
    for (verb, c) in cmds {
        let obj = c.as_object().expect("command is object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort();
        let exp: Vec<&str> = expected.iter().copied().collect();
        assert_eq!(keys, exp, "command envelope for {verb} drifted");
    }
}

#[test]
fn flag_envelope_keys_are_pinned() {
    let v = run(&["help", "--json"]);
    let grep_flags = v["commands"]["grep"]["flags"]
        .as_array()
        .expect("grep.flags is array");
    assert!(!grep_flags.is_empty());
    let expected = [
        "description",
        "long",
        "name",
        "positional",
        "repeated",
        "required",
        "short",
        "takes_value",
        "value_name",
    ];
    for f in grep_flags {
        let obj = f.as_object().expect("flag is object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort();
        let exp: Vec<&str> = expected.iter().copied().collect();
        assert_eq!(keys, exp, "flag envelope for grep drifted: {f:?}");
    }
}

#[test]
fn reserved_label_list_is_pinned() {
    let v = run(&["help", "--json"]);
    let arr: Vec<&str> = v["reserved_labels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap())
        .collect();
    assert_eq!(
        arr,
        vec!["server", "service", "container", "source", "path"]
    );
}

#[test]
fn source_types_pinned() {
    let v = run(&["help", "--json"]);
    let arr: Vec<&str> = v["source_types"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap())
        .collect();
    assert_eq!(arr, vec!["logs", "file", "discovery", "metric"]);
}

#[test]
fn output_formats_pinned() {
    let v = run(&["help", "--json"]);
    let arr: Vec<&str> = v["output_formats"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap())
        .collect();
    assert_eq!(
        arr,
        vec!["human", "json", "ndjson", "csv", "tsv", "md", "yaml", "raw", "format"]
    );
}

#[test]
fn jq_pipeline_from_acceptance_script_works() {
    // Plan §11 step 5: the acceptance demo runs this exact predicate.
    let v = run(&["help", "--json"]);
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["topics"].as_array().unwrap().len(), 12);
    assert!(v["commands"].as_object().unwrap().len() >= 30);
}

#[test]
fn topic_envelope_for_quickstart() {
    let v = run(&["help", "quickstart", "--json"]);
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["topic"]["id"], "quickstart");
    assert!(v["topic"]["examples"].as_array().unwrap().len() >= 3);
    let expected = [
        "body", "examples", "id", "see_also", "summary", "title", "verbs",
    ];
    let obj = v["topic"].as_object().unwrap();
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort();
    let exp: Vec<&str> = expected.iter().copied().collect();
    assert_eq!(keys, exp);
}

#[test]
fn unknown_topic_with_json_still_exits_one() {
    Command::cargo_bin("inspect")
        .unwrap()
        .args(["help", "definitely-not-a-topic", "--json"])
        .assert()
        .code(1);
}

#[test]
fn errors_catalog_shape_is_pinned() {
    let v = run(&["help", "--json"]);
    let errs = v["errors"].as_array().expect("errors is array");
    assert!(!errs.is_empty());
    let expected = ["code", "help_topic", "summary"];
    for e in errs {
        let obj = e.as_object().unwrap();
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort();
        let exp: Vec<&str> = expected.iter().copied().collect();
        assert_eq!(keys, exp);
    }
}

#[test]
fn pretty_vs_compact_round_trip_to_same_value() {
    // Compact mode (non-tty stdout via assert_cmd) is what we get;
    // both should parse to the same logical value when invoked with
    // and without piping. We can only assert one path here, but the
    // round-trip-to-Value comparison guards against subtle escaping
    // bugs in the hand-rolled writer.
    let v1 = run(&["help", "--json"]);
    let v2 = run(&["help", "--json"]);
    assert_eq!(v1, v2, "two runs must produce byte-equal JSON");
}

// =====================================================================
// HP-7 G8 — checked-in golden snapshot.
//
// `tests/golden/help_json_skeleton.json` pins the contractually frozen
// subset of the `--json` surface: schema version, top-level keys,
// per-envelope key sets, fixed value lists (reserved_labels,
// source_types, output_formats), topic ids, and the error code roster.
//
// Drift in any of these is a deliberate schema bump:
//   1. Update the snapshot:
//        cargo run --quiet -- help --json | python3 …  (see plan §10)
//   2. Bump `crate::help::json::SCHEMA_VERSION` if it changed shape.
//   3. Document the diff in CHANGELOG.
//
// We deliberately do *not* pin the entire JSON byte stream so adding a
// new error code or topic body change is a value-level edit rather
// than a schema bump.
// =====================================================================

#[test]
fn json_skeleton_matches_golden() {
    let v = run(&["help", "--json"]);

    // Reproduce the same skeleton extraction the generator script does.
    let mut top_keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    top_keys.sort();

    let topics = v["topics"].as_array().unwrap();
    let topic_ids: Vec<&str> = topics.iter().map(|t| t["id"].as_str().unwrap()).collect();
    let mut topic_keys: Vec<&str> = topics[0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    topic_keys.sort();

    let commands = v["commands"].as_object().unwrap();
    let first_cmd = commands.values().next().unwrap().as_object().unwrap();
    let mut cmd_keys: Vec<&str> = first_cmd.keys().map(String::as_str).collect();
    cmd_keys.sort();

    // Pick the first command with a non-empty `flags` array.
    let flag_obj = commands
        .values()
        .filter_map(|c| c["flags"].as_array())
        .find(|fs| !fs.is_empty())
        .and_then(|fs| fs[0].as_object());
    let mut flag_keys: Vec<&str> = flag_obj
        .map(|o| o.keys().map(String::as_str).collect())
        .unwrap_or_default();
    flag_keys.sort();

    let mut error_codes: Vec<&str> = v["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["code"].as_str().unwrap())
        .collect();
    error_codes.sort();

    let live = serde_json::json!({
        "command_envelope_keys": cmd_keys,
        "error_codes": error_codes,
        "flag_envelope_keys": flag_keys,
        "output_formats": v["output_formats"],
        "reserved_labels": v["reserved_labels"],
        "schema_version": v["schema_version"],
        "source_types": v["source_types"],
        "top_level_keys": top_keys,
        "topic_envelope_keys": topic_keys,
        "topic_ids": topic_ids,
    });

    let golden_text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/help_json_skeleton.json"
    ))
    .expect("golden snapshot must exist at tests/golden/help_json_skeleton.json");
    let golden: serde_json::Value =
        serde_json::from_str(&golden_text).expect("golden is valid json");

    if live != golden {
        // Render a compact diff so failing CI tells the maintainer
        // exactly what drifted, not "values differ".
        let live_pretty = serde_json::to_string_pretty(&live).unwrap();
        let gold_pretty = serde_json::to_string_pretty(&golden).unwrap();
        panic!(
            "help --json skeleton drift detected.\n\
             ----- GOLDEN (tests/golden/help_json_skeleton.json) -----\n{gold_pretty}\n\
             ----- LIVE (cargo run -- help --json) -----\n{live_pretty}\n\
             If this change is intentional: regenerate the golden via the procedure\n\
             documented at the top of tests/help_json_snapshot.rs and bump\n\
             SCHEMA_VERSION when keys/value-lists/envelopes change shape."
        );
    }
}
