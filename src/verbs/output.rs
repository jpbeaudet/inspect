//! Output contract: `SUMMARY / DATA / NEXT` blocks for human mode and
//! versioned line-delimited JSON envelopes for `--json`.

use serde::Serialize;
use serde_json::{Map, Value};

/// Schema version stamped on every JSON record. Bumped on breaking
/// changes (bible §10).
pub const SCHEMA_VERSION: u32 = 1;

/// Human renderer state. Buffers SUMMARY/DATA/NEXT and prints all at once.
///
/// In Phase 10.3 the renderer additionally buffers per-record envelopes
/// in [`Self::rows`] so the universal format dispatcher
/// ([`crate::format::render::render_rows`]) can re-render them as
/// CSV/TSV/YAML/Markdown/templates/raw without each verb having to
/// branch on every flag.
#[derive(Debug, Default)]
pub struct Renderer {
    pub summary: String,
    pub data: Vec<String>,
    pub next: Vec<String>,
    pub rows: Vec<Value>,
    /// F7.4 (v0.1.3): when true, [`Self::print`] and the dispatch
    /// helpers suppress the `SUMMARY:` and `NEXT:` envelope.
    pub quiet: bool,
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
    /// F7.4 (v0.1.3): toggle `--quiet` on this renderer.
    pub fn quiet(&mut self, q: bool) -> &mut Self {
        self.quiet = q;
        self
    }
    /// Phase 10.3 — buffer the per-record envelope so format dispatch
    /// can re-render it as CSV / TSV / YAML / Markdown / template / raw.
    pub fn push_row(&mut self, env: &Envelope) -> &mut Self {
        if let Ok(v) = serde_json::to_value(env) {
            self.rows.push(v);
        }
        self
    }
    pub fn print(&self) {
        if !self.quiet {
            println!("SUMMARY: {}", self.summary);
        }
        if self.data.is_empty() {
            if !self.quiet {
                println!("DATA:    (none)");
            }
        } else if self.quiet {
            for line in &self.data {
                println!("{line}");
            }
        } else {
            println!("DATA:");
            for line in &self.data {
                println!("  {line}");
            }
        }
        if !self.quiet {
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

    /// Phase 10.3 — render whichever output the user asked for.
    ///
    /// * `Human` falls back to [`Self::print`] (with full envelope).
    /// * `Json` emits one envelope per line via the existing
    ///   line-delimited writer (backward-compatible with all pre-10.3
    ///   tests).
    /// * `Csv` / `Tsv` / `Yaml` / `Md` / `Table` / `Format` / `Raw`
    ///   delegate to [`crate::format::render::render_rows`] using the
    ///   buffered envelopes.
    pub fn dispatch(&self, fmt: &crate::format::OutputFormat) -> anyhow::Result<()> {
        use crate::format::OutputFormat as F;
        match fmt {
            F::Human => {
                self.print();
                Ok(())
            }
            F::Json => {
                for row in &self.rows {
                    println!("{}", serde_json::to_string(row)?);
                }
                Ok(())
            }
            _ => {
                let next: Vec<NextStep> = self
                    .next
                    .iter()
                    .map(|s| NextStep::new(s.clone(), String::new()))
                    .collect();
                crate::format::render::render_rows(&self.rows, &self.summary, &next, fmt)
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

// -----------------------------------------------------------------------------
// Phase 10 — unified command-level envelope (bible §11).
// -----------------------------------------------------------------------------

/// One follow-up suggestion attached to an [`OutputDoc`] in the `next`
/// array. `cmd` is what the operator should run (or copy/paste);
/// `rationale` is a one-sentence explanation of why.
#[derive(Debug, Clone, Serialize)]
pub struct NextStep {
    pub cmd: String,
    pub rationale: String,
}

impl NextStep {
    pub fn new(cmd: impl Into<String>, rationale: impl Into<String>) -> Self {
        Self {
            cmd: cmd.into(),
            rationale: rationale.into(),
        }
    }
}

/// Single command-level output document. Used for aggregate / summary
/// commands (`status`, `health`, `why`, `connectivity`, `recipe`,
/// `search`). Streaming commands (`logs`, `grep`, etc.) use the
/// per-record [`Envelope`] from §10 instead.
#[derive(Debug, Clone, Serialize)]
pub struct OutputDoc {
    pub schema_version: u32,
    pub summary: String,
    pub data: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next: Vec<NextStep>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub meta: Map<String, Value>,
    /// F7.4 (v0.1.3): when true, the human renderer suppresses the
    /// `SUMMARY:` and `NEXT:` envelope lines so stdout is safe to
    /// pipe into `tail` / `head` / `grep -A`. Never serialized
    /// (skip-on-default + skip when false) — JSON output is already
    /// trailer-free and `--quiet` is mutually exclusive with `--json`.
    #[serde(skip)]
    pub quiet: bool,
}

impl OutputDoc {
    pub fn new(summary: impl Into<String>, data: Value) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            summary: summary.into(),
            data,
            next: Vec::new(),
            meta: Map::new(),
            quiet: false,
        }
    }

    pub fn push_next(&mut self, n: NextStep) -> &mut Self {
        self.next.push(n);
        self
    }

    pub fn with_meta(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.meta.insert(key.to_string(), value.into());
        self
    }

    /// F7.4 (v0.1.3): builder hook for the global `--quiet` flag.
    pub fn with_quiet(mut self, quiet: bool) -> Self {
        self.quiet = quiet;
        self
    }

    /// Print to stdout: a single JSON line for `--json` callers.
    pub fn print_json(&self) {
        if let Ok(s) = serde_json::to_string(self) {
            println!("{s}");
        }
    }

    /// Print as SUMMARY/DATA/NEXT human blocks. Caller supplies the
    /// renderer for `data` lines because the structured shape varies
    /// per command. `next` is rendered as `cmd  -- rationale`.
    pub fn print_human(&self, data_lines: &[String]) {
        if !self.quiet {
            println!("SUMMARY: {}", self.summary);
        }
        if data_lines.is_empty() {
            if !self.quiet {
                println!("DATA:    (none)");
            }
        } else if self.quiet {
            // F7.4: pipe-clean DATA only — no envelope, no
            // indentation prefix, just the data lines as-is.
            for l in data_lines {
                println!("{l}");
            }
        } else {
            println!("DATA:");
            for l in data_lines {
                println!("  {l}");
            }
        }
        if !self.quiet {
            if self.next.is_empty() {
                println!("NEXT:    (none)");
            } else {
                println!("NEXT:");
                for n in &self.next {
                    println!("  {}  -- {}", n.cmd, n.rationale);
                }
            }
        }
    }
}
