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

/// The canonical topic order — must match `archives/INSPECT_HELP_BIBLE.md` §2.1.
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
        id: "bundle",
        summary: "YAML-driven multi-step orchestration with rollback (B9)",
        body: Some(include_str!("content/bundle.md")),
    },
    Topic {
        id: "watch",
        summary: "Block until a condition holds on a single target (B10)",
        body: Some(include_str!("content/watch.md")),
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
    Topic {
        id: "compose",
        summary: "First-class compose verbs (ls/ps/config/logs/restart) — F6 (v0.1.3)",
        body: Some(include_str!("content/compose.md")),
    },
    Topic {
        id: "select",
        summary: "Filter / project JSON output with `--select` (jq language) — F19 (v0.1.3)",
        body: Some(include_str!("content/select.md")),
    },
];

/// Look up a topic by its canonical id. Comparison is case-insensitive
/// because users frequently type `Inspect help SEARCH`.
pub fn find(id: &str) -> Option<&'static Topic> {
    let needle = id.trim().to_ascii_lowercase();
    TOPICS.iter().find(|t| t.id == needle)
}

/// Returns true if `name` matches a top-level verb listed in
/// [`VERB_TOPICS`]. Used by `inspect help <verb>` to fall back
/// to clap's long-help renderer when there is no editorial topic of
/// the same id.
pub fn is_verb(name: &str) -> bool {
    let needle = name.trim().to_ascii_lowercase();
    VERB_TOPICS.iter().any(|(v, _)| *v == needle)
}

/// Optional `verbose/<topic>.md` sidecar (HP-6). Returns the sidecar
/// body when one ships for the given topic, else `None`. Sidecars are
/// appended after the standard topic body when the user passes
/// `--verbose`.
///
/// The mapping is hand-maintained (rather than glob-discovered) so
/// the binary's surface stays stable: adding a sidecar is a
/// deliberate edit here, mirrored by a new file under
/// `src/help/verbose/`.
pub fn verbose_body(id: &str) -> Option<&'static str> {
    let needle = id.trim().to_ascii_lowercase();
    Some(match needle.as_str() {
        "ssh" => include_str!("verbose/ssh.md"),
        "search" => include_str!("verbose/search.md"),
        "write" => include_str!("verbose/write.md"),
        "safety" => include_str!("verbose/safety.md"),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// HP-2: verb <-> topic registry.
//
// The mapping is the source of truth for two consumers:
//   * `cli.rs`, which renders `See also: inspect help <topic>, ...` in the
//     `after_help` block of every command's `--help` output,
//   * `inspect help --json` (HP-4), which exposes `commands.<verb>.see_also`
//     and `topics.<id>.verbs` so external agents can navigate the surface
//     deterministically.
//
// Order in each `&[topic]` slice matters: the first entry is the primary
// topic. The bible §HP-2 table is the source for these groupings — any
// change here must round-trip through that table.
// ---------------------------------------------------------------------------

/// Static mapping `verb -> &[topic_ids]`, ordered (primary first).
///
/// Verbs not present here have no editorial topic linkage and the renderer
/// must fall through to a generic footer. Today every top-level verb is
/// listed; the test suite guards that invariant.
pub const VERB_TOPICS: &[(&str, &[&str])] = &[
    // Read verbs — the selector + format + worked-examples cluster.
    ("status", &["selectors", "formats", "examples"]),
    ("health", &["selectors", "formats", "examples"]),
    ("logs", &["selectors", "formats", "examples"]),
    ("grep", &["selectors", "formats", "examples"]),
    ("cat", &["selectors", "formats", "examples"]),
    ("ls", &["selectors", "formats", "examples"]),
    ("find", &["selectors", "formats", "examples"]),
    ("ps", &["selectors", "formats", "examples"]),
    ("volumes", &["selectors", "formats", "examples"]),
    ("images", &["selectors", "formats", "examples"]),
    ("network", &["selectors", "formats", "examples"]),
    ("ports", &["selectors", "formats", "examples"]),
    ("run", &["selectors", "formats", "examples"]),
    ("watch", &["selectors", "formats", "examples"]),
    ("resolve", &["selectors", "aliases", "examples"]),
    // Search.
    ("search", &["search", "selectors", "aliases", "formats"]),
    // Write verbs — every one carries the safety + fleet cross-link.
    ("restart", &["write", "safety", "fleet"]),
    ("stop", &["write", "safety", "fleet"]),
    ("start", &["write", "safety", "fleet"]),
    ("reload", &["write", "safety", "fleet"]),
    ("cp", &["write", "safety", "fleet"]),
    ("edit", &["write", "safety", "fleet"]),
    ("rm", &["write", "safety", "fleet"]),
    ("mkdir", &["write", "safety", "fleet"]),
    ("touch", &["write", "safety", "fleet"]),
    ("chmod", &["write", "safety", "fleet"]),
    ("chown", &["write", "safety", "fleet"]),
    ("exec", &["write", "safety", "fleet"]),
    // Audit + revert — the safety pair.
    ("audit", &["safety", "write"]),
    ("revert", &["safety", "write"]),
    // Fleet orchestrator.
    ("fleet", &["fleet", "write", "selectors"]),
    // B9 — bundle orchestration.
    ("bundle", &["write", "safety", "fleet"]),
    // Diagnostic recipes.
    ("why", &["recipes", "examples"]),
    ("connectivity", &["recipes", "examples"]),
    ("recipe", &["recipes", "examples"]),
    // Setup / discovery / ssh lifecycle.
    ("add", &["discovery", "ssh", "quickstart"]),
    ("list", &["discovery", "ssh", "quickstart"]),
    ("remove", &["discovery", "ssh", "quickstart"]),
    ("show", &["discovery", "ssh", "quickstart"]),
    ("test", &["discovery", "ssh", "quickstart"]),
    ("setup", &["discovery", "ssh", "quickstart"]),
    ("discover", &["discovery", "ssh", "quickstart"]),
    ("profile", &["discovery", "ssh", "quickstart"]),
    ("connect", &["ssh", "discovery", "quickstart"]),
    ("disconnect", &["ssh", "discovery"]),
    ("connections", &["ssh", "discovery"]),
    ("disconnect-all", &["ssh", "discovery"]),
    // The new `inspect ssh ...` family. Cross-links into
    // the ssh editorial topic (which gained the password-auth +
    // add-key sections) and safety (audit-log shape).
    ("ssh", &["ssh", "safety"]),
    // The `inspect keychain ...` family. Cross-links
    // into the ssh editorial topic (which gained the credential-
    // lifetime section) and safety (audit-log shape for the
    // remove sub-verb).
    ("keychain", &["ssh", "safety"]),
    // Aliases.
    ("alias", &["aliases", "selectors", "search"]),
    // The compose verb cluster cross-links into the
    // compose editorial topic plus safety/formats. Listed once at
    // the top level — sub-verbs (`compose ls`, `compose ps`, …) are
    // discoverable via `inspect compose --help` and don't need
    // individual rows here.
    ("compose", &["compose", "safety", "formats"]),
    // Help is a verb too: it cross-links the user back to the index.
    ("help", &["quickstart", "examples"]),
];

/// Look up the topics linked to a verb. Returns the empty slice if the
/// verb is unknown, which the JSON serialiser treats as "no editorial
/// linkage" rather than panicking — keeps the contract resilient when a
/// new verb is added but its row is forgotten (the test suite catches
/// the omission).
pub fn topics_for_verb(verb: &str) -> &'static [&'static str] {
    VERB_TOPICS
        .iter()
        .find(|(v, _)| *v == verb)
        .map(|(_, ts)| *ts)
        .unwrap_or(&[])
}

/// Inverse view: every verb that lists `topic` (in any position).
/// Produced on demand because the registry is small (≤ 50 verbs) and
/// this is only called from `--json` and tests.
pub fn verbs_for(topic: &str) -> Vec<&'static str> {
    VERB_TOPICS
        .iter()
        .filter(|(_, ts)| ts.contains(&topic))
        .map(|(v, _)| *v)
        .collect()
}

/// Render the canonical `See also:` footer line for a verb. Used by the
/// `after_help` blocks in [`crate::cli`] and by the JSON contract.
///
/// Format (pinned by the test suite):
///   `See also: inspect help <t1>, inspect help <t2>, inspect help <t3>`
///
/// The bible §HP-2 DoD names this exact shape for `inspect grep --help`.
pub fn see_also_line(verb: &str) -> String {
    let topics = topics_for_verb(verb);
    if topics.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = topics.iter().map(|t| format!("inspect help {t}")).collect();
    format!("See also: {}", parts.join(", "))
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
            curr[j + 1] = (curr[j] + 1).min(prev[j + 1] + 1).min(prev[j] + cost);
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
    // Also consider verbs, so `inspect help serch` suggests
    // `search` even though no topic by that id exists in the
    // editorial registry.
    for (verb, _) in VERB_TOPICS {
        let d = edit_distance(&needle, verb, MAX_DISTANCE);
        if d > MAX_DISTANCE {
            continue;
        }
        match best {
            None => best = Some((*verb, d)),
            Some((_, bd)) if d < bd => best = Some((*verb, d)),
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
        // 14 HP-1 editorial topics + 1 compose topic
        // + 1 select topic.
        assert_eq!(TOPICS.len(), 16);
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
