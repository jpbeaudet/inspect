//! Output rendering: compact (one JSON document per line, the
//! `jq -c` shape) and raw (unquoted UTF-8 for string yields, the
//! `jq -r` shape — error on non-string yields rather than emitting
//! a quoted JSON form, so operators piping to `xargs` / `wc -l`
//! cannot accidentally leak literal `"` characters).

use serde_json::Value;

use super::QueryError;

pub fn render_compact(values: &[Value]) -> String {
    let mut out = String::new();
    for v in values {
        out.push_str(&serde_json::to_string(v).unwrap_or_default());
        out.push('\n');
    }
    out
}

pub fn render_raw(values: &[Value]) -> Result<String, QueryError> {
    let mut out = String::new();
    for (idx, v) in values.iter().enumerate() {
        match v {
            Value::String(s) => {
                out.push_str(s);
                out.push('\n');
            }
            other => {
                return Err(QueryError::raw_non_string(idx, kind_name(other)));
            }
        }
    }
    Ok(out)
}

fn kind_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
