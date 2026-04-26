//! Unified record model produced by readers and consumed by the pipeline.
//!
//! Bible §10.1: "Every record carries `schema_version`, `_source`,
//! `_medium`, `server`, `service`, plus medium-specific fields."

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One unit of data flowing through a query pipeline.
///
/// `labels` are the addressing dimensions (server/service/source/...).
/// `line` is the raw textual record when applicable (logs, file lines).
/// `fields` are parsed structured fields produced by stages such as
/// `| json`, `| logfmt`, `| pattern`, `| regexp`. Backends may also
/// pre-populate fields for non-textual sources (state/volume/image/...).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Record {
    pub labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, Value>,
    /// Optional record timestamp (millis since epoch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts_ms: Option<i64>,
}

impl Record {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_label(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.labels.insert(k.into(), v.into());
        self
    }
    pub fn with_line(mut self, line: impl Into<String>) -> Self {
        self.line = Some(line.into());
        self
    }
    pub fn label(&self, k: &str) -> Option<&str> {
        self.labels.get(k).map(String::as_str)
    }
    /// Resolve a name first against parsed fields, then labels.
    /// This is what `$field$` interpolation and parsed-field filters use.
    pub fn lookup(&self, name: &str) -> Option<String> {
        if let Some(v) = self.fields.get(name) {
            return Some(value_as_string(v));
        }
        self.labels.get(name).cloned()
    }
}

/// Best-effort scalar rendering for `$field$` interpolation and field
/// filter coercion.
pub fn value_as_string(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(_) | Value::Object(_) => v.to_string(),
    }
}
