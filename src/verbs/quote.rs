//! POSIX-shell single-quoting. Used to embed selector-derived values
//! (paths, patterns) safely inside the remote command string.

/// Wrap `s` in single quotes, escaping any embedded single quotes.
///
/// `o'reilly` → `'o'\''reilly'`
pub fn shquote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain() {
        assert_eq!(shquote("abc"), "'abc'");
    }

    #[test]
    fn embeds_single_quote() {
        assert_eq!(shquote("o'reilly"), "'o'\\''reilly'");
    }

    #[test]
    fn empty() {
        assert_eq!(shquote(""), "''");
    }
}
