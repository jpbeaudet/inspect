//! HP-3 — `inspect help --search <keyword>` runtime.
//!
//! The index is generated at build time by `build.rs` (see plan
//! §HP-3) into `$OUT_DIR/help_index.rs` and `include!`-d here. The
//! generated file exposes three statics:
//!
//! * `TOPIC_IDS:   &[&str]`
//! * `TOPIC_LINES: &[&[&str]]`        — every line of every topic
//! * `KEYWORDS:    &[(&str, &[(u16, u32)])]` — sorted by keyword
//!
//! Multi-keyword queries use AND semantics: only locations matching
//! every needle (via substring) survive. This matches the bible
//! example `inspect help --search "fleet apply"`.

include!(concat!(env!("OUT_DIR"), "/help_index.rs"));

use std::collections::BTreeSet;

/// One match in the help corpus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    /// Topic id (`"selectors"`, `"cmd:grep"`, …).
    pub topic: &'static str,
    /// 1-indexed line number within the topic body.
    pub line: u32,
    /// Up to ~60 chars of context centred on the first matched needle.
    pub snippet: String,
}

/// Run a `--search` query. Returns hits sorted by `(topic, line)`.
///
/// * Tokens are split on whitespace, lower-cased, and deduplicated.
/// * Each token is matched as a *substring* against the keyword
///   column, so `time` matches `timeout` and `timed-out`.
/// * Multi-token queries are AND across locations.
/// * An empty needle returns no hits (the dispatcher treats that as
///   exit-code 1, same as a true miss).
pub fn query(needle: &str) -> Vec<SearchHit> {
    let needles: Vec<String> = needle
        .split_whitespace()
        .map(|w| w.to_ascii_lowercase())
        .filter(|w| !w.is_empty())
        .collect();
    if needles.is_empty() {
        return Vec::new();
    }

    // Per-needle location set, then intersect.
    let mut sets: Vec<BTreeSet<(u16, u32)>> = Vec::with_capacity(needles.len());
    for n in &needles {
        let mut s: BTreeSet<(u16, u32)> = BTreeSet::new();
        for (kw, locs) in KEYWORDS {
            if kw.contains(n.as_str()) {
                for loc in *locs {
                    s.insert(*loc);
                }
            }
        }
        if s.is_empty() {
            // AND with empty set is empty — fast exit.
            return Vec::new();
        }
        sets.push(s);
    }
    sets.sort_by_key(|s| s.len()); // intersect smallest first
    let mut hits = sets.remove(0);
    for s in &sets {
        hits = hits.intersection(s).copied().collect();
        if hits.is_empty() {
            return Vec::new();
        }
    }

    let mut sorted: Vec<(u16, u32)> = hits.into_iter().collect();
    sorted.sort();
    sorted
        .into_iter()
        .map(|(ti, li)| {
            let topic = TOPIC_IDS[ti as usize];
            let line_text = TOPIC_LINES[ti as usize][li as usize];
            let snippet = make_snippet(line_text, &needles[0], 60);
            SearchHit {
                topic,
                line: li + 1,
                snippet,
            }
        })
        .collect()
}

/// Render search results in the canonical human format. Used by the
/// dispatcher; broken out so tests can assert on the exact shape.
pub fn render(hits: &[SearchHit], needle: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{} match{} for \"{}\"\n\n",
        hits.len(),
        if hits.len() == 1 { "" } else { "es" },
        needle
    ));
    let mut current: &str = "";
    for h in hits {
        if h.topic != current {
            if !current.is_empty() {
                out.push('\n');
            }
            current = h.topic;
            out.push_str(&format!("{}\n", h.topic));
        }
        out.push_str(&format!("  L{:<4} {}\n", h.line, h.snippet));
    }
    out
}

/// Approximate byte size of the static keyword index. Used by the
/// HP-3 size guard test. Excludes `TOPIC_LINES` (those are corpus,
/// not "index").
#[cfg(test)]
pub fn index_byte_size() -> usize {
    KEYWORDS
        .iter()
        .map(
            |(k, locs)| k.len() + 16 /* slice header */ + locs.len() * 6, /* (u16,u32) */
        )
        .sum::<usize>()
        + TOPIC_IDS.iter().map(|s| s.len() + 16).sum::<usize>()
}

/// Cut a ~`max`-char window centred on the first occurrence of
/// `needle` in `line`. If `needle` isn't found (because the matched
/// keyword was a longer variant), centre on the line midpoint.
fn make_snippet(line: &str, needle: &str, max: usize) -> String {
    let trimmed = line.trim();
    if trimmed.len() <= max {
        return trimmed.to_string();
    }
    let lower = trimmed.to_ascii_lowercase();
    let centre = lower.find(needle).unwrap_or(trimmed.len() / 2);
    let half = max / 2;
    let start = centre.saturating_sub(half);
    let end = (start + max).min(trimmed.len());
    let start = end.saturating_sub(max);
    // Snap to char boundaries.
    let start = (0..=start)
        .rev()
        .find(|i| trimmed.is_char_boundary(*i))
        .unwrap_or(0);
    let end = (end..=trimmed.len())
        .find(|i| trimmed.is_char_boundary(*i))
        .unwrap_or(trimmed.len());
    let mut s = String::with_capacity(max + 6);
    if start > 0 {
        s.push('…');
    }
    s.push_str(&trimmed[start..end]);
    if end < trimmed.len() {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_are_sorted_unique() {
        for w in KEYWORDS.windows(2) {
            assert!(
                w[0].0 < w[1].0,
                "keywords must be sorted: {:?} vs {:?}",
                w[0].0,
                w[1].0
            );
        }
    }

    #[test]
    fn topic_count_matches_lines_count() {
        assert_eq!(TOPIC_IDS.len(), TOPIC_LINES.len());
    }

    #[test]
    fn timeout_has_at_least_three_hits() {
        // HP-3 DoD: `inspect help --search timeout` produces ≥ 3 hits
        // across `search`, `grep --help` (cmd:grep), and others.
        let hits = query("timeout");
        assert!(
            hits.len() >= 3,
            "expected ≥3 hits for 'timeout', got {} ({:?})",
            hits.len(),
            hits
        );
    }

    #[test]
    fn unknown_needle_returns_empty() {
        assert!(query("xyzzynonexistent").is_empty());
    }

    #[test]
    fn empty_needle_returns_empty() {
        assert!(query("").is_empty());
        assert!(query("   ").is_empty());
    }

    #[test]
    fn and_semantics_intersect() {
        // Both tokens exist; intersection is what matters.
        let a = query("apply");
        let b = query("revert");
        let both = query("apply revert");
        // Intersection cannot exceed either set.
        assert!(both.len() <= a.len());
        assert!(both.len() <= b.len());
        // Every both-hit must be present in each single-token set.
        for h in &both {
            assert!(a.iter().any(|x| x.topic == h.topic && x.line == h.line));
            assert!(b.iter().any(|x| x.topic == h.topic && x.line == h.line));
        }
    }

    #[test]
    fn index_size_under_50kb() {
        // Cap was raised 50 KB → 64 KB in v0.1.2 (bundle + watch topic
        // prose) and 64 KB → 80 KB in v0.1.3 (L7 redaction model
        // documented across write / safety / why help). Still small
        // enough that the index loads instantly even on the smallest
        // dev VMs.
        let n = index_byte_size();
        assert!(n <= 80 * 1024, "index is {n} bytes, exceeds 80 KB cap");
    }

    #[test]
    fn render_groups_by_topic() {
        let hits = query("timeout");
        assert!(!hits.is_empty());
        let out = render(&hits, "timeout");
        assert!(out.contains("match"));
        // The first non-header line must be a topic id, not a body line.
        let body = out.lines().nth(2).unwrap_or_default();
        assert!(
            !body.starts_with("  L"),
            "first content line should be topic header, got {body:?}"
        );
    }

    #[test]
    fn cmd_topics_present() {
        // build.rs must have ingested src/cli.rs LONG_* constants.
        assert!(
            TOPIC_IDS.iter().any(|t| t.starts_with("cmd:")),
            "no cmd:* topics indexed — build.rs LONG_* extraction broken"
        );
    }
}
