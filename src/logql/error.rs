//! Parse error type with byte-span carat rendering.

use std::fmt;
use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub span: Range<usize>,
    /// Optional hint shown on a separate line.
    pub hint: Option<String>,
}

impl ParseError {
    pub fn new(message: impl Into<String>, span: Range<usize>) -> Self {
        Self {
            message: message.into(),
            span,
            hint: None,
        }
    }
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
    /// Render a diagnostic with a code frame against `source`.
    pub fn render(&self, source: &str) -> String {
        let mut out = String::new();
        out.push_str(&format!("error: {}\n", self.message));
        if let Some((line_no, col, line_str)) = locate(source, self.span.start) {
            let span_end = self.span.end.min(source.len()).max(self.span.start);
            let len = span_end.saturating_sub(self.span.start).max(1);
            // figure out caret length on the line (clamp to line end)
            let caret_len = len.min(line_str.len().saturating_sub(col).max(1));
            let prefix = format!("  {line_no} | ");
            out.push_str(&prefix);
            out.push_str(line_str);
            if !line_str.ends_with('\n') {
                out.push('\n');
            }
            // pad to column under the caret, then ^^^
            let pad = " ".repeat(prefix.len() + col);
            let carets = "^".repeat(caret_len);
            out.push_str(&pad);
            out.push_str(&carets);
            out.push('\n');
        }
        if let Some(h) = &self.hint {
            out.push_str(&format!("hint: {h}\n"));
        }
        out
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} (at {}..{})",
            self.message, self.span.start, self.span.end
        )
    }
}

impl std::error::Error for ParseError {}

/// Find (1-based line, 0-based column on that line, full line text incl newline) for a byte offset.
fn locate(source: &str, offset: usize) -> Option<(usize, usize, &str)> {
    let off = offset.min(source.len());
    let mut line_no = 1usize;
    let mut line_start = 0usize;
    for (i, b) in source.as_bytes().iter().enumerate() {
        if i == off {
            break;
        }
        if *b == b'\n' {
            line_no += 1;
            line_start = i + 1;
        }
    }
    // find end of this line
    let rest = &source[line_start..];
    let line_end_rel = rest.find('\n').map(|p| p + 1).unwrap_or(rest.len());
    let line_str = &source[line_start..line_start + line_end_rel];
    let col = off - line_start;
    Some((line_no, col, line_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_carat_points_at_span() {
        let src = "{server=}";
        let err = ParseError::new("expected string", 8..9);
        let out = err.render(src);
        assert!(out.contains("error: expected string"));
        assert!(out.contains("{server=}"));
        assert!(out.contains("^"));
    }

    #[test]
    fn render_handles_eof() {
        let src = "{server";
        let err = ParseError::new("unexpected end of input", src.len()..src.len());
        let out = err.render(src);
        assert!(out.contains("error: unexpected end"));
    }
}
