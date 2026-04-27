//! Server-side line filter (P3, v0.1.1).
//!
//! Both `inspect logs` and `inspect grep` accept `--match <regex>` and
//! `--exclude <regex>` (each repeatable). We push the filter down to the
//! remote host as a `grep -E` pipeline suffix so the SSH transport never
//! has to ferry log lines we are about to drop on the client side.
//!
//! Field-pitfall driver: P3 in [INSPECT_v0.1.1_PATCH_SPEC.md]. Operators
//! repeatedly piped `inspect logs ... | grep error | grep -v healthcheck`
//! locally; this folds that idiom into the verb itself.
//!
//! Notes:
//! - In `--follow` mode we use `grep --line-buffered` so the live stream
//!   isn't block-buffered behind the filter.
//! - Multiple `--match` flags OR together (a `(?:p1)|(?:p2)` combination).
//!   Multiple `--exclude` flags also OR together but in the negative
//!   pipeline. (Match-then-exclude order, identical to a hand-rolled
//!   `grep | grep -v` pipeline.)

use crate::verbs::quote::shquote;

/// Combine multiple regex patterns into a single extended regex.
/// Returns `None` when the slice is empty.
pub fn combine(patterns: &[String]) -> Option<String> {
    match patterns.len() {
        0 => None,
        1 => Some(patterns[0].clone()),
        _ => Some(
            patterns
                .iter()
                .map(|p| format!("(?:{p})"))
                .collect::<Vec<_>>()
                .join("|"),
        ),
    }
}

/// Build the shell suffix to append to a base command. Returns an
/// empty string when no filters are set.
///
/// `live` selects `--line-buffered` mode (use it for follow/tail-streaming
/// pipelines so each line round-trips immediately).
pub fn build_suffix(match_res: &[String], exclude_res: &[String], live: bool) -> String {
    let mut s = String::new();
    let lb = if live { " --line-buffered" } else { "" };
    if let Some(m) = combine(match_res) {
        s.push_str(&format!(" | grep{lb} -E -- {}", shquote(&m)));
    }
    if let Some(x) = combine(exclude_res) {
        s.push_str(&format!(" | grep{lb} -vE -- {}", shquote(&x)));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn empty_filters_yield_empty_suffix() {
        assert_eq!(build_suffix(&[], &[], false), "");
    }

    #[test]
    fn single_match_uses_pattern_verbatim() {
        let suf = build_suffix(&s(&["error"]), &[], false);
        assert_eq!(suf, " | grep -E -- 'error'");
    }

    #[test]
    fn multiple_matches_or_together() {
        let suf = build_suffix(&s(&["error", "fatal"]), &[], false);
        assert_eq!(suf, " | grep -E -- '(?:error)|(?:fatal)'");
    }

    #[test]
    fn match_then_exclude_pipes_in_order() {
        let suf = build_suffix(&s(&["error"]), &s(&["healthcheck"]), false);
        assert_eq!(
            suf,
            " | grep -E -- 'error' | grep -vE -- 'healthcheck'"
        );
    }

    #[test]
    fn live_mode_adds_line_buffered() {
        let suf = build_suffix(&s(&["x"]), &s(&["y"]), true);
        assert!(suf.contains("grep --line-buffered -E"));
        assert!(suf.contains("grep --line-buffered -vE"));
    }

    #[test]
    fn shell_metachars_are_quoted() {
        // Single quotes inside the pattern must round-trip safely.
        let suf = build_suffix(&s(&["a'b"]), &[], false);
        assert!(suf.contains("'a'\\''b'"));
    }
}
