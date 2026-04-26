//! Unified diff renderer used by `edit` and `cp` previews.
//!
//! Wraps the `similar` crate so the rest of the codebase doesn't pin to
//! a particular diff lib.

use similar::{ChangeTag, TextDiff};

/// Produce a `diff -u` style block. `label_old` / `label_new` are emitted
/// in the `--- ` / `+++ ` headers so a reviewer can see exactly which
/// remote target the diff applies to.
pub fn unified_diff(old: &str, new: &str, label_old: &str, label_new: &str) -> String {
    if old == new {
        return String::new();
    }
    let diff = TextDiff::from_lines(old, new);
    let mut out = String::new();
    out.push_str(&format!("--- {label_old}\n"));
    out.push_str(&format!("+++ {label_new}\n"));

    for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
        out.push_str(&hunk.header().to_string());
        out.push('\n');
        for change in hunk.iter_changes() {
            let sign = match change.tag() {
                ChangeTag::Delete => '-',
                ChangeTag::Insert => '+',
                ChangeTag::Equal => ' ',
            };
            // change.value() preserves the trailing newline if present.
            let val = change.value();
            out.push(sign);
            out.push_str(val);
            if !val.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    out
}

/// `(adds, dels, file_count)` summary line for SUMMARY blocks.
pub fn diff_summary(diffs: &[(String, String)]) -> String {
    let mut adds = 0usize;
    let mut dels = 0usize;
    let mut files = 0usize;
    for (old, new) in diffs {
        if old == new {
            continue;
        }
        files += 1;
        let d = TextDiff::from_lines(old, new);
        for change in d.iter_all_changes() {
            match change.tag() {
                ChangeTag::Insert => adds += 1,
                ChangeTag::Delete => dels += 1,
                ChangeTag::Equal => {}
            }
        }
    }
    format!("{files} file(s), +{adds} -{dels}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_renders_change() {
        let d = unified_diff("a\nb\nc\n", "a\nB\nc\n", "x", "x.new");
        assert!(d.contains("--- x"));
        assert!(d.contains("+++ x.new"));
        assert!(d.contains("-b"));
        assert!(d.contains("+B"));
    }

    #[test]
    fn diff_empty_when_equal() {
        let d = unified_diff("same\n", "same\n", "a", "b");
        assert!(d.is_empty());
    }

    #[test]
    fn summary_counts() {
        let s = diff_summary(&[("a\nb\nc\n".into(), "a\nB\nc\n".into())]);
        assert!(s.contains("1 file(s)"));
        assert!(s.contains("+1"));
        assert!(s.contains("-1"));
    }
}
