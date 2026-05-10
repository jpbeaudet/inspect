//! Output redaction pipeline.
//!
//! Every line streamed from a remote command on `inspect run`,
//! `inspect exec`, `inspect logs`, `inspect grep`, `inspect cat`,
//! `inspect search`, `inspect why`, `inspect find`, and the merged
//! follow stream is passed through this composer before it reaches
//! local stdout (or a JSON envelope's `line` field). Four maskers run
//! in a fixed order:
//!
//! 1. **PEM** — multi-line gate. A `-----BEGIN ... PRIVATE KEY-----`
//!    line emits one `[REDACTED PEM KEY]` marker; every interior +
//!    `END` line is suppressed. Stateful across lines within a single
//!    redactor instance, stateless across instances.
//! 2. **Header** — line-level regex. `Authorization`, `X-API-Key`,
//!    `Cookie`, `Set-Cookie` (case-insensitive). Replaces the value
//!    portion with `<redacted>`.
//! 3. **URL** — line-level regex. Masks the password in
//!    `scheme://user:pass@host` patterns: `user:****@host`.
//! 4. **Env** — line-level KEY=VALUE masker. Preserved
//!    verbatim; the existing `head4****tail2` partial-mask shape and
//!    suffix list stay unchanged.
//!
//! Inside a PEM block, no other masker fires on the suppressed lines
//! — the entire body is replaced with the single marker. The other
//! three are independent transforms that compose on a single line.
//!
//! ## API contract
//!
//! [`OutputRedactor::mask_line`] returns `Option<Cow<str>>`:
//! - `Some(line)` — emit the (possibly modified) line.
//! - `None` — suppress this line entirely (caller must skip emission).
//!
//! [`OutputRedactor::was_active`] is `true` once any of the four
//! maskers has fired since construction; the audit-args stamp on
//! `inspect run` / `inspect exec` keys off this for the textual
//! `[secrets_masked=true]` tag.
//!
//! [`OutputRedactor::active_kinds`] returns the deterministic ordered
//! list of masker kinds that fired
//! (`["pem", "header", "url", "env"]` — subset, in canonical order).
//! The two write-verb audit paths record this on
//! [`crate::safety::audit::AuditEntry::secrets_masked_kinds`] so
//! post-hoc reviewers can tell `[secrets_masked=true]` apart by which
//! pattern almost leaked.
//!
//! ## Lifetime / state
//!
//! Create one [`OutputRedactor`] per remote step (per ssh dispatch).
//! Stateful PEM tracking must not leak across step boundaries because
//! a step truncated mid-block would otherwise poison the next step's
//! detection. The composer is cheap to construct (regex are compiled
//! once globally via [`std::sync::OnceLock`]).

mod env;
mod header;
mod pem;
mod url;

use std::borrow::Cow;
use std::cell::Cell;

use env::EnvMasker;
use header::HeaderMasker;
use pem::{PemDecision, PemMasker};
use url::UrlCredMasker;

/// Universal redaction placeholder used by structured renderers (e.g.
/// `inspect show`) for fields whose value is a secret. Distinct from
/// the per-masker output strings — those live with their masker.
pub const REDACTED: &str = "<redacted>";

/// Marker emitted on the BEGIN line of every recognized PEM
/// private-key block; interior + END lines are suppressed by the
/// composer.
pub const PEM_REDACTED_MARKER: &str = "[REDACTED PEM KEY]";

// Stable masker kind names. Recorded on
// `AuditEntry::secrets_masked_kinds` when the corresponding masker
// fires. Order matches the canonical chain order
// (PEM → header → URL → env).
pub const KIND_PEM: &str = "pem";
pub const KIND_HEADER: &str = "header";
pub const KIND_URL: &str = "url";
pub const KIND_ENV: &str = "env";

/// Display the redaction status of an `Option<String>` without ever
/// printing its content. Used by `inspect show` and friends to render
/// secret-bearing config fields.
pub fn redact_opt(value: &Option<String>) -> &'static str {
    match value {
        Some(_) => REDACTED,
        None => "<unset>",
    }
}

/// Composed line-by-line redactor. One instance per remote-command
/// invocation; the caller passes every emitted line through
/// [`Self::mask_line`] and emits the result (skipping `None`).
pub struct OutputRedactor {
    show_secrets: bool,
    pem: PemMasker,
    header: HeaderMasker,
    url: UrlCredMasker,
    env: EnvMasker,
    fired_pem: Cell<bool>,
    fired_header: Cell<bool>,
    fired_url: Cell<bool>,
}

impl OutputRedactor {
    /// Construct a new composed redactor.
    ///
    /// * `show_secrets` — when `true`, every masker is bypassed and
    ///   [`Self::mask_line`] returns the input verbatim. Operator
    ///   opt-in via `--show-secrets` on the calling verb.
    /// * `redact_all` — applied only by [`EnvMasker`]: mask every
    ///   well-formed `KEY=VALUE` line regardless of key name. Has no
    ///   effect on the PEM, header, or URL maskers (which already
    ///   redact unconditionally on match).
    pub fn new(show_secrets: bool, redact_all: bool) -> Self {
        Self {
            show_secrets,
            pem: PemMasker::new(),
            header: HeaderMasker::new(),
            url: UrlCredMasker::new(),
            env: EnvMasker::new(redact_all),
            fired_pem: Cell::new(false),
            fired_header: Cell::new(false),
            fired_url: Cell::new(false),
        }
    }

    /// Pass `line` through the four-masker pipeline.
    ///
    /// Returns:
    /// - `None` — the line was inside (or ended) an active PEM
    ///   private-key block; the caller MUST skip emission. The
    ///   `[REDACTED PEM KEY]` marker has already been emitted on the
    ///   block's BEGIN line.
    /// - `Some(line)` — the line is safe to emit (possibly with
    ///   header values, URL passwords, or env-secret values rewritten).
    pub fn mask_line<'a>(&self, line: &'a str) -> Option<Cow<'a, str>> {
        if self.show_secrets {
            return Some(Cow::Borrowed(line));
        }

        // /proc/<pid>/environ and
        // similar interfaces emit NUL-separated `KEY=VALUE` records
        // with no newline terminator. The streaming line reader
        // splits on `\n` only, so the entire blob arrives as a single
        // "line" that the per-line maskers can't decompose. Split on
        // NUL here and mask each chunk independently, rejoining with
        // NUL so the output bytes survive for downstream consumers.
        // PEM is intentionally bypassed for NUL-split chunks — a PEM
        // BEGIN line cannot occur mid-NUL-blob, and running PEM's
        // multi-line state machine across split chunks would corrupt
        // its internal tracking.
        if line.contains('\0') {
            let masked: String = line
                .split('\0')
                .map(|chunk| match self.mask_line_inner(chunk) {
                    Some(Cow::Borrowed(s)) => s.to_string(),
                    Some(Cow::Owned(s)) => s,
                    // `None` (PEM suppress) cannot happen here; the
                    // PEM regex anchors on `^...$` and a chunk
                    // wouldn't carry a full BEGIN/END pair. Fall
                    // through to the marker for safety.
                    None => PEM_REDACTED_MARKER.to_string(),
                })
                .collect::<Vec<_>>()
                .join("\0");
            return Some(Cow::Owned(masked));
        }

        self.mask_line_inner(line)
    }

    fn mask_line_inner<'a>(&self, line: &'a str) -> Option<Cow<'a, str>> {
        // PEM is a multi-line gate. Inside / on END of a block it
        // returns Suppress (and no other masker fires); on the BEGIN
        // line it asks the composer to emit the marker; otherwise it
        // passes through.
        match self.pem.mask_line(line) {
            PemDecision::Marker => {
                self.fired_pem.set(true);
                return Some(Cow::Borrowed(PEM_REDACTED_MARKER));
            }
            PemDecision::Suppress => {
                self.fired_pem.set(true);
                return None;
            }
            PemDecision::Pass => {}
        }

        // Header → URL → Env. Each transforms the line independently
        // and returns either a new owned String (fired) or `None` /
        // Cow::Borrowed (no change).
        let mut current: Cow<'a, str> = Cow::Borrowed(line);
        if let Some(masked) = self.header.mask_line(&current) {
            self.fired_header.set(true);
            current = Cow::Owned(masked);
        }
        if let Some(masked) = self.url.mask_line(&current) {
            self.fired_url.set(true);
            current = Cow::Owned(masked);
        }
        // EnvMasker preserves the input lifetime via `Cow<'a, str>`,
        // so we collapse the two branches without an extra alloc when
        // env didn't fire and current was still Borrowed.
        Some(match current {
            Cow::Borrowed(s) => self.env.mask_line(s),
            Cow::Owned(owned) => match self.env.mask_line(&owned) {
                Cow::Borrowed(_) => Cow::Owned(owned),
                Cow::Owned(rewritten) => Cow::Owned(rewritten),
            },
        })
    }

    /// `true` once any masker has fired during this redactor's
    /// lifetime. Used by the audit-args stamping in `inspect run` /
    /// `inspect exec` to set the `[secrets_masked=true]` text tag.
    pub fn was_active(&self) -> bool {
        self.fired_pem.get()
            || self.fired_header.get()
            || self.fired_url.get()
            || self.env.was_active()
    }

    /// Ordered list of masker kinds that fired during this redactor's
    /// lifetime (canonical chain order: `pem`, `header`, `url`, `env`).
    /// Empty when [`Self::was_active`] is `false`. Recorded on
    /// `AuditEntry::secrets_masked_kinds`.
    pub fn active_kinds(&self) -> Vec<&'static str> {
        let mut out = Vec::with_capacity(4);
        if self.fired_pem.get() {
            out.push(KIND_PEM);
        }
        if self.fired_header.get() {
            out.push(KIND_HEADER);
        }
        if self.fired_url.get() {
            out.push(KIND_URL);
        }
        if self.env.was_active() {
            out.push(KIND_ENV);
        }
        out
    }
}

/// G2 (post-v0.1.3 audit hardening): redact a single command-line
/// string before it is recorded in `AuditEntry::args`.
///
/// Audit reviewers and forensic exports must never see plaintext
/// secrets in the recorded command, even when an operator inadvertently
/// types `psql -p s3cret …` on the CLI. The stream redactor only
/// runs on stdout/stderr; this helper provides the same coverage for
/// the *command text itself*.
///
/// Coverage: header (`Authorization: Bearer …`), URL credentials
/// (`scheme://user:pass@host`), env-var pairs (`KEY=VALUE` where KEY
/// matches the secret-suffix list). PEM is irrelevant for a single
/// command line and is skipped. The function is line-oriented so
/// multi-line script bodies (e.g. `--audit-script-body` or
/// `inspect bundle` step text) are masked line-by-line.
///
/// Returns the input unchanged when nothing fires; otherwise an owned
/// string with the same redaction shape the stream pipeline produces.
pub fn redact_for_audit(text: &str) -> std::borrow::Cow<'_, str> {
    let r = OutputRedactor::new(false, false);
    let mut any = false;
    let mut out = String::new();
    let mut iter = text.split('\n').peekable();
    while let Some(line) = iter.next() {
        match r.mask_line(line) {
            Some(masked) => {
                if matches!(&masked, std::borrow::Cow::Owned(_)) {
                    any = true;
                }
                out.push_str(&masked);
            }
            None => {
                // PEM-suppressed line. Should not occur for single
                // command lines but keep the contract honest.
                any = true;
            }
        }
        if iter.peek().is_some() {
            out.push('\n');
        }
    }
    if any {
        std::borrow::Cow::Owned(out)
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_secrets_passes_everything_through() {
        let r = OutputRedactor::new(true, false);
        assert_eq!(
            r.mask_line("API_KEY=sk-abcdefghk3").unwrap().as_ref(),
            "API_KEY=sk-abcdefghk3"
        );
        assert_eq!(
            r.mask_line("Authorization: Bearer xyz").unwrap().as_ref(),
            "Authorization: Bearer xyz"
        );
        assert_eq!(
            r.mask_line("postgres://u:p@h/d").unwrap().as_ref(),
            "postgres://u:p@h/d"
        );
        // Even a PEM BEGIN line passes through verbatim under
        // --show-secrets, matching the spec's contract.
        assert_eq!(
            r.mask_line("-----BEGIN RSA PRIVATE KEY-----")
                .unwrap()
                .as_ref(),
            "-----BEGIN RSA PRIVATE KEY-----"
        );
        assert!(!r.was_active());
        assert!(r.active_kinds().is_empty());
    }

    #[test]
    fn pem_block_emits_one_marker() {
        let r = OutputRedactor::new(false, false);
        let lines = [
            "before",
            "-----BEGIN RSA PRIVATE KEY-----",
            "MIIBVQIBAD",
            "AAAAAAAAAAA",
            "-----END RSA PRIVATE KEY-----",
            "after",
        ];
        let out: Vec<_> = lines
            .iter()
            .filter_map(|l| r.mask_line(l).map(|c| c.into_owned()))
            .collect();
        assert_eq!(
            out,
            vec![
                "before".to_string(),
                "[REDACTED PEM KEY]".to_string(),
                "after".to_string(),
            ]
        );
        assert!(r.was_active());
        assert_eq!(r.active_kinds(), vec!["pem"]);
    }

    #[test]
    fn header_value_masked() {
        let r = OutputRedactor::new(false, false);
        let out = r
            .mask_line("Authorization: Bearer abc.def.ghi")
            .unwrap()
            .into_owned();
        assert_eq!(out, "Authorization: <redacted>");
        assert_eq!(r.active_kinds(), vec!["header"]);
    }

    #[test]
    fn url_password_masked() {
        let r = OutputRedactor::new(false, false);
        let out = r
            .mask_line("connecting postgres://alice:hunter2@db/app")
            .unwrap()
            .into_owned();
        assert_eq!(out, "connecting postgres://alice:****@db/app");
        assert_eq!(r.active_kinds(), vec!["url"]);
    }

    #[test]
    fn env_masker_still_fires() {
        let r = OutputRedactor::new(false, false);
        let out = r
            .mask_line("API_TOKEN=sk-abcdefghijkl")
            .unwrap()
            .into_owned();
        assert!(out.starts_with("API_TOKEN=sk-a"));
        assert!(out.contains("****"));
        assert_eq!(r.active_kinds(), vec!["env"]);
    }

    #[test]
    fn ordered_kinds_when_multiple_fire_across_lines() {
        let r = OutputRedactor::new(false, false);
        // env first
        let _ = r.mask_line("API_KEY=sk-abcdefghijkl");
        // then header
        let _ = r.mask_line("Authorization: Bearer x");
        // then URL
        let _ = r.mask_line("postgres://u:p@h/d");
        // then PEM
        let _ = r.mask_line("-----BEGIN OPENSSH PRIVATE KEY-----");
        let _ = r.mask_line("body");
        let _ = r.mask_line("-----END OPENSSH PRIVATE KEY-----");
        // Despite the firing order in real time, active_kinds() is
        // canonical: PEM → header → URL → env.
        assert_eq!(r.active_kinds(), vec!["pem", "header", "url", "env"]);
    }

    #[test]
    fn pem_gate_suppresses_other_maskers_on_interior_lines() {
        // A line inside a PEM block that *would otherwise* match the
        // header masker MUST still be suppressed (no double-emit, no
        // leak of header value because the block hasn't ended).
        let r = OutputRedactor::new(false, false);
        assert_eq!(
            r.mask_line("-----BEGIN RSA PRIVATE KEY-----")
                .unwrap()
                .as_ref(),
            "[REDACTED PEM KEY]"
        );
        // Intentionally crafted interior line that contains a
        // header-shaped pattern and a URL credential. Must be
        // suppressed.
        assert!(r
            .mask_line("Authorization: Bearer x postgres://u:p@h/d")
            .is_none());
        // Block ends; subsequent lines pass through normally.
        assert!(r.mask_line("-----END RSA PRIVATE KEY-----").is_none());
        let after = r.mask_line("Authorization: Bearer y").unwrap().into_owned();
        assert_eq!(after, "Authorization: <redacted>");
    }

    #[test]
    fn header_and_url_compose_on_one_line() {
        // A header value that itself contains a URL credential — the
        // header masker fires first and masks the entire value; the
        // URL masker therefore has nothing to do.
        let r = OutputRedactor::new(false, false);
        let out = r
            .mask_line("Cookie: session=postgres://u:p@h/d; theme=dark")
            .unwrap()
            .into_owned();
        assert_eq!(out, "Cookie: <redacted>");
        // Header fired; URL did not (the credential was already
        // inside the masked value).
        assert_eq!(r.active_kinds(), vec!["header"]);
    }

    #[test]
    fn redact_opt_helper() {
        assert_eq!(redact_opt(&Some("anything".to_string())), "<redacted>");
        assert_eq!(redact_opt(&None), "<unset>");
    }

    #[test]
    fn no_match_is_zero_alloc_borrow() {
        // Sanity: a line that matches none of the four maskers should
        // come back as Cow::Borrowed (the same underlying &str).
        let r = OutputRedactor::new(false, false);
        let input = "2026-05-01T10:00:00Z hello world";
        let out = r.mask_line(input).unwrap();
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out.as_ref(), input);
        assert!(!r.was_active());
    }

    // ---- G2: redact_for_audit ------------------------------------

    #[test]
    fn redact_for_audit_passes_clean_text_through() {
        let out = redact_for_audit("ls -la /etc");
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out.as_ref(), "ls -la /etc");
    }

    #[test]
    fn redact_for_audit_masks_env_secret_in_command() {
        let out = redact_for_audit("DATABASE_URL=postgres://admin:s3cret@h/d psql");
        assert!(matches!(out, Cow::Owned(_)));
        // Either the env masker or the URL masker (or both) must hide
        // the password. We only assert the secret is gone — the exact
        // shape is the maskers' contract, tested separately.
        assert!(!out.as_ref().contains("s3cret"));
    }

    #[test]
    fn redact_for_audit_masks_url_password_in_curl() {
        let out = redact_for_audit("curl https://admin:s3cret@example.com/api");
        assert!(!out.as_ref().contains("s3cret"));
    }

    #[test]
    fn redact_for_audit_masks_header_in_curl_command() {
        let out = redact_for_audit(r#"curl -H "Authorization: Bearer eyJabcdef"  https://x"#);
        assert!(!out.as_ref().contains("eyJabcdef"));
    }

    #[test]
    fn redact_for_audit_handles_multi_line_script_body() {
        let s = "echo line1\nAPI_KEY=sk-abcdefghk3\necho line3";
        let out = redact_for_audit(s);
        assert!(matches!(out, Cow::Owned(_)));
        assert!(!out.as_ref().contains("sk-abcdefghk3"));
        assert!(out.as_ref().contains("echo line1"));
        assert!(out.as_ref().contains("echo line3"));
    }

    #[test]
    fn s2_nul_separated_environ_blob_is_masked() {
        // /proc/<pid>/environ shape: NUL-separated KEY=VALUE pairs,
        // no terminating newline. earlier the whole blob arrived as a
        // single line and only the first KEY was inspected, leaking
        // every secret after the first NUL.
        let r = OutputRedactor::new(false, false);
        let blob = "PATH=/usr/bin\0API_KEY=sk-abcdefghk3\0HOME=/root";
        let out = r.mask_line(blob).unwrap();
        // The secret value must be masked …
        assert!(
            !out.as_ref().contains("sk-abcdefghk3"),
            "secret leaked through NUL-separated blob: {out}"
        );
        // … and the NUL byte separators must survive so downstream
        // consumers (transcript, audit, stdout) see the same shape.
        assert_eq!(out.as_ref().matches('\0').count(), 2);
        // Non-secret keys pass through unchanged.
        assert!(out.as_ref().contains("PATH=/usr/bin"));
        assert!(out.as_ref().contains("HOME=/root"));
        assert!(r.was_active());
    }

    #[test]
    fn s2_nul_blob_with_no_secret_passes_clean() {
        let r = OutputRedactor::new(false, false);
        let blob = "PATH=/usr/bin\0HOME=/root\0LANG=C.UTF-8";
        let out = r.mask_line(blob).unwrap();
        assert_eq!(out.as_ref(), blob);
        assert!(!r.was_active());
    }
}
