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
    CodespaceDefault,
    LocalDefault,
}

impl TtlSource {
    pub fn label(self) -> &'static str {
        match self {
            TtlSource::Flag => "--ttl flag",
            TtlSource::Env => "INSPECT_PERSIST_TTL env",
            TtlSource::CodespaceDefault => "Codespace default (4h)",
            TtlSource::LocalDefault => "default (30m)",
        }
    }
}

/// Returns the default TTL string (e.g. `"4h"` or `"30m"`) plus its source.
pub fn default_ttl() -> (String, TtlSource) {
    if std::env::var(ENV_CODESPACES).ok().as_deref() == Some("true") {
        ("4h".to_string(), TtlSource::CodespaceDefault)
    } else {
        ("30m".to_string(), TtlSource::LocalDefault)
    }
}

/// Resolve the TTL string used for `ControlPersist`, honoring env override.
pub fn resolve(flag: Option<&str>) -> Result<(String, TtlSource)> {
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
}
