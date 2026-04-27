//! `inspect help` system.
//!
//! See `archives/INSPECT_HELP_BIBLE.md` and `archives/INSPECT_HELP_IMPLEMENTATION_PLAN.md`.
//!
//! HP-0 ships:
//! * the topic registry (with one fully authored topic, `quickstart`),
//! * the index page that matches the bible §2.1 contract verbatim,
//! * the dispatcher (index / topic body / "did you mean" suggestion),
//! * the renderer (pager-aware, `NO_COLOR`-aware, width detection).
//!
//! HP-1..HP-6 fill in the remaining topic bodies, the search index,
//! the JSON contract, the verbose sidecars, and the per-verb cross
//! links.

pub mod json;
pub mod render;
pub mod search;
pub mod topics;

pub use topics::{find, is_verb, suggest, verbose_body, Topic, TOPICS};

/// Render the index page (the output of bare `inspect help`).
///
/// Bible §2.1 — must fit on one terminal screen (≤ 40 lines on an
/// 80-col tty). We keep it deterministic and contract-shaped: the
/// command grouping below is hard-coded so the output is
/// snapshot-stable, not dependent on whatever order clap returns
/// subcommands in.
pub fn index_page() -> String {
    let mut s = String::with_capacity(2048);
    s.push_str("INSPECT — cross-server debugging & hot-fix CLI\n\n");
    s.push_str("Usage:  inspect <command> [selector] [flags]\n");
    s.push_str("        inspect help <topic>\n");
    s.push_str("        inspect <command> --help\n\n");

    s.push_str("Topics:\n");
    for t in TOPICS {
        s.push_str(&format!("  {:<14}  {}\n", t.id, t.summary));
    }
    s.push_str("  all             Print all help topics (long)\n\n");

    s.push_str("Commands:\n");
    s.push_str("  Read:   logs grep cat ls find ps status health volumes images network ports\n");
    s.push_str("  Write:  restart stop start reload cp edit rm mkdir touch chmod chown exec\n");
    s.push_str("  Diag:   why recipe connectivity\n");
    s.push_str("  Search: search\n");
    s.push_str("  Fleet:  fleet\n");
    s.push_str("  Setup:  add remove list show test connect disconnect connections setup\n");
    s.push_str("  Alias:  alias\n");
    s.push_str("  Audit:  audit revert\n");
    s.push_str("  Other:  help\n\n");

    s.push_str("Run 'inspect <command> --help' for flag details on any command.\n");
    s.push_str("Run 'inspect help <topic>' for in-depth documentation on any topic.\n");
    s.push_str("Run 'inspect help --search <keyword>' to find help by keyword.\n");
    s
}

/// Render every topic concatenated, in canonical order, with a
/// deterministic separator. Used by `inspect help all` (HP-1) and by
/// the JSON dump (HP-4) when callers want the prose alongside the
/// structured surface. Topics without a body fall back to their stub
/// renderer so the output is always complete.
pub fn all_topics_page() -> String {
    all_topics_page_inner(false)
}

/// Like [`all_topics_page`] but appends each topic's optional verbose
/// sidecar (HP-6). Used by `inspect help all --verbose`. Topics
/// without a sidecar render identically to the non-verbose dump, so
/// the verbose output is a strict superset of the standard one.
pub fn all_topics_page_verbose() -> String {
    all_topics_page_inner(true)
}

fn all_topics_page_inner(verbose: bool) -> String {
    let sep = "\n\n";
    let bar = "=".repeat(72);
    let mut out = String::with_capacity(64 * 1024);
    for (i, t) in TOPICS.iter().enumerate() {
        if i > 0 {
            out.push_str(sep);
            out.push_str(&bar);
            out.push_str(sep);
        }
        if verbose {
            out.push_str(&topic_page_verbose(t));
        } else {
            out.push_str(&topic_page(t));
        }
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Render a single topic. For topics whose bodies have not yet been
/// authored (HP-1 follow-up), produce a deterministic stub that names
/// the topic and points back to the index. This keeps the contract
/// surface honest from HP-0 onward — every id in `TOPICS` resolves
/// rather than 404'ing.
pub fn topic_page(t: &Topic) -> String {
    if let Some(body) = t.body {
        return body.to_string();
    }
    let mut s = String::with_capacity(256);
    s.push_str(&t.id.to_uppercase());
    s.push_str(" — ");
    s.push_str(t.summary);
    s.push_str("\n\n");
    s.push_str("This topic is reserved in the help registry but its body has\n");
    s.push_str("not yet been authored. See archives/INSPECT_HELP_BIBLE.md §3 for the\n");
    s.push_str("intended content; HP-1 ships the prose.\n\n");
    s.push_str("SEE ALSO\n");
    s.push_str("  inspect help              the topic and command index\n");
    s.push_str("  inspect help quickstart   getting started in 60 seconds\n");
    s
}

/// Render a topic body with its optional `verbose/<id>.md` sidecar
/// appended. When no sidecar is registered for the topic, the output
/// is identical to [`topic_page`] — `--verbose` is a safe-by-default
/// flag, never a behaviour change for topics without depth-on-demand
/// content.
///
/// Bible §4.5 — verbose content is *additive*; the standard body
/// must always be the prefix of the verbose body so users who skim
/// the head don't miss anything when they later add `--verbose`.
pub fn topic_page_verbose(t: &Topic) -> String {
    let mut s = topic_page(t);
    if let Some(extra) = verbose_body(t.id) {
        if !s.ends_with('\n') {
            s.push('\n');
        }
        s.push('\n');
        // Stable horizontal rule — pinned by tests so the boundary
        // between the standard body and the sidecar is greppable.
        s.push_str(&"-".repeat(72));
        s.push('\n');
        s.push_str(extra);
        if !s.ends_with('\n') {
            s.push('\n');
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_fits_on_one_screen() {
        let page = index_page();
        let lines = page.lines().count();
        assert!(
            lines <= 40,
            "index page must fit on a single 80x40 screen; got {lines} lines"
        );
    }

    #[test]
    fn index_lists_every_topic() {
        let page = index_page();
        for t in TOPICS {
            assert!(
                page.contains(t.id),
                "index page missing topic id {:?}",
                t.id
            );
        }
    }

    #[test]
    fn index_lists_command_groups() {
        let page = index_page();
        for marker in [
            "Read:", "Write:", "Diag:", "Search:", "Fleet:", "Setup:", "Alias:", "Audit:",
        ] {
            assert!(page.contains(marker), "index missing group {marker:?}");
        }
    }

    #[test]
    fn topic_page_renders_quickstart_body() {
        let t = find("quickstart").unwrap();
        let body = topic_page(t);
        assert!(body.contains("EXAMPLES"));
        assert!(body.contains("inspect connect arte"));
    }

    #[test]
    fn every_topic_has_an_authored_body() {
        // HP-1 contract: every topic in the registry now resolves to
        // a real `.md` file under `src/help/content/`. The stub
        // renderer (kept for forward compatibility) must never fire.
        for t in TOPICS {
            assert!(
                t.body.is_some(),
                "topic {:?} has no authored body (HP-1 should have wired it)",
                t.id
            );
        }
    }

    // -------------------------------------------------------------
    // HP-7 G5 — every embedded `$ inspect …` example must parse
    // cleanly through the live clap definition. This is the single
    // most powerful "topics can't drift from the CLI" guard: rename
    // a flag without updating the docs and the test fails with the
    // exact line that broke.
    //
    // Examples that document still-experimental features may opt
    // out via a trailing `# parse:skip` marker on the same line.
    // We only honour that marker; everything else is mandatory.
    // -------------------------------------------------------------

    /// POSIX-ish argv splitter. Handles single quotes (literal),
    /// double quotes (literal except `\"` and `\\`), and unquoted
    /// whitespace separation. Returns `None` for malformed input
    /// (unclosed quote, dangling escape) so the caller can surface
    /// a clear failure.
    fn shlex_split(line: &str) -> Option<Vec<String>> {
        let mut out: Vec<String> = Vec::new();
        let mut buf = String::new();
        let mut in_single = false;
        let mut in_double = false;
        let mut have_token = false;
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            if in_single {
                if c == '\'' {
                    in_single = false;
                } else {
                    buf.push(c);
                }
                continue;
            }
            if in_double {
                if c == '\\' {
                    match chars.next()? {
                        e @ ('"' | '\\' | '$' | '`' | '\n') => buf.push(e),
                        other => {
                            buf.push('\\');
                            buf.push(other);
                        }
                    }
                } else if c == '"' {
                    in_double = false;
                } else {
                    buf.push(c);
                }
                continue;
            }
            match c {
                '\'' => {
                    in_single = true;
                    have_token = true;
                }
                '"' => {
                    in_double = true;
                    have_token = true;
                }
                '\\' => {
                    let n = chars.next()?;
                    buf.push(n);
                    have_token = true;
                }
                ws if ws.is_whitespace() => {
                    if have_token {
                        out.push(std::mem::take(&mut buf));
                        have_token = false;
                    }
                }
                other => {
                    buf.push(other);
                    have_token = true;
                }
            }
        }
        if in_single || in_double {
            return None;
        }
        if have_token {
            out.push(buf);
        }
        Some(out)
    }

    /// Strip a trailing `# …` shell comment from an example line.
    /// Comments inside quotes are preserved by routing through the
    /// same quote-aware state machine as `shlex_split`.
    fn strip_inline_comment(line: &str) -> &str {
        let mut in_single = false;
        let mut in_double = false;
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i] as char;
            match c {
                '\'' if !in_double => in_single = !in_single,
                '"' if !in_single => in_double = !in_double,
                '#' if !in_single && !in_double => {
                    // Comment must be preceded by whitespace or be at
                    // start; otherwise it's part of an arg (e.g. URL).
                    let preceded_by_ws = i == 0 || (bytes[i - 1] as char).is_whitespace();
                    if preceded_by_ws {
                        return line[..i].trim_end();
                    }
                }
                _ => {}
            }
            i += 1;
        }
        line.trim_end()
    }

    /// Extract every `$ inspect …` example line from a topic body.
    /// Lines ending in `\` (shell continuation) are skipped — they
    /// are intentionally multi-line and would require a full shell
    /// parser to validate. Per `# parse:skip` marker is honoured.
    fn extract_examples(body: &str) -> Vec<String> {
        let mut out = Vec::new();
        for raw in body.lines() {
            let trimmed = raw.trim_start();
            if !trimmed.starts_with("$ inspect ") {
                continue;
            }
            // Skip shell continuations — we don't try to glue lines.
            if raw.trim_end().ends_with('\\') {
                continue;
            }
            // Honour explicit opt-out.
            if raw.contains("# parse:skip") {
                continue;
            }
            // Drop the `$ ` prompt; keep `inspect <verb> …`.
            let cmd = strip_inline_comment(&trimmed["$ ".len()..]);
            out.push(cmd.to_string());
        }
        out
    }

    #[test]
    fn every_topic_example_parses_via_clap() {
        use clap::Parser;
        let mut failures: Vec<String> = Vec::new();
        let mut total = 0usize;
        for t in TOPICS {
            let body = match t.body {
                Some(b) => b,
                None => continue,
            };
            for line in extract_examples(body) {
                total += 1;
                let argv = match shlex_split(&line) {
                    Some(v) => v,
                    None => {
                        failures.push(format!("[{:>10}] shlex split failed: {line}", t.id));
                        continue;
                    }
                };
                if argv.is_empty() || argv[0] != "inspect" {
                    failures.push(format!(
                        "[{:>10}] example does not start with `inspect`: {line}",
                        t.id
                    ));
                    continue;
                }
                if let Err(e) = crate::cli::Cli::try_parse_from(&argv) {
                    // clap's render is verbose; pin the kind so the
                    // failure message stays one line per broken
                    // example.
                    let kind = format!("{:?}", e.kind());
                    failures.push(format!("[{:>10}] {kind}: {line}", t.id));
                }
            }
        }
        assert!(
            total >= 30,
            "expected ≥ 30 inspect examples across all topics; saw {total}"
        );
        assert!(
            failures.is_empty(),
            "{} of {} topic example(s) failed clap parse:\n  {}",
            failures.len(),
            total,
            failures.join("\n  ")
        );
    }

    #[test]
    fn shlex_split_handles_basics() {
        assert_eq!(
            shlex_split("inspect grep \"a b\" arte/atlas").unwrap(),
            vec!["inspect", "grep", "a b", "arte/atlas"]
        );
        assert_eq!(
            shlex_split("inspect search '{foo=\"bar\"} |= \"err\"'").unwrap(),
            vec!["inspect", "search", "{foo=\"bar\"} |= \"err\""]
        );
        assert!(shlex_split("inspect 'unterminated").is_none());
    }

    #[test]
    fn strip_inline_comment_keeps_hash_inside_quotes() {
        assert_eq!(
            strip_inline_comment("inspect grep '#tag' arte # comment"),
            "inspect grep '#tag' arte"
        );
    }
}
