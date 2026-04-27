//! Helpers for redacting sensitive values in logs and human output.
//!
//! Never print passphrases, key material, or env-var contents. Show the
//! _name_ of the env var holding a secret, never the secret value.

use std::borrow::Cow;

/// Universal redaction placeholder.
pub const REDACTED: &str = "<redacted>";

/// Redact an `Option<String>` for display.
pub fn redact_opt(value: &Option<String>) -> &'static str {
    match value {
        Some(_) => REDACTED,
        None => "<unset>",
    }
}

/// P4 (v0.1.1): line-oriented secret masker for `inspect run` / `inspect exec`
/// stdout. Detects `KEY=VALUE` pairs whose KEY name suggests a secret
/// (suffix matches against the list below) and rewrites the value to keep
/// only the first 4 + last 2 characters; values <8 chars become `****`.
///
/// **Limitation**: only catches `KEY=VALUE` form. Anything inside a JSON
/// blob or free text passes through. Documented in `help/content/write.md`.
pub struct EnvSecretMasker {
    show_secrets: bool,
    redact_all: bool,
    active: std::cell::Cell<bool>,
}

/// Case-insensitive suffix list for keys whose values look like secrets.
const SECRET_KEY_SUFFIXES: &[&str] = &[
    "_KEY",
    "_SECRET",
    "_TOKEN",
    "_PASSWORD",
    "_PASS",
    "_CREDENTIAL",
    "_CREDENTIALS",
    "_APIKEY",
    "_AUTH",
    "_PRIVATE",
    "_ACCESS_KEY",
    "_DSN",
    "_CONNECTION_STRING",
];

/// Whole-name matches (no suffix logic): connection strings without a
/// trailing marker.
const SECRET_KEY_EXACT: &[&str] = &[
    "DATABASE_URL",
    "REDIS_URL",
    "MONGO_URL",
    "POSTGRES_URL",
    "POSTGRESQL_URL",
];

impl EnvSecretMasker {
    pub fn new(show_secrets: bool, redact_all: bool) -> Self {
        Self {
            show_secrets,
            redact_all,
            active: std::cell::Cell::new(false),
        }
    }

    /// True iff we masked at least one line so far. Used by `exec` to
    /// stamp the audit entry.
    pub fn was_active(&self) -> bool {
        self.active.get()
    }

    pub fn mask_line<'a>(&self, line: &'a str) -> Cow<'a, str> {
        if self.show_secrets {
            return Cow::Borrowed(line);
        }
        // Only mask lines that look like a single KEY=VALUE pair (or
        // a `export KEY=VALUE` form). Multi-pair lines are masked
        // pair-by-pair within their boundaries.
        let stripped = line.strip_prefix("export ").unwrap_or(line);
        let eq = match stripped.find('=') {
            Some(i) => i,
            None => return Cow::Borrowed(line),
        };
        let key = stripped[..eq].trim();
        if key.is_empty() || !is_valid_env_key(key) {
            return Cow::Borrowed(line);
        }
        if !self.redact_all && !key_looks_secret(key) {
            return Cow::Borrowed(line);
        }
        let value = &stripped[eq + 1..];
        let masked_value = mask_value(value);
        self.active.set(true);
        // Preserve `export ` prefix when present.
        if line.starts_with("export ") {
            Cow::Owned(format!("export {key}={masked_value}"))
        } else {
            Cow::Owned(format!("{key}={masked_value}"))
        }
    }
}

fn is_valid_env_key(k: &str) -> bool {
    let mut chars = k.chars();
    let first = match chars.next() {
        Some(c) => c,
        None => return false,
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn key_looks_secret(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    if SECRET_KEY_EXACT.iter().any(|k| *k == upper) {
        return true;
    }
    SECRET_KEY_SUFFIXES.iter().any(|s| upper.ends_with(s))
}

/// Mask the secret portion of a value: first 4 + `****` + last 2.
/// Strings shorter than 8 chars become `****`. Surrounding quotes are
/// preserved.
fn mask_value(raw: &str) -> String {
    let trimmed_quotes = (raw.starts_with('"') && raw.ends_with('"') && raw.len() >= 2)
        || (raw.starts_with('\'') && raw.ends_with('\'') && raw.len() >= 2);
    let (q, body) = if trimmed_quotes {
        let q = raw.chars().next().unwrap();
        (Some(q), &raw[1..raw.len() - 1])
    } else {
        (None, raw)
    };
    let masked_body = mask_body(body);
    match q {
        Some(c) => format!("{c}{masked_body}{c}"),
        None => masked_body,
    }
}

fn mask_body(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() < 8 {
        return "****".to_string();
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 2..].iter().collect();
    format!("{head}****{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_secrets_passes_through() {
        let m = EnvSecretMasker::new(true, false);
        assert_eq!(m.mask_line("API_KEY=sk-abcdefgh12").as_ref(), "API_KEY=sk-abcdefgh12");
        assert!(!m.was_active());
    }

    #[test]
    fn masks_anthropic_api_key() {
        let m = EnvSecretMasker::new(false, false);
        // 11 chars: "sk-abcdefgh" + "k3" -> head sk-a + **** + last 2.
        let out = m.mask_line("ANTHROPIC_API_KEY=sk-abcdefghk3");
        assert!(out.contains("sk-a****"));
        assert!(out.contains("k3"));
        assert!(!out.contains("bcdefgh"));
        assert!(m.was_active());
    }

    #[test]
    fn passes_through_non_secret_keys() {
        let m = EnvSecretMasker::new(false, false);
        assert_eq!(m.mask_line("FOO=bar").as_ref(), "FOO=bar");
        assert_eq!(m.mask_line("PATH=/usr/bin:/bin").as_ref(), "PATH=/usr/bin:/bin");
        assert!(!m.was_active());
    }

    #[test]
    fn redact_all_masks_every_kv() {
        let m = EnvSecretMasker::new(false, true);
        let out = m.mask_line("FOO=bar-baz-qux-12");
        assert!(out.contains("****"));
    }

    #[test]
    fn redact_all_only_for_kv_lines() {
        let m = EnvSecretMasker::new(false, true);
        // Plain log lines are still untouched.
        let out = m.mask_line("2025-01-01 hello world");
        assert_eq!(out.as_ref(), "2025-01-01 hello world");
    }

    #[test]
    fn short_secret_becomes_stars() {
        let m = EnvSecretMasker::new(false, false);
        assert_eq!(m.mask_line("API_KEY=hi").as_ref(), "API_KEY=****");
    }

    #[test]
    fn preserves_export_prefix() {
        let m = EnvSecretMasker::new(false, false);
        let out = m.mask_line("export API_TOKEN=abcdefghijkl");
        assert!(out.starts_with("export API_TOKEN="));
        assert!(out.contains("****"));
    }

    #[test]
    fn database_url_is_secret() {
        let m = EnvSecretMasker::new(false, false);
        let out = m.mask_line("DATABASE_URL=postgres://u:pw@host/db");
        assert!(out.contains("****"));
    }

    #[test]
    fn quoted_value_keeps_quotes() {
        let m = EnvSecretMasker::new(false, false);
        let out = m.mask_line("API_KEY=\"sk-abcdefghk3\"");
        assert!(out.contains("\"sk-a****k3\""));
    }
}
