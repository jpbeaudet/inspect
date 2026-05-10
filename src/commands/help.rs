//! `inspect help` command dispatcher.
//!
//! Bible §2: three entry points (index / topic / search), one renderer.
//! All four arms ship: `index` (HP-0), `topic` (HP-0) + `--did-you-mean`
//! suggester (HP-0), `--search <KEYWORD>` (HP-3), and `--json` (HP-4).
//! The HP-* phases are complete; `crate::help::index`, `crate::help::topic`,
//! `crate::help::search`, and `crate::help::all_topics` are the dispatch
//! targets used by `run`.

use anyhow::Result;

use crate::cli::HelpArgs;
use crate::error::ExitKind;
use crate::help;

pub fn run(args: HelpArgs) -> Result<ExitKind> {
    // Mutually exclusive flags — keep the contract honest. clap can
    // express this via `conflicts_with`, but doing it here too gives
    // a stable error message regardless of clap's internal phrasing.
    let mode_flags = [("--search", args.search.is_some()), ("--json", args.json)];
    let active: Vec<&str> = mode_flags
        .iter()
        .filter_map(|(name, on)| if *on { Some(*name) } else { None })
        .collect();
    if active.len() > 1 {
        eprintln!(
            "error: help flags {} are mutually exclusive",
            active.join(" and ")
        );
        return Ok(ExitKind::Error);
    }

    if let Some(needle) = args.search.as_deref() {
        let hits = help::search::query(needle);
        if hits.is_empty() {
            // HP-3 DoD: empty result still prints a single contract
            // line on stderr and exits 1 (NoMatches).
            eprintln!("inspect help: no results for {:?}", needle);
            return Ok(ExitKind::NoMatches);
        }
        return render(&help::search::render(&hits, needle));
    }
    if args.json {
        // HP-4: full surface envelope, or single-topic envelope when a
        // topic is named alongside `--json`.
        // When `--select` is set, route the rendered
        // envelope through the same `print_json_value` chokepoint
        // every other JSON verb uses — so `inspect help all --json
        // --select '.topics[].id'` works for topic discovery without
        // parsing the full registry.
        let pretty = help::json::pretty_default();
        let body = match args.topic.as_deref() {
            None | Some("all") => help::json::render_full(pretty),
            Some(name) => match help::json::render_topic(name, pretty) {
                Some(s) => s,
                None => return unknown_topic(name),
            },
        };
        if let Some(filter) = args.select.as_deref() {
            let value: serde_json::Value = serde_json::from_str(&body)
                .map_err(|e| anyhow::anyhow!("internal: help json reparse: {e}"))?;
            return crate::verbs::output::print_json_value(
                &value,
                Some((filter, args.select_raw, args.select_slurp)),
            );
        }
        return render(&body);
    }

    // `inspect help all` concatenates every topic with a deterministic
    // separator. HP-6 contract: this mode is intended for piping (it's
    // 1.5k+ lines), so we bypass the pager unconditionally — even on
    // an interactive tty — to match `--json`'s pipe-friendly default.
    // With `--verbose`, every topic's optional sidecar is appended.
    if matches!(args.topic.as_deref(), Some("all")) {
        let body = if args.verbose {
            help::all_topics_page_verbose()
        } else {
            help::all_topics_page()
        };
        return render_no_pager(&body);
    }

    match args.topic.as_deref() {
        None => render(&help::index_page()),
        Some(name) => match help::find(name) {
            Some(t) => {
                // HP-6: when --verbose is passed, append the optional
                // `verbose/<id>.md` sidecar. Topics without a sidecar
                // render identically to the non-verbose path.
                let body = if args.verbose {
                    help::topic_page_verbose(t)
                } else {
                    help::topic_page(t)
                };
                render(&body)
            }
            // No editorial topic, but it might be a verb.
            // Fall back to clap's long-help renderer so users can type
            // either `inspect help logs` or `inspect logs --help`.
            None if help::is_verb(name) => render_clap_long_help(name),
            None => unknown_topic(name),
        },
    }
}

/// Render clap's long help for a top-level subcommand.
/// Returns `Success` when the subcommand exists (always, since we
/// gate the call with [`help::is_verb`]); the only failure surface is
/// the writer, which we route through the same handler as topic
/// pages.
fn render_clap_long_help(verb: &str) -> Result<ExitKind> {
    use clap::CommandFactory;
    let mut top = crate::cli::Cli::command();
    let Some(sub) = top.find_subcommand_mut(verb) else {
        // Defensive: VERB_TOPICS got out of sync with the clap tree.
        // Tests guard the invariant, but at runtime we degrade to the
        // unknown-topic path rather than panicking.
        return unknown_topic(verb);
    };
    let body = sub.render_long_help().to_string();
    render(&body)
}

fn render(text: &str) -> Result<ExitKind> {
    if let Err(e) = help::render::write_paged(text) {
        eprintln!("inspect: failed to write help: {e}");
        return Ok(ExitKind::Error);
    }
    Ok(ExitKind::Success)
}

/// `inspect help all` is contracted as a pipe-friendly dump (HP-6
/// DoD). We bypass the pager regardless of stdout's tty status; the
/// output is meant to land in `wc -l`, `grep`, or a file. Other write
/// errors fall through to the same handler as [`render`].
fn render_no_pager(text: &str) -> Result<ExitKind> {
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    match lock.write_all(text.as_bytes()) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => return Ok(ExitKind::Success),
        Err(e) => {
            eprintln!("inspect: failed to write help: {e}");
            return Ok(ExitKind::Error);
        }
    }
    if !text.ends_with('\n') {
        let _ = lock.write_all(b"\n");
    }
    Ok(ExitKind::Success)
}

fn unknown_topic(name: &str) -> Result<ExitKind> {
    let suggestion = help::suggest(name);
    // `Inspect help <unknown>` now exits 2 (Error) with
    // the canonical `error: unknown command or topic: <name>` line
    // and a chained hint pointing at the top-level catalog. earlier
    // wording was "unknown help topic" + exit 1 — that drift was
    // surprising for operators coming from `git`/`cargo`/`kubectl`,
    // all of which exit non-zero on `help <unknown>`. The
    // ERROR_CATALOG fragment was updated in lockstep so the
    // `see: inspect help …` line still attaches automatically.
    crate::error::emit(format!("unknown command or topic: '{}'", name));
    if let Some(s) = suggestion {
        eprintln!("  did you mean: {s}?");
    }
    Ok(ExitKind::Error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(topic: Option<&str>) -> HelpArgs {
        HelpArgs {
            topic: topic.map(String::from),
            search: None,
            json: false,
            verbose: false,
            select: None,
            select_raw: false,
            select_slurp: false,
        }
    }

    #[test]
    fn dispatch_index_succeeds() {
        // We can't easily capture stdout here without an extra plumbing
        // layer; the renderer's own tests cover the writer. This test
        // just confirms dispatch returns Success for the no-arg case.
        let r = run(args(None)).unwrap();
        assert!(matches!(r, ExitKind::Success));
    }

    #[test]
    fn dispatch_known_topic_succeeds() {
        let r = run(args(Some("quickstart"))).unwrap();
        assert!(matches!(r, ExitKind::Success));
    }

    #[test]
    fn dispatch_unknown_topic_returns_nomatches() {
        // Unknown topic / unknown command now exits with
        // ExitKind::Error (code 2). earlier it was NoMatches (code 1)
        // — see CHANGELOG for the full topic list.
        let r = run(args(Some("definitely-not-a-topic"))).unwrap();
        assert!(matches!(r, ExitKind::Error));
    }

    #[test]
    fn dispatch_help_all_succeeds() {
        let r = run(args(Some("all"))).unwrap();
        assert!(matches!(r, ExitKind::Success));
    }
}
