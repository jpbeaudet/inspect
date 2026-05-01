//! Multi-line PEM private-key block redactor (L7, v0.1.3).
//!
//! Recognized BEGIN forms (`-----BEGIN ... PRIVATE KEY-----`):
//! - `-----BEGIN PRIVATE KEY-----`            (PKCS#8 unencrypted)
//! - `-----BEGIN ENCRYPTED PRIVATE KEY-----`  (PKCS#8 encrypted)
//! - `-----BEGIN RSA PRIVATE KEY-----`        (PKCS#1)
//! - `-----BEGIN EC PRIVATE KEY-----`         (SEC1)
//! - `-----BEGIN DSA PRIVATE KEY-----`
//! - `-----BEGIN OPENSSH PRIVATE KEY-----`
//! - `-----BEGIN PGP PRIVATE KEY BLOCK-----`
//!
//! And the matching END form. Lines before BEGIN and after END pass
//! through unchanged. Inside a block, every interior line + the END
//! line are suppressed; the BEGIN line is reported as `Marker` so the
//! composer emits a single `[REDACTED PEM KEY]` placeholder.
//!
//! Public certificate blocks (`-----BEGIN CERTIFICATE-----`) and
//! public-key blocks (`-----BEGIN PUBLIC KEY-----`) are deliberately
//! *not* redacted — they're public material by definition and
//! redacting them would obscure useful diagnostic output.

use std::cell::Cell;

use once_cell::sync::Lazy;
use regex::Regex;

/// Decision returned by [`PemMasker::mask_line`].
#[derive(Debug, PartialEq, Eq)]
pub(super) enum PemDecision {
    /// Line is not part of a PEM private-key block. The composer
    /// continues running other maskers on it.
    Pass,
    /// Line is the BEGIN of a PEM private-key block. The composer
    /// emits a single redaction marker in its place.
    Marker,
    /// Line is interior to or the END of an active PEM block. The
    /// composer suppresses it (does not emit at all).
    Suppress,
}

// `([A-Z0-9]+ )*` allows zero or more space-terminated algorithm
// tokens before `PRIVATE KEY`, accommodating every form listed in the
// module doc comment plus the bare PKCS#8 form. The trailing
// `( BLOCK)?` covers PGP's `PRIVATE KEY BLOCK` armor. RFC 7468 fixes
// the dash count at exactly five and uses uppercase for the labels;
// we stay strict on both.
static BEGIN_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\s*-----BEGIN ([A-Z0-9]+ )*PRIVATE KEY( BLOCK)?-----\s*$")
        .expect("redact::pem BEGIN_RE compiles")
});
static END_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\s*-----END ([A-Z0-9]+ )*PRIVATE KEY( BLOCK)?-----\s*$")
        .expect("redact::pem END_RE compiles")
});

pub(super) struct PemMasker {
    in_block: Cell<bool>,
}

impl PemMasker {
    pub(super) fn new() -> Self {
        Self {
            in_block: Cell::new(false),
        }
    }

    pub(super) fn mask_line(&self, line: &str) -> PemDecision {
        if self.in_block.get() {
            // Defensive ordering: check END first so a malformed line
            // that matches both BEGIN and END (pathological) cleanly
            // exits the block instead of nesting.
            if END_RE.is_match(line) {
                self.in_block.set(false);
            }
            return PemDecision::Suppress;
        }
        if BEGIN_RE.is_match(line) {
            // Single-line BEGIN+END (impossible in practice for keys
            // but trivially possible if a tool emits both armor lines
            // back-to-back without a body): we still emit one marker
            // and stay out-of-block.
            if !END_RE.is_match(line) {
                self.in_block.set(true);
            }
            return PemDecision::Marker;
        }
        PemDecision::Pass
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(masker: &PemMasker, lines: &[&str]) -> Vec<PemDecision> {
        lines.iter().map(|l| masker.mask_line(l)).collect()
    }

    #[test]
    fn pkcs8_unencrypted_redacted() {
        let m = PemMasker::new();
        let out = run(
            &m,
            &[
                "before",
                "-----BEGIN PRIVATE KEY-----",
                "MIIBVQIBADAN...",
                "AAAAAAAAAAA",
                "-----END PRIVATE KEY-----",
                "after",
            ],
        );
        assert_eq!(
            out,
            vec![
                PemDecision::Pass,
                PemDecision::Marker,
                PemDecision::Suppress,
                PemDecision::Suppress,
                PemDecision::Suppress,
                PemDecision::Pass,
            ]
        );
    }

    #[test]
    fn pkcs8_encrypted_redacted() {
        let m = PemMasker::new();
        assert_eq!(
            m.mask_line("-----BEGIN ENCRYPTED PRIVATE KEY-----"),
            PemDecision::Marker
        );
        assert_eq!(m.mask_line("body"), PemDecision::Suppress);
        assert_eq!(
            m.mask_line("-----END ENCRYPTED PRIVATE KEY-----"),
            PemDecision::Suppress
        );
    }

    #[test]
    fn rsa_redacted() {
        let m = PemMasker::new();
        assert_eq!(
            m.mask_line("-----BEGIN RSA PRIVATE KEY-----"),
            PemDecision::Marker
        );
        assert_eq!(
            m.mask_line("-----END RSA PRIVATE KEY-----"),
            PemDecision::Suppress
        );
    }

    #[test]
    fn ec_redacted() {
        let m = PemMasker::new();
        assert_eq!(
            m.mask_line("-----BEGIN EC PRIVATE KEY-----"),
            PemDecision::Marker
        );
        assert_eq!(
            m.mask_line("-----END EC PRIVATE KEY-----"),
            PemDecision::Suppress
        );
    }

    #[test]
    fn dsa_redacted() {
        let m = PemMasker::new();
        assert_eq!(
            m.mask_line("-----BEGIN DSA PRIVATE KEY-----"),
            PemDecision::Marker
        );
        assert_eq!(
            m.mask_line("-----END DSA PRIVATE KEY-----"),
            PemDecision::Suppress
        );
    }

    #[test]
    fn openssh_redacted() {
        let m = PemMasker::new();
        assert_eq!(
            m.mask_line("-----BEGIN OPENSSH PRIVATE KEY-----"),
            PemDecision::Marker
        );
        assert_eq!(
            m.mask_line("-----END OPENSSH PRIVATE KEY-----"),
            PemDecision::Suppress
        );
    }

    #[test]
    fn pgp_block_redacted() {
        let m = PemMasker::new();
        assert_eq!(
            m.mask_line("-----BEGIN PGP PRIVATE KEY BLOCK-----"),
            PemDecision::Marker
        );
        assert_eq!(m.mask_line("Version: GnuPG v2"), PemDecision::Suppress);
        assert_eq!(m.mask_line(""), PemDecision::Suppress);
        assert_eq!(m.mask_line("AAAAA="), PemDecision::Suppress);
        assert_eq!(
            m.mask_line("-----END PGP PRIVATE KEY BLOCK-----"),
            PemDecision::Suppress
        );
    }

    #[test]
    fn certificates_pass_through() {
        // Certificates are public; they must not be redacted.
        let m = PemMasker::new();
        assert_eq!(
            m.mask_line("-----BEGIN CERTIFICATE-----"),
            PemDecision::Pass
        );
        assert_eq!(m.mask_line("-----END CERTIFICATE-----"), PemDecision::Pass);
    }

    #[test]
    fn public_keys_pass_through() {
        let m = PemMasker::new();
        assert_eq!(m.mask_line("-----BEGIN PUBLIC KEY-----"), PemDecision::Pass);
        assert_eq!(
            m.mask_line("-----BEGIN RSA PUBLIC KEY-----"),
            PemDecision::Pass
        );
    }

    #[test]
    fn lines_outside_block_pass() {
        let m = PemMasker::new();
        assert_eq!(m.mask_line("hello world"), PemDecision::Pass);
        assert_eq!(m.mask_line("Authorization: Bearer xyz"), PemDecision::Pass);
    }

    #[test]
    fn two_blocks_in_one_stream() {
        let m = PemMasker::new();
        let out = run(
            &m,
            &[
                "-----BEGIN RSA PRIVATE KEY-----",
                "AAAA",
                "-----END RSA PRIVATE KEY-----",
                "between",
                "-----BEGIN OPENSSH PRIVATE KEY-----",
                "BBBB",
                "-----END OPENSSH PRIVATE KEY-----",
            ],
        );
        assert_eq!(
            out,
            vec![
                PemDecision::Marker,
                PemDecision::Suppress,
                PemDecision::Suppress,
                PemDecision::Pass,
                PemDecision::Marker,
                PemDecision::Suppress,
                PemDecision::Suppress,
            ]
        );
    }

    #[test]
    fn leading_trailing_whitespace_tolerated() {
        // Some tools indent or pad PEM armor lines.
        let m = PemMasker::new();
        assert_eq!(
            m.mask_line("   -----BEGIN PRIVATE KEY-----   "),
            PemDecision::Marker
        );
        assert_eq!(
            m.mask_line("-----END PRIVATE KEY-----   "),
            PemDecision::Suppress
        );
    }
}
