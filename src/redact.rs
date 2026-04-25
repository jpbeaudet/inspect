//! Helpers for redacting sensitive values in logs and human output.
//!
//! Never print passphrases, key material, or env-var contents. Show the
//! _name_ of the env var holding a secret, never the secret value.

/// Universal redaction placeholder.
pub const REDACTED: &str = "<redacted>";

/// Redact an `Option<String>` for display.
pub fn redact_opt(value: &Option<String>) -> &'static str {
    match value {
        Some(_) => REDACTED,
        None => "<unset>",
    }
}
