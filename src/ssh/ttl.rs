//! Compute SSH ControlPersist TTL with Codespace-aware defaults.
//!
//! Per the bible §4.3:
//! - default `persist_ttl = "4h"` inside Codespaces
//! - `"30m"` elsewhere
//!
//! Override sources, in priority order:
//! 1. `INSPECT_PERSIST_TTL` environment variable (operator escape hatch)
//! 2. `--ttl` flag on `inspect connect`
//! 3. Codespace-aware default

use std::time::Duration;

use anyhow::{anyhow, Result};

pub const ENV_TTL_OVERRIDE: &str = "INSPECT_PERSIST_TTL";
pub const ENV_CODESPACES: &str = "CODESPACES";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtlSource {
    Flag,
    Env,
    /// Per-namespace `session_ttl` from
    /// `~/.inspect/servers.toml`. Slots between the env override and
    /// the auth-mode default.
    PerNamespace,
    /// Default for `auth = "password"` namespaces (`12h`).
    /// Operators on legacy boxes don't want to re-prompt every 30m.
    PasswordDefault,
    CodespaceDefault,
    LocalDefault,
}

impl TtlSource {
    pub fn label(self) -> &'static str {
        match self {
            TtlSource::Flag => "--ttl flag",
            TtlSource::Env => "INSPECT_PERSIST_TTL env",
            TtlSource::PerNamespace => "namespace session_ttl",
            TtlSource::PasswordDefault => "password-auth default (12h)",
            TtlSource::CodespaceDefault => "Codespace default (4h)",
            TtlSource::LocalDefault => "default (30m)",
        }
    }
}

/// Hard cap on any operator-supplied TTL when the
/// namespace uses password auth. Mirrors the schema cap so
/// `--ttl 48h` against a password-auth ns is rejected the same way
/// `session_ttl = "48h"` is.
pub const PASSWORD_AUTH_TTL_CAP_SECS: u64 = 24 * 60 * 60;

/// Returns the default TTL string (e.g. `"4h"` or `"30m"`) plus its source.
pub fn default_ttl() -> (String, TtlSource) {
    if std::env::var(ENV_CODESPACES).ok().as_deref() == Some("true") {
        ("4h".to_string(), TtlSource::CodespaceDefault)
    } else {
        ("30m".to_string(), TtlSource::LocalDefault)
    }
}

/// TTL resolver with per-namespace context. Priority:
/// `--ttl` flag → `INSPECT_PERSIST_TTL` env → per-namespace
/// `session_ttl` → password-default (12h) when `auth = "password"`
/// → Codespace/local default. When the resolved namespace uses
/// password auth, the final TTL is capped at 24h regardless of
/// where it came from (so `--ttl 48h legacy-box` is rejected the
/// same way `session_ttl = "48h"` is).
pub fn resolve_with_ns(
    flag: Option<&str>,
    per_ns: Option<&str>,
    password_auth: Option<bool>,
) -> Result<(String, TtlSource)> {
    let (value, source) = resolve_inner(flag, per_ns, password_auth)?;
    if password_auth == Some(true) {
        let dur = parse_ttl(&value)?;
        if dur.as_secs() > PASSWORD_AUTH_TTL_CAP_SECS {
            return Err(anyhow!(
                "ttl '{value}' (from {src}) exceeds the 24h password-auth cap; \
                 pick a shorter duration so a forgotten session does not stay live indefinitely",
                src = source.label()
            ));
        }
    }
    Ok((value, source))
}

fn resolve_inner(
    flag: Option<&str>,
    per_ns: Option<&str>,
    password_auth: Option<bool>,
) -> Result<(String, TtlSource)> {
    if let Some(v) = flag {
        validate(v)?;
        return Ok((v.to_string(), TtlSource::Flag));
    }
    if let Ok(env) = std::env::var(ENV_TTL_OVERRIDE) {
        if !env.is_empty() {
            validate(&env)?;
            return Ok((env, TtlSource::Env));
        }
    }
    if let Some(v) = per_ns {
        validate(v)?;
        return Ok((v.to_string(), TtlSource::PerNamespace));
    }
    if password_auth == Some(true) {
        return Ok(("12h".to_string(), TtlSource::PasswordDefault));
    }
    Ok(default_ttl())
}

/// Validate a TTL string: integer + unit (`s`, `m`, `h`, `d`). Used both for
/// human-readable display and to confirm OpenSSH will accept it.
pub fn parse_ttl(s: &str) -> Result<Duration> {
    if s.is_empty() {
        return Err(anyhow!("empty ttl"));
    }
    let (num, unit) = split_num_unit(s)?;
    let multiplier = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 60 * 60,
        "d" => 24 * 60 * 60,
        other => return Err(anyhow!("invalid ttl unit '{other}', expected s|m|h|d")),
    };
    let secs = num
        .checked_mul(multiplier)
        .ok_or_else(|| anyhow!("ttl '{s}' overflows"))?;
    Ok(Duration::from_secs(secs))
}

fn validate(s: &str) -> Result<()> {
    parse_ttl(s).map(|_| ())
}

fn split_num_unit(s: &str) -> Result<(u64, &str)> {
    let idx = s
        .find(|c: char| !c.is_ascii_digit())
        .ok_or_else(|| anyhow!("ttl '{s}' missing unit (use s|m|h|d)"))?;
    if idx == 0 {
        return Err(anyhow!("ttl '{s}' missing leading number"));
    }
    let num: u64 = s[..idx]
        .parse()
        .map_err(|_| anyhow!("invalid ttl number in '{s}'"))?;
    Ok((num, &s[idx..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_units() {
        assert_eq!(parse_ttl("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_ttl("30m").unwrap(), Duration::from_secs(30 * 60));
        assert_eq!(parse_ttl("4h").unwrap(), Duration::from_secs(4 * 3600));
        assert_eq!(parse_ttl("2d").unwrap(), Duration::from_secs(2 * 86400));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_ttl("").is_err());
        assert!(parse_ttl("abc").is_err());
        assert!(parse_ttl("30").is_err());
        assert!(parse_ttl("3x").is_err());
        assert!(parse_ttl("h30").is_err());
    }

    // ---- : per-namespace + password defaults --------------------

    #[test]
    fn l4_resolve_with_ns_picks_per_ns_over_default() {
        let (v, src) = resolve_with_ns(None, Some("8h"), Some(true)).unwrap();
        assert_eq!(v, "8h");
        assert_eq!(src, TtlSource::PerNamespace);
    }

    #[test]
    fn l4_resolve_with_ns_password_default_when_unset() {
        let (v, src) = resolve_with_ns(None, None, Some(true)).unwrap();
        assert_eq!(v, "12h");
        assert_eq!(src, TtlSource::PasswordDefault);
    }

    #[test]
    fn l4_resolve_with_ns_key_auth_unchanged_default() {
        let (_v, src) = resolve_with_ns(None, None, Some(false)).unwrap();
        // Either local or codespace default depending on env; never password.
        assert!(matches!(
            src,
            TtlSource::LocalDefault | TtlSource::CodespaceDefault
        ));
    }

    #[test]
    fn l4_resolve_with_ns_flag_wins() {
        let (v, src) = resolve_with_ns(Some("2h"), Some("8h"), Some(true)).unwrap();
        assert_eq!(v, "2h");
        assert_eq!(src, TtlSource::Flag);
    }

    #[test]
    fn l4_resolve_with_ns_caps_at_24h_for_password() {
        // Per-namespace 48h is rejected when password_auth=true even
        // though 48h passes basic ttl parsing.
        let err = resolve_with_ns(None, Some("48h"), Some(true)).unwrap_err();
        assert!(err.to_string().contains("24h password-auth cap"));
        // 24h exactly is fine.
        assert!(resolve_with_ns(None, Some("24h"), Some(true)).is_ok());
        // 24h is fine for key auth too — the cap only applies to password mode.
        assert!(resolve_with_ns(Some("48h"), None, Some(false)).is_ok());
    }
}
