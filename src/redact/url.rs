//! URL credential masker (L7, v0.1.3).
//!
//! Replaces the password portion of `scheme://user:pass@host` patterns
//! with `****`, preserving the scheme, username, and host so the
//! diagnostic is still readable. Examples:
//!
//! - `postgres://alice:hunter2@db/app`
//!   → `postgres://alice:****@db/app`
//! - `mongodb+srv://svc:s3cr3t@cluster.mongodb.net/foo`
//!   → `mongodb+srv://svc:****@cluster.mongodb.net/foo`
//!
//! Lines without an embedded credential pattern pass through
//! unchanged (no allocation).

use std::sync::OnceLock;

use regex::Regex;

fn url_cred_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // scheme: alpha first, then alnum / + / - / .
        // user:   any non-:/@/whitespace character
        // pass:   greedy `[^\s]+` so that passwords *containing*
        //         `@` (the L7 audit §5.4 case
        //         `postgres://admin:p@ssw0rd!@host/db`) are masked
        //         in full. Greedy + regex backtracking finds the
        //         rightmost `@` that is followed by a host-shaped
        //         token (alnum / `.` / `-`, optional `:port`); any
        //         earlier `@` inside the password is consumed as
        //         data. The Rust `regex` crate does not support
        //         look-around, so we capture the host explicitly
        //         and rewrite `$1:****@$2` (the URL trailer —
        //         `/path`, `?query`, ` log-suffix`, etc. — is
        //         unmatched and naturally preserved).
        Regex::new(
            r"(?x)
            ([a-zA-Z][a-zA-Z0-9+.\-]*://[^\s:/@]+)        # $1 scheme://user
            :                                              # :
            [^\s]+                                         # password (greedy, no capture)
            @                                              # @
            ([a-zA-Z0-9.\-]+(?::[0-9]+)?)                  # $2 host[:port]
            ",
        )
        .expect("redact::url URL_CRED_RE compiles")
    })
}

pub(super) struct UrlCredMasker;

impl UrlCredMasker {
    pub(super) fn new() -> Self {
        Self
    }

    /// Returns `Some(masked_line)` when at least one credential
    /// pattern was rewritten. `None` for clean lines (no allocation).
    pub(super) fn mask_line(&self, line: &str) -> Option<String> {
        let re = url_cred_re();
        if !re.is_match(line) {
            return None;
        }
        Some(re.replace_all(line, "$1:****@$2").into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postgres_password_masked() {
        let m = UrlCredMasker::new();
        let out = m
            .mask_line("DATABASE=postgres://alice:hunter2@db.internal/app")
            .unwrap();
        assert_eq!(out, "DATABASE=postgres://alice:****@db.internal/app");
    }

    #[test]
    fn mongodb_srv_masked() {
        let m = UrlCredMasker::new();
        let out = m
            .mask_line("connecting to mongodb+srv://svc:s3cr3t@cluster.mongodb.net/foo")
            .unwrap();
        assert_eq!(
            out,
            "connecting to mongodb+srv://svc:****@cluster.mongodb.net/foo"
        );
    }

    #[test]
    fn redis_masked() {
        let m = UrlCredMasker::new();
        let out = m
            .mask_line("REDIS_URL=redis://default:r3d1s@host:6379/0")
            .unwrap();
        assert_eq!(out, "REDIS_URL=redis://default:****@host:6379/0");
    }

    #[test]
    fn ssh_masked() {
        let m = UrlCredMasker::new();
        let out = m
            .mask_line("git+ssh://deploy:tk_abc@git.example.com/repo")
            .unwrap();
        assert_eq!(out, "git+ssh://deploy:****@git.example.com/repo");
    }

    #[test]
    fn url_without_password_unchanged() {
        // `https://user@host` — no `:password` portion.
        let m = UrlCredMasker::new();
        assert!(m.mask_line("https://user@example.com").is_none());
    }

    #[test]
    fn bare_url_unchanged() {
        let m = UrlCredMasker::new();
        assert!(m.mask_line("https://example.com/path").is_none());
        assert!(m.mask_line("see https://docs.example.com").is_none());
    }

    #[test]
    fn mailto_unchanged() {
        // `mailto:user@example.com` is not `://`-shaped.
        let m = UrlCredMasker::new();
        assert!(m.mask_line("contact: mailto:ops@example.com").is_none());
    }

    #[test]
    fn host_port_no_creds_unchanged() {
        // `host:8080/path` looks `:`-then-something but lacks the
        // scheme and the `@`. Must not fire.
        let m = UrlCredMasker::new();
        assert!(m.mask_line("Listening on host:8080/api").is_none());
    }

    #[test]
    fn multiple_urls_in_one_line_all_masked() {
        let m = UrlCredMasker::new();
        let out = m
            .mask_line("primary=postgres://a:p1@h1/db replica=postgres://b:p2@h2/db")
            .unwrap();
        assert_eq!(
            out,
            "primary=postgres://a:****@h1/db replica=postgres://b:****@h2/db"
        );
    }

    #[test]
    fn url_in_log_line_masked() {
        // Realistic log shape — connection string buried in a sentence.
        let m = UrlCredMasker::new();
        let out = m
            .mask_line(
                "2026-05-01T10:00:00Z connecting to amqp://prod_user:abcd1234@rabbit.svc:5672/vhost",
            )
            .unwrap();
        assert_eq!(
            out,
            "2026-05-01T10:00:00Z connecting to amqp://prod_user:****@rabbit.svc:5672/vhost"
        );
    }

    #[test]
    fn username_preserved() {
        // The masker only redacts the password; username stays
        // visible so the operator still has actionable detail
        // ("which account is in trouble?").
        let m = UrlCredMasker::new();
        let out = m
            .mask_line("postgres://distinctive_username:topsecret@db/app")
            .unwrap();
        assert!(out.contains("distinctive_username"));
        assert!(!out.contains("topsecret"));
    }

    #[test]
    fn password_containing_at_fully_masked() {
        // Audit §5.4 — `postgres://admin:p@ssw0rd!@host/db`. The
        // unescaped `@` inside the password used to anchor the old
        // first-`@` regex, leaking `ssw0rd!` into the masked
        // output. The lazy-+-host-lookahead form now captures the
        // entire password regardless of embedded `@`.
        let m = UrlCredMasker::new();
        let out = m
            .mask_line("DATABASE_URL=postgres://admin:p@ssw0rd!@db.internal/app")
            .unwrap();
        assert_eq!(out, "DATABASE_URL=postgres://admin:****@db.internal/app");
        // Defense-in-depth: the masked output must not contain any
        // suffix of the original password.
        assert!(!out.contains("p@"));
        assert!(!out.contains("ssw0rd"));
    }

    #[test]
    fn password_with_multiple_ats_fully_masked() {
        // Pathological case — three `@`s in the password. The
        // lazy-+-host-lookahead must skip past every `@` whose
        // suffix is not host-shaped (`!`, `#`, etc. break the host
        // grammar) and only stop at the real authority boundary.
        let m = UrlCredMasker::new();
        let out = m.mask_line("amqp://svc:a@b@c@host:5672/vhost").unwrap();
        assert_eq!(out, "amqp://svc:****@host:5672/vhost");
    }

    #[test]
    fn password_with_at_then_no_host_unchanged() {
        // If there is no host-shaped token after the last `@`, the
        // line is not a credential URL and must pass through
        // unchanged. (Avoids false positives on `mailto:` after
        // `:` etc.)
        let m = UrlCredMasker::new();
        // `://user:pass@` followed by `!` — `!` is not a hostname
        // char, so no boundary match; line passes through.
        assert!(m.mask_line("postgres://u:p@!notahost").is_none());
    }
}
