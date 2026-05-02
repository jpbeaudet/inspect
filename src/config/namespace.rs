//! Namespace data model.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::ConfigError;

/// A configured namespace as it appears on disk and after resolution.
///
/// All fields are independently overridable from environment variables (see
/// [`crate::config::env`]). A merged value is represented by
/// [`ResolvedNamespace`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct NamespaceConfig {
    pub host: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub key_path: Option<String>,
    /// Name of the environment variable holding the key passphrase.
    /// Never store passphrase values themselves on disk.
    pub key_passphrase_env: Option<String>,
    /// Base64-encoded inline private key (env-only, never on disk; this field
    /// is parsed only from environment variables and serialized back out for
    /// `show --json` purposes — never written to `servers.toml`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_inline: Option<String>,
    /// F12 (v0.1.3): per-namespace remote environment overlay. Applied
    /// transparently to every `inspect run` / `inspect exec` invocation
    /// against this namespace as `env KEY1="VAL1" KEY2="VAL2" -- <cmd>`.
    /// Values are passed through the remote shell with double-quoting,
    /// so `$HOME`/`$PATH` references on the right-hand side expand on
    /// the remote (intentional: the operator wants the remote user's
    /// home, not the local one). Shell metacharacters (`;`, `&`, `|`,
    /// backticks) are preserved as literal text inside the quoted value.
    /// `None` (the default) means no overlay is applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    /// F13 (v0.1.3): per-namespace opt-out for stale-session
    /// auto-reauth. Default (when `None`) is `true` — every dispatch
    /// that hits a transport-stale failure transparently re-auths
    /// and retries once. Operators who want every session expiry to
    /// surface as a hard failure (e.g. CI runners) set
    /// `auto_reauth = false` in `~/.inspect/servers.toml`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_reauth: Option<bool>,
    /// F18 (v0.1.3): per-namespace transcript override. When the
    /// `[namespaces.<ns>.history]` table is present, its fields
    /// override the global defaults: `disabled = true` skips
    /// transcript writes for this namespace entirely (audit log is
    /// still written), `redact = "off"|"normal"|"strict"` overrides
    /// the L7 redaction mode applied to lines tee'd into the
    /// transcript file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<HistoryNsOverride>,
}

/// F18 (v0.1.3): per-namespace transcript policy override. See
/// [`NamespaceConfig::history`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryNsOverride {
    /// When `Some(true)`, transcript writes for this namespace are
    /// skipped (the audit log is still written). `None` ⇒ transcript
    /// is enabled (the default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
    /// Per-namespace transcript redaction mode: `"normal"` (default
    /// — L7 four-masker pipeline), `"strict"` (reserved; identical
    /// to `"normal"` in v0.1.3), `"off"` (write raw lines without
    /// masking). `None` ⇒ inherit the global L7 default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redact: Option<String>,
}

impl NamespaceConfig {
    /// Merge `other` over `self`: any field set in `other` takes precedence.
    pub fn merge_over(&self, other: &NamespaceConfig) -> NamespaceConfig {
        // F12 (v0.1.3): env overlay merge semantics. When both file
        // and env override carry a map, `other`'s keys win on
        // collision but file-only keys survive. This matches the
        // resolver's general "env over file" idiom (env is a
        // partial overlay, not a replacement).
        let env = match (self.env.as_ref(), other.env.as_ref()) {
            (None, None) => None,
            (Some(a), None) => Some(a.clone()),
            (None, Some(b)) => Some(b.clone()),
            (Some(a), Some(b)) => {
                let mut merged = a.clone();
                for (k, v) in b {
                    merged.insert(k.clone(), v.clone());
                }
                Some(merged)
            }
        };
        NamespaceConfig {
            host: other.host.clone().or_else(|| self.host.clone()),
            user: other.user.clone().or_else(|| self.user.clone()),
            port: other.port.or(self.port),
            key_path: other.key_path.clone().or_else(|| self.key_path.clone()),
            key_passphrase_env: other
                .key_passphrase_env
                .clone()
                .or_else(|| self.key_passphrase_env.clone()),
            key_inline: other.key_inline.clone().or_else(|| self.key_inline.clone()),
            env,
            auto_reauth: other.auto_reauth.or(self.auto_reauth),
            history: other.history.clone().or_else(|| self.history.clone()),
        }
    }

    /// Validate that required fields are populated and that mutually
    /// exclusive options aren't both set.
    pub fn validate(&self, namespace: &str) -> Result<(), ConfigError> {
        if self.host.is_none() {
            return Err(ConfigError::MissingField {
                namespace: namespace.to_string(),
                field: "host",
            });
        }
        if self.user.is_none() {
            return Err(ConfigError::MissingField {
                namespace: namespace.to_string(),
                field: "user",
            });
        }
        if self.key_path.is_some() && self.key_inline.is_some() {
            return Err(ConfigError::ConflictingKeySources);
        }
        // F12 (v0.1.3): every env-overlay key must be a POSIX-portable
        // identifier ([A-Za-z_][A-Za-z0-9_]*). Reject anything else
        // here so a typo'd config does not silently produce a remote
        // command line that the shell parses as something else
        // (`KEY-NAME=val` would split on `-` in some shells, or be
        // taken as a flag to `env`).
        if let Some(map) = self.env.as_ref() {
            for k in map.keys() {
                if !is_valid_env_key(k) {
                    return Err(ConfigError::InvalidEnvKey {
                        namespace: namespace.to_string(),
                        key: k.clone(),
                    });
                }
            }
        }
        Ok(())
    }
}

/// F12 (v0.1.3): `[A-Za-z_][A-Za-z0-9_]*`, max 256 chars. Mirrors
/// POSIX 3.231 (Name) which `sh`, `env`, and `printenv` all enforce.
pub fn is_valid_env_key(s: &str) -> bool {
    if s.is_empty() || s.len() > 256 {
        return false;
    }
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Where a namespace was sourced from after merging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamespaceSource {
    /// Defined entirely from `INSPECT_<NS>_*` env vars (no file entry).
    EnvOnly,
    /// Defined entirely from the on-disk file (no env override).
    FileOnly,
    /// File-defined but at least one field was overridden by env vars.
    EnvOverFile,
}

/// A namespace whose values have been merged across env + file.
#[derive(Debug, Clone)]
pub struct ResolvedNamespace {
    pub name: String,
    pub config: NamespaceConfig,
    pub source: NamespaceSource,
}

/// Validate namespace short names. Lowercase, digits, dash, underscore.
/// Must start with a letter or digit. Max 63 chars.
pub fn validate_namespace_name(name: &str) -> Result<(), ConfigError> {
    use std::sync::OnceLock;

    use regex::Regex;
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^[a-z0-9][a-z0-9_-]{0,62}$").expect("valid regex"));
    if !re.is_match(name) {
        return Err(ConfigError::InvalidNamespaceName(name.to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(host: Option<&str>, user: Option<&str>, port: Option<u16>) -> NamespaceConfig {
        NamespaceConfig {
            host: host.map(String::from),
            user: user.map(String::from),
            port,
            key_path: None,
            key_passphrase_env: None,
            key_inline: None,
            env: None,
            auto_reauth: None,
            history: None,
        }
    }

    #[test]
    fn merge_env_over_file() {
        let file = cfg(Some("file.example"), Some("fileuser"), Some(22));
        let env = cfg(Some("env.example"), None, None);
        let merged = file.merge_over(&env);
        assert_eq!(merged.host.as_deref(), Some("env.example"));
        assert_eq!(merged.user.as_deref(), Some("fileuser"));
        assert_eq!(merged.port, Some(22));
    }

    #[test]
    fn validate_requires_host_and_user() {
        let mut c = cfg(None, Some("u"), None);
        c.key_path = Some("/tmp/k".into());
        assert!(c.validate("ns").is_err());

        let mut c = cfg(Some("h"), None, None);
        c.key_path = Some("/tmp/k".into());
        assert!(c.validate("ns").is_err());

        let mut c = cfg(Some("h"), Some("u"), None);
        c.key_path = Some("/tmp/k".into());
        assert!(c.validate("ns").is_ok());
    }

    #[test]
    fn validate_rejects_conflicting_key_sources() {
        let mut c = cfg(Some("h"), Some("u"), None);
        c.key_path = Some("/tmp/k".into());
        c.key_inline = Some("base64==".into());
        assert!(matches!(
            c.validate("ns"),
            Err(ConfigError::ConflictingKeySources)
        ));
    }

    #[test]
    fn namespace_name_validation() {
        assert!(validate_namespace_name("arte").is_ok());
        assert!(validate_namespace_name("prod-eu").is_ok());
        assert!(validate_namespace_name("a1_2-3").is_ok());
        assert!(validate_namespace_name("Arte").is_err());
        assert!(validate_namespace_name("").is_err());
        assert!(validate_namespace_name("-bad").is_err());
        assert!(validate_namespace_name("bad name").is_err());
    }
}
