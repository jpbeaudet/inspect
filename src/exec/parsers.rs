//! Stream parsers: `| json`, `| logfmt`, `| pattern "..."`, `| regexp "..."`.
//!
//! ## Error semantics (audit §2.4)
//!
//! Real log streams are mixed: a single JSON parse failure must NOT
//! drop the offending line, otherwise users lose visibility on exactly
//! the records they want to investigate. Every parser in this module
//! follows the same convention:
//!
//! * The offending record is preserved (it always reaches downstream
//!   stages).
//! * On failure we set two fields:
//!     - `__error__`         — short tag (`JSONParserErr`,
//!       `LogfmtParserErr`, `PatternParserErr`, `RegexParserErr`).
//!     - `__error_details__` — human-readable explanation.
//! * Users can filter with `| __error__ = ""` (clean only) or
//!   `| __error__ != ""` (failures only), matching Loki's behaviour.
//!
//! ## Label name sanitization (audit §1.5)
//!
//! Extracted keys are sanitized to follow Prometheus naming
//! (`[A-Za-z_:][A-Za-z0-9_:]*`). Invalid characters are replaced with
//! `_`; names starting with a digit are prefixed with `_`. If two
//! parser stages extract the same sanitized key into the same record
//! (`| json | logfmt` both producing `level`, for instance), the
//! second one is suffixed `_extracted` (then `_extracted_2`, ...) so
//! both values stay addressable. Pre-existing labels are untouched
//! — `Record::lookup()` already prefers fields over labels, so an
//! extracted `status` shadows the source label at lookup time without
//! destroying it.

use regex::Regex;
use serde_json::Value;

use crate::exec::record::Record;

/// Tag set on `record.fields["__error__"]` when a parser stage fails.
pub const ERR_FIELD: &str = "__error__";
/// Detail field set alongside `__error__`.
pub const ERR_DETAILS_FIELD: &str = "__error_details__";

/// Sanitize an extracted key to a Prometheus-compatible label name.
///
/// Rules:
///   * Allowed chars: `A-Z a-z 0-9 _ :`
///   * Any other char (including `.`, `-`, space, unicode) → `_`
///   * Leading digit → prefixed with `_`
///   * Empty input → `_`
pub fn sanitize_label_name(name: &str) -> String {
    if name.is_empty() {
        return "_".to_string();
    }
    let mut out = String::with_capacity(name.len() + 1);
    for (i, c) in name.chars().enumerate() {
        let ok = c.is_ascii_alphanumeric() || c == '_' || c == ':';
        if i == 0 && c.is_ascii_digit() {
            out.push('_');
        }
        if ok {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    out
}

/// Insert an extracted field into `rec`, sanitizing the key and avoiding
/// collisions with existing labels or fields by suffixing `_extracted`.
///
/// Returns the final key actually used (useful for tests and diagnostics).
pub fn insert_extracted(rec: &mut Record, raw_key: &str, value: Value) -> String {
    let base = sanitize_label_name(raw_key);
    let key = unique_key(rec, &base);
    rec.fields.insert(key.clone(), value);
    key
}

fn unique_key(rec: &Record, base: &str) -> String {
    // Note: we only check the `fields` map here, not `labels`. Fields
    // and labels are separate dimensions; `Record::lookup()` already
    // prefers fields over labels, so putting an extracted `status` into
    // `fields["status"]` shadows the label at lookup time without
    // destroying it. The real collision case is when two parser stages
    // (e.g. `| json | logfmt`) both extract the same key — there we
    // suffix `_extracted` to keep both values addressable.
    if !rec.fields.contains_key(base) {
        return base.to_string();
    }
    let suffixed = format!("{base}_extracted");
    if !rec.fields.contains_key(&suffixed) {
        return suffixed;
    }
    for n in 2..u32::MAX {
        let candidate = format!("{base}_extracted_{n}");
        if !rec.fields.contains_key(&candidate) {
            return candidate;
        }
    }
    // Practically unreachable; fall back to the suffixed form.
    format!("{base}_extracted")
}

/// Mark a record as having failed a parser stage. Idempotent: if the
/// record already carries an `__error__` it is preserved (first
/// failure wins), but details are appended so users see the full chain.
fn mark_error(rec: &mut Record, tag: &str, details: &str) {
    if !rec.fields.contains_key(ERR_FIELD) {
        rec.fields
            .insert(ERR_FIELD.to_string(), Value::String(tag.to_string()));
    }
    let new_detail = if let Some(Value::String(prev)) = rec.fields.get(ERR_DETAILS_FIELD) {
        if prev.is_empty() {
            details.to_string()
        } else {
            format!("{prev}; {tag}: {details}")
        }
    } else {
        details.to_string()
    };
    rec.fields
        .insert(ERR_DETAILS_FIELD.to_string(), Value::String(new_detail));
}

/// Apply `| json`: parse `record.line` as a JSON object and merge its
/// keys into `fields` (sanitized, collision-safe). On any failure the
/// record survives with `__error__` set; it is **not** dropped.
pub fn parse_json(rec: &mut Record) {
    let Some(line) = rec.line.as_deref() else {
        // No line to parse: not a structural error, just nothing to do.
        return;
    };
    match serde_json::from_str::<Value>(line) {
        Ok(Value::Object(map)) => {
            for (k, v) in map {
                insert_extracted(rec, &k, v);
            }
        }
        Ok(other) => {
            // Valid JSON but not an object (array, string, number, ...).
            // Loki's `| json` likewise refuses non-object roots.
            mark_error(
                rec,
                "JSONParserErr",
                &format!("expected JSON object, got {}", json_kind(&other)),
            );
        }
        Err(e) => {
            mark_error(rec, "JSONParserErr", &e.to_string());
        }
    }
}

fn json_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Apply `| logfmt`: parse `key=value` pairs (space-separated, optional
/// quoted values).
///
/// Logfmt is intentionally permissive — bare keys, missing values, and
/// truncated quoted strings are tolerated silently because that matches
/// what real log shippers produce. We only flag a top-level error when
/// the result is empty *and* the line was non-empty (likely binary or
/// JSON misclassified as logfmt).
pub fn parse_logfmt(rec: &mut Record) {
    let Some(line) = rec.line.clone() else { return };
    let bytes = line.as_bytes();
    let mut extracted = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        // skip whitespace
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        // key
        let key_start = i;
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && bytes[i] != b'='
        {
            i += 1;
        }
        let key = std::str::from_utf8(&bytes[key_start..i])
            .unwrap_or("")
            .to_string();
        if i >= bytes.len() || bytes[i] != b'=' {
            // bare key; skip
            continue;
        }
        i += 1; // consume `=`
        // value
        let value = if i < bytes.len() && bytes[i] == b'"' {
            i += 1;
            let v_start = i;
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            let v = std::str::from_utf8(&bytes[v_start..i])
                .unwrap_or("")
                .to_string();
            if i < bytes.len() {
                i += 1; // closing quote
            }
            v
        } else {
            let v_start = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            std::str::from_utf8(&bytes[v_start..i])
                .unwrap_or("")
                .to_string()
        };
        if !key.is_empty() {
            insert_extracted(rec, &key, Value::String(value));
            extracted += 1;
        }
    }
    if extracted == 0 && !line.trim().is_empty() {
        mark_error(rec, "LogfmtParserErr", "no key=value pairs found");
    }
}

/// Apply `| pattern "<...>"` Loki-style: any `<name>` placeholder
/// captures up to the next literal text.
pub fn parse_pattern(rec: &mut Record, template: &str) -> Result<(), String> {
    let Some(line) = rec.line.clone() else { return Ok(()) };
    let re = compile_pattern(template)?;
    match re.captures(&line) {
        Some(caps) => {
            for name in re.capture_names().flatten() {
                if let Some(m) = caps.name(name) {
                    insert_extracted(rec, name, Value::String(m.as_str().into()));
                }
            }
        }
        None => mark_error(rec, "PatternParserErr", "pattern did not match line"),
    }
    Ok(())
}

fn compile_pattern(template: &str) -> Result<Regex, String> {
    let mut out = String::from("^");
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' {
            let mut name = String::new();
            for cc in chars.by_ref() {
                if cc == '>' {
                    break;
                }
                name.push(cc);
            }
            if name == "_" {
                out.push_str(".*?");
            } else if !name.is_empty() {
                out.push_str(&format!("(?P<{name}>.*?)"));
            }
            continue;
        }
        if c.is_whitespace() {
            // collapse consecutive whitespace
            while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
                chars.next();
            }
            out.push_str(r"\s+");
            continue;
        }
        // escape regex metacharacters
        if "\\.+*?()[]{}|^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('$');
    Regex::new(&out).map_err(|e| format!("invalid pattern: {e}"))
}

/// Apply `| regexp "<...>"`: a real regex with named groups extracted into fields.
pub fn parse_regexp(rec: &mut Record, pattern: &str) -> Result<(), String> {
    let Some(line) = rec.line.clone() else { return Ok(()) };
    let re = Regex::new(pattern).map_err(|e| format!("invalid regexp: {e}"))?;
    match re.captures(&line) {
        Some(caps) => {
            for name in re.capture_names().flatten() {
                if let Some(m) = caps.name(name) {
                    insert_extracted(rec, name, Value::String(m.as_str().into()));
                }
            }
        }
        None => mark_error(rec, "RegexParserErr", "regexp did not match line"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_parses_object() {
        let mut r = Record::new().with_line(r#"{"level":"error","status":500}"#);
        parse_json(&mut r);
        assert_eq!(r.fields.get("level").unwrap(), "error");
        assert_eq!(r.fields.get("status").unwrap(), 500);
        assert!(!r.fields.contains_key(ERR_FIELD));
    }

    #[test]
    fn json_marks_error_on_non_json_line() {
        let mut r = Record::new().with_line("startup: listening on :8080");
        parse_json(&mut r);
        // Record survives.
        assert_eq!(r.line.as_deref(), Some("startup: listening on :8080"));
        // And carries an error marker.
        assert_eq!(
            r.fields.get(ERR_FIELD).and_then(|v| v.as_str()),
            Some("JSONParserErr")
        );
        assert!(r.fields.contains_key(ERR_DETAILS_FIELD));
    }

    #[test]
    fn json_marks_error_on_array_root() {
        let mut r = Record::new().with_line("[1,2,3]");
        parse_json(&mut r);
        assert_eq!(
            r.fields.get(ERR_FIELD).and_then(|v| v.as_str()),
            Some("JSONParserErr")
        );
    }

    #[test]
    fn json_sanitizes_invalid_label_names() {
        let mut r = Record::new()
            .with_line(r#"{"2xx-count":5,"response.time":0.3,"ok key":1}"#);
        parse_json(&mut r);
        // Leading digit → underscore prefix; hyphens/dots/spaces → underscore.
        assert!(r.fields.contains_key("_2xx_count"));
        assert!(r.fields.contains_key("response_time"));
        assert!(r.fields.contains_key("ok_key"));
    }

    #[test]
    fn json_field_shadows_label_without_destroying_it() {
        let mut r = Record::new()
            .with_label("status", "running") // pre-existing label
            .with_line(r#"{"status":"500"}"#);
        parse_json(&mut r);
        // Original label preserved untouched (separate map).
        assert_eq!(r.label("status"), Some("running"));
        // Extracted value lands in fields under the same name; lookup()
        // prefers fields and surfaces the JSON value.
        assert_eq!(r.fields.get("status").unwrap(), "500");
        assert_eq!(r.lookup("status").as_deref(), Some("500"));
    }

    #[test]
    fn extraction_collision_between_two_stages_uses_suffix() {
        // First stage extracts `level`, second stage extracts `level`
        // again — the second value must not silently overwrite the first.
        let mut r = Record::new().with_line(r#"{"level":"info"}"#);
        parse_json(&mut r);
        assert_eq!(r.fields.get("level").unwrap(), "info");
        // Re-run a different parser that produces the same key.
        parse_regexp(&mut r, r"(?P<level>\w+)").unwrap();
        assert_eq!(r.fields.get("level").unwrap(), "info");
        assert_eq!(r.fields.get("level_extracted").unwrap(), "level");
    }

    #[test]
    fn logfmt_parses_quoted_and_unquoted() {
        let mut r = Record::new().with_line(r#"a=1 msg="hello world" k=v"#);
        parse_logfmt(&mut r);
        assert_eq!(r.fields.get("a").unwrap(), "1");
        assert_eq!(r.fields.get("msg").unwrap(), "hello world");
        assert_eq!(r.fields.get("k").unwrap(), "v");
        assert!(!r.fields.contains_key(ERR_FIELD));
    }

    #[test]
    fn logfmt_marks_error_on_unparseable_line() {
        let mut r = Record::new().with_line("plain text with no key value pairs");
        parse_logfmt(&mut r);
        assert_eq!(
            r.fields.get(ERR_FIELD).and_then(|v| v.as_str()),
            Some("LogfmtParserErr")
        );
    }

    #[test]
    fn pattern_extracts_named() {
        let mut r = Record::new().with_line("GET /index.html 200 1234".to_string());
        parse_pattern(&mut r, "<method> <path> <status> <_>").unwrap();
        assert_eq!(r.fields.get("method").unwrap(), "GET");
        assert_eq!(r.fields.get("path").unwrap(), "/index.html");
        assert_eq!(r.fields.get("status").unwrap(), "200");
        assert!(!r.fields.contains_key(ERR_FIELD));
    }

    #[test]
    fn pattern_marks_error_on_no_match() {
        // Template requires the literal "HTTP/" between status and version,
        // which the line does not contain — guaranteed non-match.
        let mut r = Record::new().with_line("totally different shape".to_string());
        parse_pattern(&mut r, "<method> <path> <status> HTTP/<version>").unwrap();
        assert_eq!(
            r.fields.get(ERR_FIELD).and_then(|v| v.as_str()),
            Some("PatternParserErr")
        );
    }

    #[test]
    fn regexp_named_groups() {
        let mut r = Record::new().with_line("err=oops code=500".to_string());
        parse_regexp(&mut r, r"err=(?P<err>\S+)\s+code=(?P<code>\d+)").unwrap();
        assert_eq!(r.fields.get("err").unwrap(), "oops");
        assert_eq!(r.fields.get("code").unwrap(), "500");
        assert!(!r.fields.contains_key(ERR_FIELD));
    }

    #[test]
    fn regexp_marks_error_on_no_match() {
        let mut r = Record::new().with_line("nothing here".to_string());
        parse_regexp(&mut r, r"err=(?P<err>\S+)").unwrap();
        assert_eq!(
            r.fields.get(ERR_FIELD).and_then(|v| v.as_str()),
            Some("RegexParserErr")
        );
    }

    #[test]
    fn sanitize_rules() {
        assert_eq!(sanitize_label_name(""), "_");
        assert_eq!(sanitize_label_name("ok"), "ok");
        assert_eq!(sanitize_label_name("ok_2"), "ok_2");
        assert_eq!(sanitize_label_name("trace:id"), "trace:id");
        assert_eq!(sanitize_label_name("2xx"), "_2xx");
        assert_eq!(sanitize_label_name("a-b.c d"), "a_b_c_d");
        assert_eq!(sanitize_label_name("café"), "caf_");
    }

    #[test]
    fn mixed_stream_no_record_dropped() {
        let lines = [
            r#"{"level":"info","msg":"ok"}"#,
            "startup banner: not json",
            r#"{"level":"error","status":500}"#,
            "[1,2,3]",
            r#"{"level":"warn"}"#,
        ];
        let mut records: Vec<Record> = lines
            .iter()
            .map(|l| Record::new().with_line(l.to_string()))
            .collect();
        for r in &mut records {
            parse_json(r);
        }
        // All 5 records still present.
        assert_eq!(records.len(), 5);
        // Two failures (lines 2 and 4), three successes.
        let failures = records
            .iter()
            .filter(|r| r.fields.contains_key(ERR_FIELD))
            .count();
        assert_eq!(failures, 2);
    }
}
