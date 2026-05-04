//! Shared shell snippet for atomic write+rename that **preserves the
//! original file's mode/uid/gid** (audit §4.2 / P3.13).
//!
//! Without this, `mv tmp path` inherits the temp file's permissions
//! (typically `0644` and the SSH user's uid:gid). For config files
//! that were `0600 root:root`, that silently widens permissions on
//! every edit — a classic production foot-gun.
//!
//! The snippet uses POSIX-portable `stat -c '%a'` / `stat -c '%u:%g'`
//! to read the original file's mode and ownership, then `chmod` /
//! `chown` the temp file before the atomic rename. `stat -c` is
//! supported by both GNU coreutils and BusyBox (Alpine), unlike the
//! pre-v0.1.3 `chmod --reference` form which is GNU-only and printed
//! BusyBox usage spew on every Alpine edit (release-smoke find on
//! arte/inspect-smoke-* against `nginx:alpine`). `chown` frequently
//! requires root; we tolerate its failure (the `2>/dev/null || true`)
//! so unprivileged edits still succeed with the operator's own
//! ownership rather than aborting outright. Mode preservation is
//! required (failure aborts via `set -e`).
//!
//! The whole pre-rename block is wrapped in `if [ -e PATH ]` so this
//! also works for first-time creates (no original to mirror).

use crate::verbs::quote::shquote;

/// Build a `set -e` shell snippet that:
/// 1. base64-decodes `b64` into `tmp`,
/// 2. mirrors `path`'s mode (and best-effort uid:gid) onto `tmp` if
///    `path` already exists, and
/// 3. atomically renames `tmp` over `path`.
///
/// All three arguments are inserted via [`shquote`] so they're safe
/// against arbitrary characters (spaces, quotes, `$`, backticks, …).
pub fn write_then_rename(b64: &str, tmp: &str, path: &str) -> String {
    let b64_q = shquote(b64);
    let tmp_q = shquote(tmp);
    let path_q = shquote(path);
    format!(
        "set -e; \
         printf %s {b64_q} | base64 -d > {tmp_q}; \
         if [ -e {path_q} ]; then \
            chmod \"$(stat -c '%a' {path_q})\" {tmp_q}; \
            chown \"$(stat -c '%u:%g' {path_q})\" {tmp_q} 2>/dev/null || true; \
         fi; \
         mv {tmp_q} {path_q}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_preserves_mode_via_stat() {
        let s = write_then_rename("BASE64==", "/etc/foo.tmp", "/etc/foo");
        assert!(s.contains("chmod \"$(stat -c '%a' "));
        assert!(s.contains("chown \"$(stat -c '%u:%g' "));
        // chown failure tolerated, chmod failure is not.
        assert!(s.contains("|| true"));
        // The pre-v0.1.3 GNU-only form must be gone.
        assert!(!s.contains("--reference="));
    }

    #[test]
    fn snippet_skips_stat_when_path_missing() {
        // The if-guard is what enables first-time creates.
        let s = write_then_rename("X", "/new.tmp", "/new");
        assert!(s.contains("if [ -e "));
    }

    #[test]
    fn snippet_quotes_paths_with_spaces() {
        let s = write_then_rename("X", "/a b.tmp", "/a b");
        // shquote uses single quotes for awkward chars
        assert!(s.contains("'/a b.tmp'"));
        assert!(s.contains("'/a b'"));
    }

    #[test]
    fn snippet_quotes_paths_with_quotes() {
        let s = write_then_rename("X", "/o'k.tmp", "/o'k");
        // No unescaped single-quote next to a literal `/`
        assert!(!s.contains("/o'k.tmp "));
    }
}
