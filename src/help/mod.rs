//! `inspect help` system.
//!
//! See `INSPECT_HELP_BIBLE.md` and `INSPECT_HELP_IMPLEMENTATION_PLAN.md`.
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

pub mod render;
pub mod topics;

pub use topics::{find, suggest, Topic, TOPICS};

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
    s.push_str("not yet been authored. See INSPECT_HELP_BIBLE.md §3 for the\n");
    s.push_str("intended content; HP-1 ships the prose.\n\n");
    s.push_str("SEE ALSO\n");
    s.push_str("  inspect help              the topic and command index\n");
    s.push_str("  inspect help quickstart   getting started in 60 seconds\n");
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
        for marker in ["Read:", "Write:", "Diag:", "Search:", "Fleet:", "Setup:", "Alias:", "Audit:"] {
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
    fn topic_page_stubs_unauthored_topics() {
        let t = find("selectors").unwrap();
        let body = topic_page(t);
        assert!(body.starts_with("SELECTORS"));
        assert!(body.contains("not yet been authored"));
    }
}
