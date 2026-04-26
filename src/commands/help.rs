//! `inspect help` command dispatcher.
//!
//! Bible §2: three entry points (index / topic / search), one renderer.
//! HP-0 implements index + topic + "did you mean" suggester. The
//! `--search` and `--json` arms are scaffolded as `Unimplemented`
//! placeholders so the CLI surface is stable; HP-3 and HP-4 fill them.

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
        let pretty = help::json::pretty_default();
        let body = match args.topic.as_deref() {
            None | Some("all") => help::json::render_full(pretty),
            Some(name) => match help::json::render_topic(name, pretty) {
                Some(s) => s,
                None => return unknown_topic(name),
            },
        };
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
            None => unknown_topic(name),
        },
    }
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
    // HP-5: emit() already appends `see: inspect help examples` for
    // the "unknown help topic" fragment via the central catalog. We
    // keep the "did you mean" hint but drop the redundant trailing
    // see-line that the HP-0 baseline used.
    crate::error::emit(format!("unknown help topic '{}'", name));
    if let Some(s) = suggestion {
        eprintln!("  did you mean: {s}?");
    }
    // Exit code 1 = "no match" per bible §6 contract for `inspect help`.
    Ok(ExitKind::NoMatches)
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
        let r = run(args(Some("definitely-not-a-topic"))).unwrap();
        assert!(matches!(r, ExitKind::NoMatches));
    }

    #[test]
    fn dispatch_help_all_succeeds() {
        let r = run(args(Some("all"))).unwrap();
        assert!(matches!(r, ExitKind::Success));
    }
}
