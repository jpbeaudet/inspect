//! Output contract: `SUMMARY / DATA / NEXT` blocks for human mode and
//! versioned line-delimited JSON envelopes for `--json`.

use anyhow::Result;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::error::{self, ExitKind};
use crate::query::{self, ndjson, QueryErrorKind};

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
    /// When true, [`Self::print`] and the dispatch
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
    /// Toggle `--quiet` on this renderer.
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
        use crate::transcript::emit_stdout;
        if !self.quiet {
            emit_stdout(&format!("SUMMARY: {}", self.summary));
        }
        if self.data.is_empty() {
            if !self.quiet {
                emit_stdout("DATA:    (none)");
            }
        } else if self.quiet {
            for line in &self.data {
                emit_stdout(line);
            }
        } else {
            emit_stdout("DATA:");
            for line in &self.data {
                emit_stdout(&format!("  {line}"));
            }
        }
        if !self.quiet {
            if self.next.is_empty() {
                emit_stdout("NEXT:    (none)");
            } else {
                emit_stdout("NEXT:");
                for line in &self.next {
                    emit_stdout(&format!("  {line}"));
                }
            }
        }
    }

    /// Phase 10.3 — render whichever output the user asked for.
    ///
    /// * `Human` falls back to [`Self::print`] (with full envelope).
    /// * `Json` emits one envelope per line via [`emit_value`] which
    ///   routes through `transcript::emit_stdout` (the earlier path
    ///   bypassed transcript via raw `println!` — fixed in this
    ///   commit alongside the `--select` plumbing per the
    ///   sweep-the-pattern policy in CLAUDE.md).
    /// * `Csv` / `Tsv` / `Yaml` / `Md` / `Table` / `Format` / `Raw`
    ///   delegate to [`crate::format::render::render_rows`] using the
    ///   buffered envelopes.
    ///
    /// `select` is the `--select` filter (taken by value
    /// so the dispatcher owns the borrow checker around per-row
    /// re-borrows + the end-of-stream slurp flush). When the filter
    /// is set and yields zero results across all rows + slurp flush,
    /// returns `Ok(ExitKind::NoMatches)` for exit code 1 — same
    /// "zero-results = exit 1" contract as `inspect query` and
    /// `OutputDoc::print_json`.
    pub fn dispatch(
        &self,
        fmt: &crate::format::OutputFormat,
        mut select: Option<ndjson::Filter>,
    ) -> Result<ExitKind> {
        use crate::format::OutputFormat as F;
        match fmt {
            F::Human => {
                self.print();
                Ok(ExitKind::Success)
            }
            F::Json => {
                let select_was_set = select.is_some();
                let mut emitted = false;
                for row in &self.rows {
                    if emit_value(row, select.as_mut())? {
                        emitted = true;
                    }
                }
                if flush_filter_with_status(select.as_mut())? {
                    emitted = true;
                }
                if select_was_set && !emitted {
                    Ok(ExitKind::NoMatches)
                } else {
                    Ok(ExitKind::Success)
                }
            }
            _ => {
                let next: Vec<NextStep> = self
                    .next
                    .iter()
                    .map(|s| NextStep::new(s.clone(), String::new()))
                    .collect();
                crate::format::render::render_rows(&self.rows, &self.summary, &next, fmt)
                    .map(|_| ExitKind::Success)
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
    /// Emit one envelope as a JSON line. The optional `filter` is the
    /// `--select` streaming filter — when present, the
    /// envelope is converted to `serde_json::Value`, the filter is
    /// applied, and the rendered output (if any) is emitted in place
    /// of the bare envelope.
    ///
    /// Per-frame errors:
    /// - runtime / raw-non-string → `error::emit` + skip this frame
    ///   (the verb keeps streaming; a per-frame error must not abort
    ///   a long-running `inspect run --stream` or `logs --follow`).
    /// - serialize errors → propagated as anyhow.
    ///
    /// Slurp mode: `filter.on_line` swallows the value into the slurp
    /// buffer and returns an empty string; emission is deferred until
    /// the streaming caller invokes [`flush_filter`].
    pub fn write(env: &Envelope, filter: Option<&mut ndjson::Filter>) -> Result<()> {
        let value = serde_json::to_value(env)
            .map_err(|e| anyhow::anyhow!("internal: envelope serialize failed: {e}"))?;
        emit_value(&value, filter).map(|_emitted| ())
    }
}

/// Emit one value through stdout, optionally filtered.
///
/// Returns `true` if the call wrote one or more lines to stdout. Slurp
/// mode and runtime/raw-non-string errors return `false` (slurp defers
/// to flush; errors emit via `error::emit` to stderr only). The bool
/// lets `Renderer::dispatch` track whether anything was emitted across
/// the buffered rows, so it can return `ExitKind::NoMatches` when a
/// `--select` filter swallows every row.
pub(crate) fn emit_value(
    value: &serde_json::Value,
    filter: Option<&mut ndjson::Filter>,
) -> Result<bool> {
    let Some(f) = filter else {
        let s = serde_json::to_string(value)
            .map_err(|e| anyhow::anyhow!("internal: serialize failed: {e}"))?;
        crate::transcript::emit_stdout(&s);
        return Ok(true);
    };
    match f.on_line(value) {
        Ok(s) if s.is_empty() => Ok(false),
        Ok(rendered) => {
            for line in rendered.lines() {
                crate::transcript::emit_stdout(line);
            }
            Ok(true)
        }
        Err(e) if e.kind == QueryErrorKind::Parse => {
            // Parse errors are caught at construction in
            // `select_filter()`; reaching here means a bug. Bubble
            // anyhow so it surfaces rather than silently dropping.
            Err(anyhow::anyhow!("filter parse: {}", e.message))
        }
        Err(e) => {
            let label = match e.kind {
                QueryErrorKind::RawNonString => "filter --raw",
                _ => "filter runtime",
            };
            error::emit(format!("{}: {}", label, e.message));
            Ok(false)
        }
    }
}

/// Flush a streaming `Filter`'s slurp buffer at end-of-
/// stream. No-op for per-frame mode (`finish` returns an empty string
/// in that case). Returns the per-frame `Ok(())` / `Ok(NoMatches)` /
/// `Err` shape so streaming verbs can propagate exit kinds uniformly.
///
/// Streaming verbs call this once after the per-frame loop ends:
/// ```text
/// for frame in stream { JsonOut::write(&env, select.as_mut())?; }
/// flush_filter(select.as_mut())?;
/// ```
pub fn flush_filter(filter: Option<&mut ndjson::Filter>) -> Result<()> {
    flush_filter_with_status(filter).map(|_| ())
}

/// Same as [`flush_filter`] but returns `true` when the slurp eval
/// produced output. Used by `Renderer::dispatch` so a slurp-mode
/// filter that yields a result counts toward the "emitted anything"
/// total (otherwise the per-row counter would always be 0 for slurp
/// and `dispatch` would return NoMatches even on a successful slurp).
pub(crate) fn flush_filter_with_status(filter: Option<&mut ndjson::Filter>) -> Result<bool> {
    let Some(f) = filter else { return Ok(false) };
    match f.finish() {
        Ok(s) if s.is_empty() => Ok(false),
        Ok(rendered) => {
            for line in rendered.lines() {
                crate::transcript::emit_stdout(line);
            }
            Ok(true)
        }
        Err(e) if e.kind == QueryErrorKind::Parse => {
            Err(anyhow::anyhow!("filter parse: {}", e.message))
        }
        Err(e) => {
            let label = match e.kind {
                QueryErrorKind::RawNonString => "filter --raw",
                _ => "filter runtime",
            };
            error::emit(format!("{}: {}", label, e.message));
            Ok(false)
        }
    }
}

/// Emit a single JSON envelope, optionally filtered.
///
/// The free-function counterpart to [`OutputDoc::print_json`] for
/// callers that hand-roll their `serde_json::Value` rather than going
/// through the [`OutputDoc`] builder (notably `inspect search`, whose
/// log/metric envelopes carry medium-specific shapes that don't fit
/// the SUMMARY/DATA/NEXT mold). Behavior is identical to
/// `OutputDoc::print_json`: same exit-code mapping, same parse/runtime
/// error handling, same slurp/raw rendering — see that doc-comment for
/// the full contract.
pub fn print_json_value(value: &Value, select: Option<(&str, bool, bool)>) -> Result<ExitKind> {
    let Some((filter, raw, slurp)) = select else {
        let s = serde_json::to_string(value)
            .map_err(|e| anyhow::anyhow!("internal: serialize failed: {e}"))?;
        crate::transcript::emit_stdout(&s);
        return Ok(ExitKind::Success);
    };

    let result = if slurp {
        query::eval_slurp(filter, std::slice::from_ref(value))
    } else {
        query::eval(filter, value)
    };
    let values = match result {
        Ok(v) => v,
        Err(e) if e.kind == QueryErrorKind::Parse => {
            return Err(anyhow::anyhow!("filter parse: {}", e.message));
        }
        Err(e) => {
            error::emit(format!("filter runtime: {}", e.message));
            return Ok(ExitKind::NoMatches);
        }
    };
    if values.is_empty() {
        return Ok(ExitKind::NoMatches);
    }
    let rendered = if raw {
        match query::render_raw(&values) {
            Ok(s) => s,
            Err(e) => {
                error::emit(format!("filter --raw: {}", e.message));
                return Ok(ExitKind::NoMatches);
            }
        }
    } else {
        query::render_compact(&values)
    };
    for line in rendered.lines() {
        crate::transcript::emit_stdout(line);
    }
    Ok(ExitKind::Success)
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
    /// When true, the human renderer suppresses the
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

    /// Builder hook for the global `--quiet` flag.
    pub fn with_quiet(mut self, quiet: bool) -> Self {
        self.quiet = quiet;
        self
    }

    /// Print to stdout as a single-line JSON envelope.
    ///
    /// `select` is the `--select` triple: filter source,
    /// raw-rendering flag, slurp flag. `None` reproduces the v0.1.2
    /// behavior (one envelope, full shape). `Some` evaluates the
    /// filter against the envelope and emits the rendered result.
    ///
    /// Exit-code mapping (returned for the verb to propagate):
    /// - parse error → `Err(anyhow!("filter parse: …"))` → exit 2
    ///   (top-level `error::emit` adds the canonical "error: " prefix
    ///   and, once the C3 catalog row lands, the
    ///   `see: inspect help select` cross-link).
    /// - runtime / `--select-raw` non-string → `error::emit` + return
    ///   `Ok(ExitKind::NoMatches)` → exit 1.
    /// - zero results → `Ok(NoMatches)` → exit 1.
    /// - success → `Ok(Success)` → exit 0.
    pub fn print_json(&self, select: Option<(&str, bool, bool)>) -> Result<ExitKind> {
        let value = serde_json::to_value(self)
            .map_err(|e| anyhow::anyhow!("internal: envelope serialize failed: {e}"))?;
        print_json_value(&value, select)
    }

    /// Print as SUMMARY/DATA/NEXT human blocks. Caller supplies the
    /// renderer for `data` lines because the structured shape varies
    /// per command. `next` is rendered as `cmd  -- rationale`.
    pub fn print_human(&self, data_lines: &[String]) {
        use crate::transcript::emit_stdout;
        if !self.quiet {
            emit_stdout(&format!("SUMMARY: {}", self.summary));
        }
        if data_lines.is_empty() {
            if !self.quiet {
                emit_stdout("DATA:    (none)");
            }
        } else if self.quiet {
            // Pipe-clean DATA only — no envelope, no
            // indentation prefix, just the data lines as-is.
            for l in data_lines {
                emit_stdout(l);
            }
        } else {
            emit_stdout("DATA:");
            for l in data_lines {
                emit_stdout(&format!("  {l}"));
            }
        }
        if !self.quiet {
            if self.next.is_empty() {
                emit_stdout("NEXT:    (none)");
            } else {
                emit_stdout("NEXT:");
                for n in &self.next {
                    emit_stdout(&format!("  {}  -- {}", n.cmd, n.rationale));
                }
            }
        }
    }
}
