//! Resolve a namespace from `INSPECT_<NS>_*` environment variables.
//!
//! Recognized variables, all optional:
//!
//! - `INSPECT_<NS>_HOST`
//! - `INSPECT_<NS>_USER`
//! - `INSPECT_<NS>_PORT`
//! - `INSPECT_<NS>_KEY_PATH`
//! - `INSPECT_<NS>_KEY_PASSPHRASE_ENV`
//! - `INSPECT_<NS>_KEY_INLINE`
//!
//! `KEY_PATH` and `KEY_INLINE` are mutually exclusive; the resolver returns
//! both so the caller's [`NamespaceConfig::validate`] can flag conflicts.

use std::collections::BTreeSet;

use super::namespace::NamespaceConfig;

/// Read env-var overrides for `namespace`. Returns `None` when no recognized
/// variable is set for that namespace.
pub fn read_env(namespace: &str) -> Option<NamespaceConfig> {
    let prefix = env_prefix(namespace);
    let host = std::env::var(format!("{prefix}HOST")).ok();
    let user = std::env::var(format!("{prefix}USER")).ok();
    let port = std::env::var(format!("{prefix}PORT"))
        .ok()
        .and_then(|s| s.parse::<u16>().ok());
    let key_path = std::env::var(format!("{prefix}KEY_PATH")).ok();
    let key_passphrase_env = std::env::var(format!("{prefix}KEY_PASSPHRASE_ENV")).ok();
    let key_inline = std::env::var(format!("{prefix}KEY_INLINE")).ok();

    if host.is_none()
        && user.is_none()
        && port.is_none()
        && key_path.is_none()
        && key_passphrase_env.is_none()
        && key_inline.is_none()
    {
        return None;
    }
    Some(NamespaceConfig {
        host,
        user,
        port,
        key_path,
        key_passphrase_env,
        key_inline,
        // Env-overlay overrides via env vars are out of
        // scope for v0.1.3 (would conflict with the existing
        // `INSPECT_<NS>_*` suffix-matching scheme used by
        // `enumerate_env_namespaces`). The overlay is config-file only.
        env: None,
        // Auto_reauth has no env-var override path; it is
        // a per-namespace policy that lives in `servers.toml`.
        auto_reauth: None,
        // Per-namespace transcript override is
        // config-file only (the policy is rarely set per-invocation;
        // the file pattern matches the auto-reauth + env-overlay design).
        history: None,
        // Auth / password_env / session_ttl are
        // config-file only — same rationale as the env-overlay /
        // auto-reauth / transcript designs (the
        // policy rarely changes per invocation, and password
        // material in env vars is what `password_env` itself names,
        // so the env-override path is not where that value belongs).
        auth: None,
        password_env: None,
        session_ttl: None,
    })
}

/// Discover all namespace names that appear in the current environment via
/// the `INSPECT_<NS>_*` convention. Lowercased.
pub fn enumerate_env_namespaces() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let suffixes = [
        "_HOST",
        "_USER",
        "_PORT",
        "_KEY_PATH",
        "_KEY_PASSPHRASE_ENV",
        "_KEY_INLINE",
    ];
    for (key, _) in std::env::vars() {
        let Some(rest) = key.strip_prefix("INSPECT_") else {
            continue;
        };
        // Match the longest suffix to avoid clipping namespace names like
        // FOO_KEY (which would contain `_KEY`).
        let mut matched: Option<&str> = None;
        for suf in suffixes {
            if rest.ends_with(suf)
                && rest.len() > suf.len()
                && matched.map(|m| m.len()).unwrap_or(0) < suf.len()
            {
                matched = Some(suf);
            }
        }
        if let Some(suf) = matched {
            let ns_upper = &rest[..rest.len() - suf.len()];
            if ns_upper.is_empty() {
                continue;
            }
            // Reserved global namespaces (e.g. INSPECT_HOME, INSPECT_FLEET_*).
            if is_reserved(ns_upper) {
                continue;
            }
            out.insert(ns_upper.to_ascii_lowercase());
        }
    }
    out
}

fn env_prefix(namespace: &str) -> String {
    format!("INSPECT_{}_", namespace.to_ascii_uppercase())
}

fn is_reserved(ns_upper: &str) -> bool {
    matches!(ns_upper, "FLEET" | "HOME" | "AUDIT" | "DEBUG" | "LOG")
}
