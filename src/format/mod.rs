//! Phase 10.3 — universal output-format dispatch.
//!
//! Every user-facing command resolves its output format through
//! [`FormatArgs::resolve`], which enforces:
//!
//! 1. Mutual exclusivity across the format flags (bible §10.3).
//! 2. `NO_COLOR` env / `--no-color` flag handling.
//! 3. TTY detection (color + decoration auto-off when stdout isn't a
//!    terminal).
//!
//! Renderers live next to this module: `render_doc` for the
//! command-level `OutputDoc` used by aggregate verbs (status, health,
//! why, connectivity, recipe, search) and `render_rows` for per-record
//! verbs (ps, ports, images, volumes, network, list).

use anyhow::{anyhow, Result};
use clap::Args;

pub mod render;
pub mod safe;
pub mod template;

/// Resolved output format. Carries display state (color, decoration)
/// alongside the chosen format so renderers don't have to re-derive it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputFormat {
    /// Default human format with color + decoration when stdout is a TTY.
    Human,
    /// Plain ASCII table (no box-drawing, no color).
    Table,
    /// GitHub-flavored Markdown table.
    Md,
    /// Line-delimited JSON (one record per line). Aliased by `--jsonl`.
    Json,
    /// RFC 4180 CSV with header row.
    Csv,
    /// Tab-separated values with header row.
    Tsv,
    /// YAML document(s).
    Yaml,
    /// Go-style template applied per record.
    Format(String),
    /// Strip all decoration; just the content.
    Raw,
}

impl OutputFormat {
    /// True for formats that retain SUMMARY/DATA/NEXT decoration
    /// (default human, `--table`, `--md`).
    pub fn shows_envelope(&self) -> bool {
        matches!(
            self,
            OutputFormat::Human | OutputFormat::Table | OutputFormat::Md
        )
    }
}

/// Reusable clap block. Embed in any command via `#[command(flatten)]`.
///
/// Flags are defined as plain booleans (and one `Option<String>` for
/// `--format`) so clap parses them independently; mutual-exclusivity is
/// enforced in [`Self::resolve`] with a single, friendly error message
/// (matching the bible's exact wording).
#[derive(Debug, Clone, Args, Default)]
pub struct FormatArgs {
    /// Emit line-delimited JSON (one record per line).
    #[arg(long, global = false)]
    pub json: bool,
    /// Alias for `--json` (explicit NDJSON).
    #[arg(long)]
    pub jsonl: bool,
    /// RFC 4180 CSV with header row.
    #[arg(long)]
    pub csv: bool,
    /// Tab-separated values with header row.
    #[arg(long)]
    pub tsv: bool,
    /// YAML document(s).
    #[arg(long)]
    pub yaml: bool,
    /// Plain ASCII table (no box-drawing, no color).
    #[arg(long)]
    pub table: bool,
    /// GitHub-flavored Markdown table.
    #[arg(long)]
    pub md: bool,
    /// Go-style template applied per record (e.g. `{{.service}}`).
    #[arg(long, value_name = "TEMPLATE")]
    pub format: Option<String>,
    /// Strip all decoration; raw content only.
    #[arg(long)]
    pub raw: bool,
    /// Suppress ANSI color codes in human / table output.
    #[arg(long)]
    pub no_color: bool,
    /// Suppress the trailing `SUMMARY:` and `NEXT:`
    /// lines so output is safe to pipe into `tail` / `head` /
    /// `grep -A` without trailer corruption. Mutually exclusive with
    /// `--json` / `--jsonl` (those formats are already trailer-free).
    #[arg(long, conflicts_with_all = ["json", "jsonl"])]
    pub quiet: bool,

    /// Apply a jq-language filter to the JSON output
    /// before emission. Requires `--json` or `--jsonl`. The filter
    /// language is the same one `jq` (and `jaq`) implement; see
    /// `inspect help select` for the full reference.
    ///
    /// Examples:
    ///   --json --select '.summary' -r
    ///   --json --select '.data.entries | length'
    ///   --json --select '.data.services[] | select(.healthy == false)'
    #[arg(long, value_name = "FILTER")]
    pub select: Option<String>,

    /// Emit string yields unquoted (the `jq -r` shape)
    /// instead of compact JSON. Errors with exit code 1 if any yield
    /// is not a string — the alternative (silently quoting non-strings
    /// as JSON) would let literal `"` characters reach `xargs` /
    /// `wc -l` / shell loops and cause subtle quoting bugs. Requires
    /// `--select`.
    #[arg(long, requires = "select")]
    pub select_raw: bool,

    /// Collect every NDJSON value from the stream into a
    /// single array before evaluating the filter (the `jq -s` shape).
    /// Lets `length` / `map(.x) | unique` / `reduce .[] as $x (…)`
    /// work over the whole stream. Memory is O(stream); use sparingly
    /// on unbounded streams. Requires `--select`.
    #[arg(long, requires = "select")]
    pub select_slurp: bool,
}

impl FormatArgs {
    /// Resolve to a single [`OutputFormat`]. Errors when more than one
    /// mutually-exclusive flag is set, with the bible-mandated message:
    /// `error: --json and --csv are mutually exclusive. Pick one output
    /// format.`
    pub fn resolve(&self) -> Result<OutputFormat> {
        // Build a Vec of (flag-name, is-set) so we can report which two
        // flags collided.
        let candidates: [(&'static str, bool); 9] = [
            ("--json", self.json),
            ("--jsonl", self.jsonl),
            ("--csv", self.csv),
            ("--tsv", self.tsv),
            ("--yaml", self.yaml),
            ("--table", self.table),
            ("--md", self.md),
            ("--format", self.format.is_some()),
            ("--raw", self.raw),
        ];
        let set: Vec<&'static str> = candidates
            .iter()
            .filter(|(_, on)| *on)
            .map(|(n, _)| *n)
            .collect();
        if set.len() > 1 {
            return Err(anyhow!(
                "{} and {} are mutually exclusive. Pick one output format.",
                set[0],
                set[1]
            ));
        }
        let fmt = if self.json || self.jsonl {
            OutputFormat::Json
        } else if self.csv {
            OutputFormat::Csv
        } else if self.tsv {
            OutputFormat::Tsv
        } else if self.yaml {
            OutputFormat::Yaml
        } else if self.table {
            OutputFormat::Table
        } else if self.md {
            OutputFormat::Md
        } else if let Some(t) = &self.format {
            OutputFormat::Format(t.clone())
        } else if self.raw {
            OutputFormat::Raw
        } else {
            OutputFormat::Human
        };
        // `--select` is a JSON-only filter. Reject it
        // against any non-JSON-class format with the same anyhow →
        // exit 2 path the format-mutex check uses. The `--quiet`
        // mutex is enforced transitively: `--quiet` is already
        // `conflicts_with_all = ["json", "jsonl"]` at clap level, so
        // `--quiet --select` falls out of `--json` and lands here.
        if self.select.is_some() && !matches!(fmt, OutputFormat::Json) {
            return Err(anyhow!(
                "--select requires --json or --jsonl (output format is {fmt:?})"
            ));
        }
        Ok(fmt)
    }

    /// Convenience: true when the user picked `--json` or `--jsonl`.
    /// Lets command bodies keep their existing `if args.format.is_json()`
    /// fast paths while migrating gradually.
    pub fn is_json(&self) -> bool {
        self.json || self.jsonl
    }

    /// Build a streaming `query::ndjson::Filter` from the
    /// `--select` / `--select-raw` / `--select-slurp` triple. Returns
    /// `Ok(None)` when no filter was requested, `Ok(Some(filter))` on
    /// successful compile, and an anyhow error keyed with the
    /// canonical "filter parse:" prefix on parse failure (so main's
    /// `error::emit` renders the standard `error: filter parse: …`
    /// shape, with the C3 catalog row attaching the cross-link to
    /// `inspect help select`).
    ///
    /// Streaming verbs (logs, grep, run --stream, …) call this once
    /// at verb entry and thread `select.as_mut()` through their
    /// per-frame loop into [`crate::verbs::output::JsonOut::write`].
    pub fn select_filter(&self) -> Result<Option<crate::query::ndjson::Filter>> {
        let Some(filter) = self.select.as_deref() else {
            return Ok(None);
        };
        crate::query::ndjson::Filter::new(filter, self.select_raw, self.select_slurp)
            .map(Some)
            .map_err(|e| anyhow!("filter parse: {}", e.message))
    }

    /// Single-shot variant for envelope verbs that emit
    /// one `OutputDoc` per invocation. Returns the (filter, raw,
    /// slurp) triple as plain references so the envelope chokepoint
    /// in [`crate::verbs::output::OutputDoc::print_json`] can call
    /// `query::eval` / `eval_slurp` + `render_compact` / `render_raw`
    /// directly without round-tripping through the streaming `Filter`
    /// (which would need a `&mut` borrow that doesn't fit a single-
    /// shot consumer).
    pub fn select_spec(&self) -> Option<(&str, bool, bool)> {
        self.select
            .as_deref()
            .map(|f| (f, self.select_raw, self.select_slurp))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> FormatArgs {
        FormatArgs::default()
    }

    #[test]
    fn default_is_human() {
        assert_eq!(args().resolve().unwrap(), OutputFormat::Human);
    }

    #[test]
    fn json_and_jsonl_both_resolve_to_json() {
        let mut a = args();
        a.json = true;
        assert_eq!(a.resolve().unwrap(), OutputFormat::Json);
        let mut a = args();
        a.jsonl = true;
        assert_eq!(a.resolve().unwrap(), OutputFormat::Json);
    }

    #[test]
    fn mutex_error_carries_both_flags() {
        let mut a = args();
        a.json = true;
        a.csv = true;
        let err = a.resolve().unwrap_err().to_string();
        assert!(err.contains("--json"));
        assert!(err.contains("--csv"));
        assert!(err.contains("mutually exclusive"));
    }

    #[test]
    fn template_format_carries_template_string() {
        let mut a = args();
        a.format = Some("{{.x}}".into());
        assert_eq!(a.resolve().unwrap(), OutputFormat::Format("{{.x}}".into()));
    }

    #[test]
    fn raw_resolves() {
        let mut a = args();
        a.raw = true;
        assert_eq!(a.resolve().unwrap(), OutputFormat::Raw);
    }

    // --- `--select` validation + helpers ----------------------------------

    #[test]
    fn select_with_json_resolves() {
        let mut a = args();
        a.json = true;
        a.select = Some(".summary".into());
        assert_eq!(a.resolve().unwrap(), OutputFormat::Json);
    }

    #[test]
    fn select_with_jsonl_resolves() {
        let mut a = args();
        a.jsonl = true;
        a.select = Some(".line".into());
        assert_eq!(a.resolve().unwrap(), OutputFormat::Json);
    }

    #[test]
    fn select_without_json_class_format_errors() {
        let mut a = args();
        // Default format is Human — `--select '.x'` should reject.
        a.select = Some(".x".into());
        let err = a.resolve().unwrap_err().to_string();
        assert!(err.contains("--select"), "got: {err}");
        assert!(err.contains("--json or --jsonl"), "got: {err}");
    }

    #[test]
    fn select_with_csv_errors() {
        let mut a = args();
        a.csv = true;
        a.select = Some(".x".into());
        let err = a.resolve().unwrap_err().to_string();
        assert!(err.contains("--select"));
        assert!(err.contains("--json or --jsonl"));
    }

    #[test]
    fn select_filter_returns_none_when_unset() {
        let a = args();
        let f = a.select_filter().unwrap();
        assert!(f.is_none());
    }

    #[test]
    fn select_filter_compiles_valid_filter() {
        let mut a = args();
        a.select = Some(".summary".into());
        let f = a.select_filter().unwrap();
        assert!(f.is_some());
    }

    #[test]
    fn select_filter_parse_error_carries_canonical_prefix() {
        let mut a = args();
        // Unbalanced bracket → jaq parse error.
        a.select = Some(".[".into());
        let err = a.select_filter().unwrap_err().to_string();
        assert!(
            err.starts_with("filter parse:"),
            "expected canonical 'filter parse:' prefix, got: {err}"
        );
    }

    #[test]
    fn select_spec_returns_triple_when_set() {
        let mut a = args();
        a.select = Some(".data".into());
        a.select_raw = true;
        a.select_slurp = false;
        let spec = a.select_spec().unwrap();
        assert_eq!(spec, (".data", true, false));
    }

    #[test]
    fn select_spec_returns_none_when_unset() {
        let a = args();
        assert!(a.select_spec().is_none());
    }
}
