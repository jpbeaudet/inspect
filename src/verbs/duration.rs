//! Tiny human-friendly duration parser shared by `--since` / `--until`.
//!
//! Accepted forms (case-insensitive): `30s`, `5m`, `1h`, `2d`. Bare digits
//! are interpreted as seconds.

use std::time::Duration;

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DurationError {
    #[error("duration '{0}' is empty")]
    Empty(String),
    #[error("duration '{0}' is missing a number")]
    NoNumber(String),
    #[error("duration '{0}' has unknown unit '{1}' (use s/m/h/d)")]
    BadUnit(String, char),
    #[error("duration '{0}' overflowed")]
    Overflow(String),
}

pub fn parse_duration(s: &str) -> Result<Duration, DurationError> {
    let t = s.trim();
    if t.is_empty() {
        return Err(DurationError::Empty(s.to_string()));
    }
    let (num_str, unit_char) = match t.chars().last() {
        Some(c) if c.is_ascii_digit() => (t, 's'),
        Some(c) => (&t[..t.len() - c.len_utf8()], c.to_ascii_lowercase()),
        None => unreachable!(),
    };
    if num_str.is_empty() {
        return Err(DurationError::NoNumber(s.to_string()));
    }
    let n: u64 = num_str
        .parse()
        .map_err(|_| DurationError::NoNumber(s.to_string()))?;
    let secs = match unit_char {
        's' => n,
        'm' => n
            .checked_mul(60)
            .ok_or_else(|| DurationError::Overflow(s.to_string()))?,
        'h' => n
            .checked_mul(3600)
            .ok_or_else(|| DurationError::Overflow(s.to_string()))?,
        'd' => n
            .checked_mul(86400)
            .ok_or_else(|| DurationError::Overflow(s.to_string()))?,
        other => return Err(DurationError::BadUnit(s.to_string(), other)),
    };
    Ok(Duration::from_secs(secs))
}

/// Render a Duration back as the canonical short form (largest fitting unit).
#[cfg(test)]
pub fn fmt_short(d: Duration) -> String {
    let s = d.as_secs();
    if s % 86400 == 0 && s >= 86400 {
        format!("{}d", s / 86400)
    } else if s % 3600 == 0 && s >= 3600 {
        format!("{}h", s / 3600)
    } else if s % 60 == 0 && s >= 60 {
        format!("{}m", s / 60)
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms() {
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
        assert_eq!(
            parse_duration("2d").unwrap(),
            Duration::from_secs(2 * 86400)
        );
        assert_eq!(parse_duration("45").unwrap(), Duration::from_secs(45));
    }

    #[test]
    fn bad_unit() {
        assert!(matches!(
            parse_duration("1y"),
            Err(DurationError::BadUnit(_, 'y'))
        ));
    }

    #[test]
    fn fmt_round_trip() {
        for s in ["30s", "5m", "1h", "2d"] {
            let d = parse_duration(s).unwrap();
            assert_eq!(fmt_short(d), s);
        }
    }
}
