//! Namespace data model.

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
}

impl NamespaceConfig {
    /// Merge `other` over `self`: any field set in `other` takes precedence.
    pub fn merge_over(&self, other: &NamespaceConfig) -> NamespaceConfig {
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
        Ok(())
    }
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
    use once_cell::sync::Lazy;
    use regex::Regex;
    static RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"^[a-z0-9][a-z0-9_-]{0,62}$").expect("valid regex"));
    if !RE.is_match(name) {
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
