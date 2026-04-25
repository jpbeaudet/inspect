//! Output contract: `SUMMARY / DATA / NEXT` blocks for human mode and
//! versioned line-delimited JSON envelopes for `--json`.

use serde::Serialize;
use serde_json::{Map, Value};

/// Schema version stamped on every JSON record. Bumped on breaking
/// changes (bible §10).
pub const SCHEMA_VERSION: u32 = 1;

/// Human renderer state. Buffers SUMMARY/DATA/NEXT and prints all at once.
#[derive(Debug, Default)]
pub struct Renderer {
    pub summary: String,
    pub data: Vec<String>,
    pub next: Vec<String>,
}

impl Renderer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn summary(&mut self, s: impl Into<String>) -> &mut Self {
        self.summary = s.into();
        self
    }
    pub fn data_line(&mut self, s: impl Into<String>) -> &mut Self {
        self.data.push(s.into());
        self
    }
    pub fn next(&mut self, s: impl Into<String>) -> &mut Self {
        self.next.push(s.into());
        self
    }
    pub fn print(&self) {
        println!("SUMMARY: {}", self.summary);
        if self.data.is_empty() {
            println!("DATA:    (none)");
        } else {
            println!("DATA:");
            for line in &self.data {
                println!("  {line}");
            }
        }
        if self.next.is_empty() {
            println!("NEXT:    (none)");
        } else {
            println!("NEXT:");
            for line in &self.next {
                println!("  {line}");
            }
        }
    }
}

/// JSON record. Carries the bible's reserved fields plus medium-specific
/// extras. Serialized one-per-line by [`JsonOut`].
#[derive(Debug, Clone, Serialize)]
pub struct Envelope {
    pub schema_version: u32,
    #[serde(rename = "_source")]
    pub source: String,
    #[serde(rename = "_medium")]
    pub medium: String,
    pub server: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Envelope {
    pub fn new(server: impl Into<String>, medium: &str, source: impl Into<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            source: source.into(),
            medium: medium.to_string(),
            server: server.into(),
            service: None,
            extra: Map::new(),
        }
    }
    pub fn with_service(mut self, svc: impl Into<String>) -> Self {
        self.service = Some(svc.into());
        self
    }
    pub fn put(mut self, k: &str, v: impl Into<Value>) -> Self {
        self.extra.insert(k.to_string(), v.into());
        self
    }
}

/// Line-delimited JSON sink. Writes to stdout, flushing on each record.
pub struct JsonOut;

impl JsonOut {
    pub fn write(env: &Envelope) {
        if let Ok(s) = serde_json::to_string(env) {
            println!("{s}");
        }
    }
}
