//! HP-4 — `inspect help --json` machine contract.
//!
//! Emits a single, byte-stable JSON document that exposes the entire
//! help surface for LLM agents, MCP servers, and CI tooling. Schema
//! version is incremented deliberately (see plan §10).
//!
//! Stability rules:
//! * keys in every object are emitted in a *fixed* lexical order so
//!   the snapshot test in `tests/help_json_snapshot.rs` is byte-stable
//!   without depending on hashmap insertion order;
//! * arrays preserve registry order (topics in catalog order, commands
//!   in clap definition order, flags in clap definition order);
//! * we use `serde_json::Value` only for *parsing* (the snapshot test
//!   round-trips through it); the writer is hand-rolled for full
//!   control over key ordering and pretty/compact toggling.
//!
//! The schema is versioned by [`SCHEMA_VERSION`]. Bumping it is a
//! deliberate act and requires updating the golden snapshot (see
//! [`crate::help::json::SCHEMA_VERSION`] doc comment for procedure).

use clap::CommandFactory;
use std::fmt::Write as _;
use std::io::IsTerminal;

use crate::cli::Cli;
use crate::help::topics::{see_also_line, topics_for_verb, TOPICS};

/// The major schema version. Increment only when the document shape
/// changes in a backwards-incompatible way.
///
/// Bump procedure (plan §10):
///   1. Increment this constant.
///   2. Run `cargo test help_json_snapshot -- --ignored` to regenerate
///      `tests/snapshots/help.v<N>.json`.
///   3. Document the diff in `CHANGELOG.md`.
///   4. Add a row to the `JSON --json` section of the bible.
pub const SCHEMA_VERSION: u32 = 1;

/// Render the full `inspect help --json` envelope.
///
/// `pretty` controls 2-space indentation. The dispatcher passes
/// `pretty = stdout.is_terminal()` so humans get readable output and
/// pipes / NDJSON consumers get compact lines.
pub fn render_full(pretty: bool) -> String {
    let mut w = JsonWriter::new(pretty);
    w.begin_object();
    w.kv_str("schema_version", &SCHEMA_VERSION.to_string());
    // Above is numeric — overwrite with raw write to skip quoting.
    w = w.replace_last_value(&SCHEMA_VERSION.to_string());
    w.kv_string("binary_version", env!("CARGO_PKG_VERSION"));
    w.kv_string("binary_name", env!("CARGO_PKG_NAME"));

    // -- topics ----------------------------------------------------------
    w.begin_array_field("topics");
    for t in TOPICS {
        w.begin_object();
        w.kv_string("id", t.id);
        w.kv_string("title", topic_title(t.body.unwrap_or("")));
        w.kv_string("summary", t.summary);
        w.field_array_of_strings("examples", &topic_examples(t.body.unwrap_or("")));
        w.field_array_of_strings("see_also", &topic_see_also(t.body.unwrap_or("")));
        w.field_array_of_strings("verbs", &crate::help::topics::verbs_for(t.id));
        w.end_object();
    }
    w.end_array();

    // -- commands --------------------------------------------------------
    w.begin_object_field("commands");
    let cli = Cli::command();
    let mut subs: Vec<&clap::Command> = cli.get_subcommands().collect();
    // Preserve clap's declaration order, but skip clap-injected `help`
    // (we ship our own; HP-2 disables clap's anyway).
    subs.retain(|c| c.get_name() != "help" || c.get_about().is_some());
    for sub in &subs {
        let name = sub.get_name();
        w.begin_object_field(name);
        w.kv_string("name", name);
        w.kv_string(
            "summary",
            &sub.get_about().map(|s| s.to_string()).unwrap_or_default(),
        );
        let aliases: Vec<&str> = sub.get_visible_aliases().collect();
        w.field_array_of_strings("aliases", &aliases);
        w.field_array_of_strings(
            "examples",
            &command_examples(sub)
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>(),
        );
        w.field_array_of_strings(
            "see_also",
            &topics_for_verb(name).iter().copied().collect::<Vec<_>>(),
        );
        w.kv_string("see_also_line", &see_also_line(name));
        w.begin_array_field("flags");
        for arg in sub.get_arguments() {
            // Skip clap's auto-generated --help / --version on each sub.
            if matches!(arg.get_id().as_str(), "help" | "version") {
                continue;
            }
            w.begin_object();
            w.kv_string("name", arg.get_id().as_str());
            w.kv_string(
                "long",
                &arg.get_long().map(|s| s.to_string()).unwrap_or_default(),
            );
            w.kv_string(
                "short",
                &arg.get_short().map(|c| c.to_string()).unwrap_or_default(),
            );
            w.kv_bool(
                "takes_value",
                matches!(
                    arg.get_action(),
                    clap::ArgAction::Set
                        | clap::ArgAction::Append
                        | clap::ArgAction::SetTrue
                            if !matches!(arg.get_action(), clap::ArgAction::SetTrue)
                ) || arg.get_value_names().is_some(),
            );
            w.kv_bool(
                "repeated",
                matches!(
                    arg.get_action(),
                    clap::ArgAction::Append | clap::ArgAction::Count
                ),
            );
            w.kv_bool("required", arg.is_required_set());
            w.kv_bool("positional", arg.is_positional());
            w.kv_string(
                "value_name",
                &arg.get_value_names()
                    .and_then(|v| v.first().map(|s| s.to_string()))
                    .unwrap_or_default(),
            );
            w.kv_string(
                "description",
                &arg.get_help().map(|s| s.to_string()).unwrap_or_default(),
            );
            w.end_object();
        }
        w.end_array();
        w.end_object();
    }
    w.end_object();

    // -- reserved labels / source types / output formats -----------------
    w.field_array_of_strings(
        "reserved_labels",
        &["server", "service", "container", "source", "path"],
    );
    w.field_array_of_strings("source_types", &["logs", "file", "discovery", "metric"]);
    w.field_array_of_strings(
        "output_formats",
        &[
            "human", "json", "ndjson", "csv", "tsv", "md", "yaml", "raw", "format",
        ],
    );

    // -- errors ----------------------------------------------------------
    // HP-5: surface the central error catalog so external tools can
    // map any `error: …` line emitted by the binary back to its help
    // topic. Order is the catalog's declared order — stable input for
    // snapshot consumers.
    w.begin_array_field("errors");
    for e in crate::error::ERROR_CATALOG {
        w.begin_object();
        w.kv_string("code", e.code);
        w.kv_string("summary", e.summary);
        w.kv_string("help_topic", e.help_topic.unwrap_or(""));
        w.end_object();
    }
    w.end_array();

    w.end_object();
    w.finish()
}

/// Render the single-topic envelope (`inspect help <topic> --json`).
pub fn render_topic(topic_id: &str, pretty: bool) -> Option<String> {
    let t = TOPICS.iter().find(|t| t.id == topic_id)?;
    let mut w = JsonWriter::new(pretty);
    w.begin_object();
    w.kv_str("schema_version", &SCHEMA_VERSION.to_string());
    w = w.replace_last_value(&SCHEMA_VERSION.to_string());
    w.kv_string("binary_version", env!("CARGO_PKG_VERSION"));
    w.begin_object_field("topic");
    let body = t.body.unwrap_or("");
    w.kv_string("id", t.id);
    w.kv_string("title", topic_title(body));
    w.kv_string("summary", t.summary);
    w.kv_string("body", body);
    w.field_array_of_strings("examples", &topic_examples(body));
    w.field_array_of_strings("see_also", &topic_see_also(body));
    w.field_array_of_strings("verbs", &crate::help::topics::verbs_for(t.id));
    w.end_object();
    w.end_object();
    Some(w.finish())
}

/// Should the dispatcher pretty-print? Pretty when stdout is a tty.
pub fn pretty_default() -> bool {
    std::io::stdout().is_terminal()
}

// ---------- topic body helpers -------------------------------------------

fn topic_title(body: &str) -> &str {
    body.lines().next().unwrap_or("").trim()
}

fn topic_examples(body: &str) -> Vec<&str> {
    body.lines()
        .filter_map(|l| {
            let t = l.trim_start();
            t.strip_prefix("$ inspect ")
                .map(|_| t.strip_prefix("$ ").unwrap_or(t))
        })
        .collect()
}

fn topic_see_also(body: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut in_block = false;
    for line in body.lines() {
        let t = line.trim();
        if t == "SEE ALSO" {
            in_block = true;
            continue;
        }
        if in_block {
            if t.is_empty() {
                if !out.is_empty() {
                    break;
                }
                continue;
            }
            // Lines look like:  `inspect help <id>   <reason>` or `<id>   <reason>`
            let stripped = t.strip_prefix("inspect help ").unwrap_or(t);
            let id = stripped
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_end_matches(',');
            if !id.is_empty() {
                out.push(id);
            }
        }
    }
    out
}

fn command_examples(sub: &clap::Command) -> Vec<String> {
    let long = sub
        .get_long_about()
        .map(|s| s.to_string())
        .unwrap_or_default();
    long.lines()
        .filter_map(|l| {
            let t = l.trim_start();
            if let Some(rest) = t.strip_prefix("$ inspect ") {
                Some(format!("inspect {}", rest))
            } else {
                None
            }
        })
        .collect()
}

// ---------- error catalog (single source of truth: src/error.rs) ----------
//
// HP-5 moved the catalog to `crate::error::ERROR_CATALOG`. The JSON path
// reads from there directly so there is no second copy to drift. This
// section is intentionally left empty; see `src/error.rs::ERROR_CATALOG`.

// ---------- hand-rolled stable JSON writer -------------------------------

struct JsonWriter {
    buf: String,
    pretty: bool,
    depth: usize,
    /// One per nesting level: number of elements written so far. Used
    /// to insert leading commas without trailing ones.
    counts: Vec<usize>,
}

impl JsonWriter {
    fn new(pretty: bool) -> Self {
        Self {
            buf: String::with_capacity(8 * 1024),
            pretty,
            depth: 0,
            counts: Vec::new(),
        }
    }

    fn finish(mut self) -> String {
        if self.pretty && !self.buf.ends_with('\n') {
            self.buf.push('\n');
        }
        self.buf
    }

    fn indent(&mut self) {
        if self.pretty {
            for _ in 0..self.depth {
                self.buf.push_str("  ");
            }
        }
    }

    fn comma_if_needed(&mut self) {
        if let Some(c) = self.counts.last_mut() {
            if *c > 0 {
                self.buf.push(',');
                if self.pretty {
                    self.buf.push('\n');
                }
            } else if self.pretty {
                self.buf.push('\n');
            }
            *c += 1;
        }
    }

    fn begin_object(&mut self) {
        self.comma_if_needed();
        self.indent();
        self.buf.push('{');
        self.depth += 1;
        self.counts.push(0);
    }

    fn end_object(&mut self) {
        self.depth -= 1;
        let n = self.counts.pop().unwrap_or(0);
        if self.pretty && n > 0 {
            self.buf.push('\n');
            for _ in 0..self.depth {
                self.buf.push_str("  ");
            }
        }
        self.buf.push('}');
    }

    fn begin_object_field(&mut self, key: &str) {
        self.comma_if_needed();
        self.indent();
        self.write_str_lit(key);
        self.buf.push(':');
        if self.pretty {
            self.buf.push(' ');
        }
        self.buf.push('{');
        self.depth += 1;
        self.counts.push(0);
    }

    fn begin_array_field(&mut self, key: &str) {
        self.comma_if_needed();
        self.indent();
        self.write_str_lit(key);
        self.buf.push(':');
        if self.pretty {
            self.buf.push(' ');
        }
        self.buf.push('[');
        self.depth += 1;
        self.counts.push(0);
    }

    fn end_array(&mut self) {
        self.depth -= 1;
        let n = self.counts.pop().unwrap_or(0);
        if self.pretty && n > 0 {
            self.buf.push('\n');
            for _ in 0..self.depth {
                self.buf.push_str("  ");
            }
        }
        self.buf.push(']');
    }

    fn kv_string(&mut self, key: &str, val: &str) {
        self.comma_if_needed();
        self.indent();
        self.write_str_lit(key);
        self.buf.push(':');
        if self.pretty {
            self.buf.push(' ');
        }
        self.write_str_lit(val);
    }

    /// Like `kv_string` but the value is taken as a literal raw token
    /// (used to write numbers/bools without quoting).
    fn kv_str(&mut self, key: &str, raw_val: &str) {
        self.comma_if_needed();
        self.indent();
        self.write_str_lit(key);
        self.buf.push(':');
        if self.pretty {
            self.buf.push(' ');
        }
        self.write_str_lit(raw_val);
    }

    fn kv_bool(&mut self, key: &str, val: bool) {
        self.comma_if_needed();
        self.indent();
        self.write_str_lit(key);
        self.buf.push(':');
        if self.pretty {
            self.buf.push(' ');
        }
        self.buf.push_str(if val { "true" } else { "false" });
    }

    /// Replace the final value (must be a quoted string just written
    /// by `kv_str`) with a raw, unquoted token. Used to convert a
    /// stringified number back to a bare number after the helper
    /// routed it through `write_str_lit`.
    fn replace_last_value(mut self, raw: &str) -> Self {
        // Find the trailing `"<value>"` we just wrote and replace.
        if let Some(quote_open) = self.buf.rfind('"') {
            // skip the closing quote, find opening
            let s = &self.buf[..quote_open];
            if let Some(open) = s.rfind('"') {
                self.buf.truncate(open);
                self.buf.push_str(raw);
            }
        }
        self
    }

    fn field_array_of_strings<S: AsRef<str>>(&mut self, key: &str, items: &[S]) {
        self.begin_array_field(key);
        for s in items {
            self.comma_if_needed();
            self.indent();
            self.write_str_lit(s.as_ref());
        }
        self.end_array();
    }

    fn write_str_lit(&mut self, s: &str) {
        self.buf.push('"');
        for c in s.chars() {
            match c {
                '\\' => self.buf.push_str("\\\\"),
                '"' => self.buf.push_str("\\\""),
                '\n' => self.buf.push_str("\\n"),
                '\r' => self.buf.push_str("\\r"),
                '\t' => self.buf.push_str("\\t"),
                c if (c as u32) < 0x20 => {
                    let _ = write!(self.buf, "\\u{:04x}", c as u32);
                }
                c => self.buf.push(c),
            }
        }
        self.buf.push('"');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> serde_json::Value {
        serde_json::from_str(s).expect("valid JSON")
    }

    #[test]
    fn full_envelope_is_valid_json() {
        let s = render_full(true);
        let _ = parse(&s);
    }

    #[test]
    fn full_envelope_carries_schema_version_and_topics() {
        let v = parse(&render_full(false));
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["topics"].as_array().unwrap().len(), TOPICS.len());
        // Every topic envelope has the required keys.
        for t in v["topics"].as_array().unwrap() {
            for k in ["id", "title", "summary", "examples", "see_also", "verbs"] {
                assert!(t.get(k).is_some(), "topic envelope missing {k}");
            }
        }
    }

    #[test]
    fn full_envelope_lists_every_top_level_command() {
        let v = parse(&render_full(false));
        let cmds = v["commands"].as_object().unwrap();
        for verb in ["grep", "logs", "fleet", "edit", "audit", "search", "alias"] {
            assert!(cmds.contains_key(verb), "commands.{verb} missing");
        }
    }

    #[test]
    fn grep_command_has_flags_and_see_also() {
        let v = parse(&render_full(false));
        let grep = &v["commands"]["grep"];
        assert!(grep["flags"].is_array());
        let sa = grep["see_also_line"].as_str().unwrap();
        assert!(sa.starts_with("See also: inspect help "));
    }

    #[test]
    fn topic_envelope_round_trips() {
        let s = render_topic("quickstart", true).expect("topic exists");
        let v = parse(&s);
        assert_eq!(v["topic"]["id"], "quickstart");
        assert!(v["topic"]["examples"].as_array().unwrap().len() >= 3);
    }

    #[test]
    fn unknown_topic_returns_none() {
        assert!(render_topic("definitely-not-a-topic", false).is_none());
    }

    #[test]
    fn compact_mode_has_no_newlines_inside() {
        let s = render_full(false);
        // Hand-rolled compact mode emits zero newlines.
        assert!(!s.contains('\n'));
    }

    #[test]
    fn pretty_mode_uses_two_space_indent() {
        let s = render_full(true);
        assert!(s.contains("\n  \"schema_version\""));
    }

    #[test]
    fn reserved_lists_are_present() {
        let v = parse(&render_full(false));
        assert!(v["reserved_labels"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("server")));
        assert!(v["source_types"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("logs")));
        assert!(v["output_formats"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("json")));
    }

    #[test]
    fn errors_array_present_with_required_keys() {
        let v = parse(&render_full(false));
        let errs = v["errors"].as_array().unwrap();
        assert!(!errs.is_empty(), "errors catalog must not be empty");
        for e in errs {
            for k in ["code", "summary", "help_topic"] {
                assert!(e.get(k).is_some(), "error envelope missing {k}");
            }
        }
    }
}
