//! Defensive line sanitization for terminal output (field pitfalls
//! §7.1, §7.5).
//!
//! Real-world log streams contain three things that we MUST NOT pass
//! verbatim to a TTY:
//!
//! 1. **ANSI escape sequences** — `\x1b[2J` (clear screen),
//!    `\x1b[H` (cursor home), and the various OSC sequences. A
//!    crafted log line can clear the operator's terminal, change the
//!    title bar, or in some terminal emulators trigger paste-buffer
//!    injection. CVE-2017-7768 / CVE-2008-2383 class.
//!
//! 2. **Other C0 control bytes** (0x00–0x08, 0x0B, 0x0C, 0x0E–0x1F,
//!    0x7F) — bell, NUL, form-feed, etc. These don't *typically*
//!    cause harm but they make output unreadable and can confuse
//!    line-buffered consumers.
//!
//! 3. **Very long lines** — services occasionally emit 100 KB+ JSON
//!    blobs that hang most terminals and balloon log capture. We
//!    truncate display at a configurable byte budget.
//!
//! Whitespace control bytes that have a defined display semantics —
//! `\t` (0x09), `\n` (0x0A), `\r` (0x0D) — are preserved. The caller
//! is responsible for stripping `\n`/`\r` if they want strict
//! single-line display; `print_line` below does that.
//!
//! For machine-readable output (`--json`, audit log) we apply only
//! the C0/ANSI strip, not the truncation, because downstream
//! consumers expect full fidelity.

use std::borrow::Cow;

/// Default per-line display budget in bytes (4 KiB).
pub const DEFAULT_MAX_LINE_BYTES: usize = 4096;

/// Sanitize a line for display on a terminal. Strips ANSI ESC and
/// other dangerous C0 control bytes (replaces them with `?`),
/// optionally truncates to `max_bytes` with a `[truncated, full N
/// bytes]` marker.
///
/// Returns a borrowed slice when the input is already safe and short
/// enough — the common case. Allocations only happen when we
/// actually have to rewrite or truncate.
pub fn safe_terminal_line(s: &str, max_bytes: usize) -> Cow<'_, str> {
    // Fast path: every byte is safe and the line fits.
    let needs_strip = s.bytes().any(is_unsafe_ctl);
    let needs_trunc = s.len() > max_bytes;
    if !needs_strip && !needs_trunc {
        return Cow::Borrowed(s);
    }

    // Build a sanitized copy. Bound the buffer to `max_bytes` (plus
    // truncation marker) so a 100 MiB line cannot OOM us.
    let cap = if needs_trunc { max_bytes + 64 } else { s.len() };
    let mut out = String::with_capacity(cap);
    let mut over_budget = false;
    let original_len = s.len();
    for c in s.chars() {
        if needs_trunc && out.len() + c.len_utf8() > max_bytes {
            over_budget = true;
            break;
        }
        if (c as u32) < 0x20 {
            // Preserve TAB. Newlines are typically already split by
            // `lines()` upstream; if one slips through we replace it.
            if c == '\t' {
                out.push('\t');
            } else {
                out.push('?');
            }
        } else if c == '\u{7F}' {
            out.push('?');
        } else {
            out.push(c);
        }
    }
    if over_budget {
        out.push_str(&format!("... [truncated, full line: {original_len} bytes]"));
    }
    Cow::Owned(out)
}

/// Sanitize for machine-readable output (JSON, audit log). Strips
/// ANSI / C0 controls but preserves the full line — downstream
/// consumers want fidelity, and most JSON parsers handle long
/// strings fine.
pub fn safe_machine_line(s: &str) -> Cow<'_, str> {
    if !s.bytes().any(is_unsafe_ctl) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if ((c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r') || c == '\u{7F}' {
            out.push('?');
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

/// Returns `true` for bytes that are unsafe to pass to a terminal.
/// We strip everything below 0x20 except `\t` (0x09), `\n` (0x0A),
/// `\r` (0x0D). We also strip DEL (0x7F).
#[inline]
fn is_unsafe_ctl(b: u8) -> bool {
    matches!(b, 0..=0x08 | 0x0B | 0x0C | 0x0E..=0x1F | 0x7F)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_line_passes_through_plain_ascii() {
        let s = "hello world";
        match safe_terminal_line(s, 4096) {
            Cow::Borrowed(_) => {}
            Cow::Owned(_) => panic!("should not allocate for plain ASCII"),
        }
    }

    #[test]
    fn safe_line_passes_through_utf8() {
        let s = "日本語 ✓ tab\there";
        let out = safe_terminal_line(s, 4096);
        assert_eq!(out, s);
    }

    #[test]
    fn safe_line_strips_ansi_escape() {
        // \x1b[2J would clear the operator's terminal.
        let nasty = "before\x1b[2J\x1b[Hafter";
        let out = safe_terminal_line(nasty, 4096);
        assert!(!out.contains('\x1b'), "ESC must be stripped: {out:?}");
        assert!(out.starts_with("before") && out.ends_with("after"));
    }

    #[test]
    fn safe_line_strips_bel_and_del() {
        let out = safe_terminal_line("a\x07b\x7Fc", 4096);
        assert_eq!(out, "a?b?c");
    }

    #[test]
    fn safe_line_preserves_tab() {
        assert_eq!(safe_terminal_line("a\tb", 4096), "a\tb");
    }

    #[test]
    fn safe_line_truncates_long_input() {
        let big = "x".repeat(10_000);
        let out = safe_terminal_line(&big, 4096);
        assert!(out.len() <= 4096 + 64);
        assert!(out.contains("truncated"));
        assert!(out.contains("10000 bytes"));
    }

    #[test]
    fn safe_line_truncation_respects_utf8_boundary() {
        // 4-byte chars: the budget cut must not split a codepoint.
        let big = "🦀".repeat(2000);
        let out = safe_terminal_line(&big, 16);
        // Whatever we kept must be valid UTF-8 (test will panic
        // earlier if not).
        assert!(out.starts_with("🦀"));
        assert!(out.contains("truncated"));
    }

    #[test]
    fn safe_machine_line_strips_controls_but_keeps_length() {
        let s = "ok\x1b[31mred\x1b[0mreset";
        let out = safe_machine_line(s);
        assert!(!out.contains('\x1b'));
        assert!(out.contains("red") && out.contains("reset"));
    }

    #[test]
    fn safe_machine_line_keeps_newlines_inside_value() {
        // JSON consumers tolerate \n in strings (it gets encoded).
        let s = "line1\nline2";
        assert_eq!(safe_machine_line(s), s);
    }

    #[test]
    fn safe_machine_line_no_truncation() {
        let big = "x".repeat(100_000);
        let out = safe_machine_line(&big);
        assert_eq!(out.len(), 100_000);
    }
}
