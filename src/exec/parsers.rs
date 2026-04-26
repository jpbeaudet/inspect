//! Stream parsers: `| json`, `| logfmt`, `| pattern "..."`, `| regexp "..."`.

use regex::Regex;
use serde_json::Value;

use crate::exec::record::Record;

/// Apply `| json` to one record: parse `record.line` as JSON and merge
/// the resulting object's keys into `fields`. No-op if the line isn't
/// valid JSON or isn't an object.
pub fn parse_json(rec: &mut Record) {
    let Some(line) = rec.line.as_deref() else { return };
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(line) else {
        return;
    };
    for (k, v) in map {
        rec.fields.insert(k, v);
    }
}

/// Apply `| logfmt`: parse `key=value` pairs (space-separated, optional
/// quoted values).
pub fn parse_logfmt(rec: &mut Record) {
    let Some(line) = rec.line.as_deref() else { return };
    let bytes = line.as_bytes();
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
            rec.fields.insert(key, Value::String(value));
        }
    }
}

/// Apply `| pattern "<...>"` Loki-style: any `<name>` placeholder
/// captures up to the next literal text.
///
/// This is a strict translation: `<_>` skips, `<name>` captures into
/// `fields["name"]`. Literal text between placeholders is matched
/// verbatim. Whitespace in the template matches any non-empty run of
/// whitespace in the input.
pub fn parse_pattern(rec: &mut Record, template: &str) -> Result<(), String> {
    let Some(line) = rec.line.clone() else { return Ok(()) };
    let re = compile_pattern(template)?;
    if let Some(caps) = re.captures(&line) {
        for name in re.capture_names().flatten() {
            if let Some(m) = caps.name(name) {
                rec.fields.insert(name.to_string(), Value::String(m.as_str().into()));
            }
        }
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
    if let Some(caps) = re.captures(&line) {
        for name in re.capture_names().flatten() {
            if let Some(m) = caps.name(name) {
                rec.fields.insert(name.to_string(), Value::String(m.as_str().into()));
            }
        }
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
    }
    #[test]
    fn logfmt_parses_quoted_and_unquoted() {
        let mut r = Record::new().with_line(r#"a=1 msg="hello world" k=v"#);
        parse_logfmt(&mut r);
        assert_eq!(r.fields.get("a").unwrap(), "1");
        assert_eq!(r.fields.get("msg").unwrap(), "hello world");
        assert_eq!(r.fields.get("k").unwrap(), "v");
    }
    #[test]
    fn pattern_extracts_named() {
        let mut r = Record::new().with_line("GET /index.html 200 1234".to_string());
        parse_pattern(&mut r, "<method> <path> <status> <_>").unwrap();
        assert_eq!(r.fields.get("method").unwrap(), "GET");
        assert_eq!(r.fields.get("path").unwrap(), "/index.html");
        assert_eq!(r.fields.get("status").unwrap(), "200");
    }
    #[test]
    fn regexp_named_groups() {
        let mut r = Record::new().with_line("err=oops code=500".to_string());
        parse_regexp(&mut r, r"err=(?P<err>\S+)\s+code=(?P<code>\d+)").unwrap();
        assert_eq!(r.fields.get("err").unwrap(), "oops");
        assert_eq!(r.fields.get("code").unwrap(), "500");
    }
}
