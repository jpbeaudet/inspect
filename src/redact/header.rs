//! HTTP-header secret masker (L7, v0.1.3).
//!
//! Catches the four header names listed in the L7 spec —
//! `Authorization`, `X-API-Key`, `Cookie`, `Set-Cookie` — and
//! replaces the value portion with `<redacted>`. Matching is
//! case-insensitive (HTTP header names are case-insensitive per
//! RFC 9110 §5.1).
//!
//! The pattern anchors the header name on either line start or a
//! non-word boundary so we catch the common `curl -v` shapes:
//!
//! ```text
//! > Authorization: Bearer abc123
//! < Set-Cookie: session=xyz
//! ```
//!
//! …without firing on prose like `MyAuthorization` or
//! `subAuthorization` that contains the header name as a substring.
//! The bounded scope keeps false positives below the threshold where
//! they would obscure useful diagnostic output.

use once_cell::sync::Lazy;
use regex::Regex;

static HEADER_RE: Lazy<Regex> = Lazy::new(|| {
    // (?i)        — case-insensitive
    // ($1)        — captured prefix: line start (empty) OR a single non-word char
    // ($2)        — captured header name
    // ($3)        — captured separator: `:` with optional surrounding tabs/spaces
    // .*          — the value portion (greedy to end of line / next match)
    Regex::new(r"(?i)(^|\W)(Authorization|X-API-Key|Cookie|Set-Cookie)([ \t]*:[ \t]*)[^\r\n]*")
        .expect("redact::header HEADER_RE compiles")
});

pub(super) struct HeaderMasker;

impl HeaderMasker {
    pub(super) fn new() -> Self {
        Self
    }

    /// Returns `Some(masked_line)` when the masker fired, otherwise
    /// `None` so the composer can keep the original borrowed `&str`
    /// (no allocation in the no-fire case).
    pub(super) fn mask_line(&self, line: &str) -> Option<String> {
        if !HEADER_RE.is_match(line) {
            return None;
        }
        let masked = HEADER_RE
            .replace_all(line, |caps: &regex::Captures| {
                format!("{}{}{}<redacted>", &caps[1], &caps[2], &caps[3])
            })
            .into_owned();
        Some(masked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorization_bearer_masked() {
        let m = HeaderMasker::new();
        let out = m
            .mask_line("Authorization: Bearer eyJhbGc.eyJzdWI.signature")
            .unwrap();
        assert_eq!(out, "Authorization: <redacted>");
    }

    #[test]
    fn authorization_basic_masked() {
        let m = HeaderMasker::new();
        let out = m.mask_line("Authorization: Basic dXNlcjpwYXNz").unwrap();
        assert_eq!(out, "Authorization: <redacted>");
    }

    #[test]
    fn x_api_key_masked() {
        let m = HeaderMasker::new();
        let out = m.mask_line("X-API-Key: sk_live_abcdefghijkl").unwrap();
        assert_eq!(out, "X-API-Key: <redacted>");
    }

    #[test]
    fn x_api_key_case_insensitive() {
        let m = HeaderMasker::new();
        let out = m.mask_line("x-api-key: sk_live_abcdefghijkl").unwrap();
        assert_eq!(out, "x-api-key: <redacted>");
        let out = m.mask_line("X-Api-Key: sk_live_abcdefghijkl").unwrap();
        assert_eq!(out, "X-Api-Key: <redacted>");
    }

    #[test]
    fn cookie_masked() {
        let m = HeaderMasker::new();
        let out = m.mask_line("Cookie: session=abc123; theme=dark").unwrap();
        assert_eq!(out, "Cookie: <redacted>");
    }

    #[test]
    fn set_cookie_masked() {
        let m = HeaderMasker::new();
        let out = m
            .mask_line("Set-Cookie: session=abc123; HttpOnly; Secure")
            .unwrap();
        assert_eq!(out, "Set-Cookie: <redacted>");
    }

    #[test]
    fn curl_verbose_prefix_handled() {
        // `curl -v` emits headers prefixed with `> ` (request) or `< ` (response).
        let m = HeaderMasker::new();
        let out = m.mask_line("> Authorization: Bearer abc123").unwrap();
        assert_eq!(out, "> Authorization: <redacted>");
        let out = m.mask_line("< Set-Cookie: session=xyz").unwrap();
        assert_eq!(out, "< Set-Cookie: <redacted>");
    }

    #[test]
    fn substring_does_not_fire() {
        // `MyAuthorization` is not the Authorization header.
        let m = HeaderMasker::new();
        assert!(m.mask_line("MyAuthorization: ok").is_none());
        // Same for `Authorization` embedded in a word.
        assert!(m.mask_line("xAuthorizationy: ok").is_none());
    }

    #[test]
    fn unrelated_lines_pass() {
        let m = HeaderMasker::new();
        assert!(m.mask_line("hello world").is_none());
        assert!(m.mask_line("Content-Length: 1234").is_none());
        assert!(m.mask_line("user=alice").is_none());
    }

    #[test]
    fn no_space_after_colon() {
        // Some servers / clients omit the space after the colon.
        let m = HeaderMasker::new();
        let out = m.mask_line("Authorization:Bearer xyz").unwrap();
        assert_eq!(out, "Authorization:<redacted>");
    }

    #[test]
    fn extra_whitespace_tolerated() {
        let m = HeaderMasker::new();
        let out = m.mask_line("Authorization:   Bearer xyz").unwrap();
        assert_eq!(out, "Authorization:   <redacted>");
        let out = m.mask_line("Authorization\t:\tBearer xyz").unwrap();
        assert_eq!(out, "Authorization\t:\t<redacted>");
    }
}
