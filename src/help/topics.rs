//! Topic registry for the help system.
//!
//! HP-0 shipped the registry shape with a single authored body
//! (`quickstart`); HP-1 fills in the remaining eleven topic bodies.
//! Every entry in [`TOPICS`] now resolves to a real `.md` file under
//! `src/help/content/`.

/// One help topic. The `body` is rendered verbatim by [`crate::help::render`].
#[derive(Debug, Clone, Copy)]
pub struct Topic {
    /// Stable, lowercase, URL-safe identifier (e.g. `"selectors"`).
    /// This is what the user types after `inspect help`.
    pub id: &'static str,
    /// One-line description used by the index page.
    pub summary: &'static str,
    /// Full body (markdown source, see [`crate::help::render`]).
    /// `None` for topics that have not yet been authored — the
    /// dispatcher renders a deterministic stub so HP-0 already
    /// exposes the full contract surface.
    pub body: Option<&'static str>,
}

/// The canonical topic order — must match [`INSPECT_HELP_BIBLE.md`] §2.1.
/// The index page renders topics in this exact order.
pub const TOPICS: &[Topic] = &[
    Topic {
        id: "quickstart",
        summary: "Set up your first server in 60 seconds",
        body: Some(include_str!("content/quickstart.md")),
    },
    Topic {
        id: "selectors",
        summary: "How to address servers, services, and files",
        body: Some(include_str!("content/selectors.md")),
    },
    Topic {
        id: "aliases",
        summary: "Save and reuse selectors with @name",
        body: Some(include_str!("content/aliases.md")),
    },
    Topic {
        id: "search",
        summary: "LogQL query syntax for cross-medium search",
        body: Some(include_str!("content/search.md")),
    },
    Topic {
        id: "formats",
        summary: "Output format options (--json, --csv, --md, --format, ...)",
        body: Some(include_str!("content/formats.md")),
    },
    Topic {
        id: "write",
        summary: "Write verbs, dry-run/apply, safety contract",
        body: Some(include_str!("content/write.md")),
    },
    Topic {
        id: "safety",
        summary: "Audit log, snapshots, revert",
        body: Some(include_str!("content/safety.md")),
    },
    Topic {
        id: "fleet",
        summary: "Multi-server operations",
        body: Some(include_str!("content/fleet.md")),
    },
    Topic {
        id: "recipes",
        summary: "Multi-step diagnostic and remediation runbooks",
        body: Some(include_str!("content/recipes.md")),
    },
    Topic {
        id: "discovery",
        summary: "Auto-discovery, profiles, drift detection",
        body: Some(include_str!("content/discovery.md")),
    },
    Topic {
        id: "ssh",
        summary: "Persistent connections, ControlMaster, passphrases",
        body: Some(include_str!("content/ssh.md")),
    },
    Topic {
        id: "examples",
        summary: "Worked examples and translation guide (grep -> inspect, etc.)",
        body: Some(include_str!("content/examples.md")),
    },
];

/// Look up a topic by its canonical id. Comparison is case-insensitive
/// because users frequently type `Inspect help SEARCH`.
pub fn find(id: &str) -> Option<&'static Topic> {
    let needle = id.trim().to_ascii_lowercase();
    TOPICS.iter().find(|t| t.id == needle)
}

/// All known topic ids in canonical order. Used by the index renderer
/// and by `inspect help all` (HP-6).
#[allow(dead_code)] // consumed by `inspect help all` in HP-6
pub fn all_ids() -> impl Iterator<Item = &'static str> {
    TOPICS.iter().map(|t| t.id)
}

/// Levenshtein distance between two ASCII-lowercase strings, capped at
/// `max + 1` for early exit. Used by the "did you mean" suggester so a
/// long unknown topic doesn't turn into a quadratic comparison against
/// every registered name.
pub(crate) fn edit_distance(a: &str, b: &str, max: usize) -> usize {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len().abs_diff(b.len()) > max {
        return max + 1;
    }
    // Two-row DP, O(min(a, b)) memory. Sufficient for our < 20-char topic ids.
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr: Vec<usize> = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        let mut row_min = curr[0];
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca.eq_ignore_ascii_case(cb) { 0 } else { 1 };
            curr[j + 1] = (curr[j] + 1)
                .min(prev[j + 1] + 1)
                .min(prev[j] + cost);
            row_min = row_min.min(curr[j + 1]);
        }
        if row_min > max {
            return max + 1;
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// Suggest the closest topic id to `needle` within `max` edits.
/// Returns `None` if every topic is farther than `max`.
pub fn suggest(needle: &str) -> Option<&'static str> {
    // Cap at 2 edits: tight enough that short ids like "ssh" or
    // "fleet" don't match unrelated 3-letter typos, loose enough to
    // catch the common single-character slips ("quickstrt",
    // "selecter").
    const MAX_DISTANCE: usize = 2;
    let needle = needle.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return None;
    }
    let mut best: Option<(&'static str, usize)> = None;
    for t in TOPICS {
        let d = edit_distance(&needle, t.id, MAX_DISTANCE);
        if d > MAX_DISTANCE {
            continue;
        }
        match best {
            None => best = Some((t.id, d)),
            Some((_, bd)) if d < bd => best = Some((t.id, d)),
            _ => {}
        }
    }
    best.map(|(id, _)| id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quickstart_body_is_present() {
        let t = find("quickstart").expect("quickstart topic registered");
        let body = t.body.expect("quickstart body shipped in HP-0");
        assert!(body.starts_with("QUICKSTART"));
        assert!(body.contains("EXAMPLES"));
        assert!(body.contains("SEE ALSO"));
    }

    #[test]
    fn topic_count_matches_bible() {
        assert_eq!(TOPICS.len(), 12);
    }

    #[test]
    fn find_is_case_insensitive() {
        assert!(find("QuickStart").is_some());
        assert!(find("  quickstart  ").is_some());
        assert!(find("nope").is_none());
    }

    #[test]
    fn suggest_finds_close_topic() {
        assert_eq!(suggest("quickstrt"), Some("quickstart"));
        assert_eq!(suggest("selecter"), Some("selectors"));
        assert_eq!(suggest("xyz"), None); // distance > 2 from every id
    }

    #[test]
    fn edit_distance_cap_short_circuits() {
        // exact match
        assert_eq!(edit_distance("abc", "abc", 3), 0);
        // beyond cap: returns max + 1 sentinel
        assert!(edit_distance("aaaaaaaa", "bbbbbbbb", 2) > 2);
    }
}
