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
//! Renderers live next to this module: [`render_doc`] for the
//! command-level [`OutputDoc`] used by aggregate verbs (status, health,
//! why, connectivity, recipe, search) and [`render_rows`] for per-record
//! verbs (ps, ports, images, volumes, network, list).

use anyhow::{anyhow, Result};
use clap::Args;

pub mod render;
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

    /// True for the `--json`/`--jsonl` family.
    pub fn is_json(&self) -> bool {
        matches!(self, OutputFormat::Json)
    }

    /// True if this format should never emit ANSI color codes regardless
    /// of TTY / NO_COLOR state.
    pub fn always_plain(&self) -> bool {
        !matches!(self, OutputFormat::Human)
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
        Ok(fmt)
    }

    /// Convenience: true when the user picked `--json` or `--jsonl`.
    /// Lets command bodies keep their existing `if args.format.is_json()`
    /// fast paths while migrating gradually.
    pub fn is_json(&self) -> bool {
        self.json || self.jsonl
    }
}

/// True if the running process should suppress ANSI color codes.
/// Honors the upstream [`NO_COLOR`](https://no-color.org/) standard,
/// the `--no-color` flag, and TTY presence.
pub fn no_color_active(no_color_flag: bool) -> bool {
    if no_color_flag {
        return true;
    }
    if std::env::var_os("NO_COLOR").is_some() {
        return true;
    }
    if !is_stdout_tty() {
        return true;
    }
    false
}

/// Best-effort check whether stdout is attached to a terminal. We avoid
/// pulling in a new dependency and rely on `isatty(1)` via libc on
/// unix; on non-unix we return `true` so behavior is unchanged.
pub fn is_stdout_tty() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: `isatty` is a pure check on a file descriptor that
        // touches no user-visible state.
        unsafe { libc::isatty(1) == 1 }
    }
    #[cfg(not(unix))]
    {
        true
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
        assert_eq!(
            a.resolve().unwrap(),
            OutputFormat::Format("{{.x}}".into())
        );
    }

    #[test]
    fn raw_resolves() {
        let mut a = args();
        a.raw = true;
        assert_eq!(a.resolve().unwrap(), OutputFormat::Raw);
    }

    #[test]
    fn no_color_flag_wins() {
        // We can't safely toggle env in tests; just check the flag path.
        assert!(no_color_active(true));
    }
}
