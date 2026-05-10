//! On-disk storage for `~/.inspect/groups.toml` (Phase 11).
//!
//! File layout:
//!
//! ```toml
//! schema_version = 1
//!
//! [groups.prod]
//! members = ["prod-*"]
//!
//! [groups.canaries]
//! members = ["prod-1", "staging-canary"]
//! ```
//!
//! Each group has a `members` list whose entries are namespace names or
//! shell-style globs (`*`, `?`, `[...]`). Resolution against the live set
//! of configured namespaces happens in `expand_group`.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::ConfigError;
use crate::paths;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GroupsFile {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub groups: BTreeMap<String, GroupSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GroupSpec {
    #[serde(default)]
    pub members: Vec<String>,
}

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

/// Load `groups.toml` from the inspect home. Returns an empty file if
/// the path does not exist. If it does exist, enforces 0600 permissions
/// before reading (matching `servers.toml` and `aliases.toml`).
pub fn load() -> Result<GroupsFile, ConfigError> {
    let path = paths::groups_toml();
    if !path.exists() {
        return Ok(GroupsFile::default());
    }
    paths::check_file_mode_0600(&path)?;
    load_from(&path)
}

pub fn load_from(path: &Path) -> Result<GroupsFile, ConfigError> {
    let bytes = std::fs::read(path).map_err(|e| ConfigError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let parsed: GroupsFile = toml::from_str(&text).map_err(|e| ConfigError::Parse {
        path: path.display().to_string(),
        source: e,
    })?;
    Ok(parsed)
}

/// Expand `members` (names + globs) against the list of configured
/// namespaces. Returns the matched names in stable, sorted order.
pub fn expand_members(members: &[String], known: &[String]) -> Vec<String> {
    let mut out: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for m in members {
        if m.contains(['*', '?', '[']) {
            for n in known {
                if glob_matches(m, n) {
                    out.insert(n.clone());
                }
            }
        } else if known.iter().any(|n| n == m) {
            out.insert(m.clone());
        }
    }
    out.into_iter().collect()
}

fn glob_matches(pat: &str, name: &str) -> bool {
    use regex::Regex;
    let mut re = String::with_capacity(pat.len() + 4);
    re.push('^');
    let mut chars = pat.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => re.push_str(".*"),
            '?' => re.push('.'),
            '[' => {
                re.push('[');
                for cc in chars.by_ref() {
                    re.push(cc);
                    if cc == ']' {
                        break;
                    }
                }
            }
            '.' | '+' | '(' | ')' | '|' | '^' | '$' | '{' | '}' | '\\' => {
                re.push('\\');
                re.push(c);
            }
            _ => re.push(c),
        }
    }
    re.push('$');
    Regex::new(&re).map(|r| r.is_match(name)).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal() {
        let toml = r#"schema_version = 1
[groups.prod]
members = ["prod-*"]
[groups.canaries]
members = ["prod-1", "staging-canary"]
"#;
        let parsed: GroupsFile = toml::from_str(toml).unwrap();
        assert_eq!(parsed.groups.len(), 2);
        assert_eq!(parsed.groups["prod"].members, vec!["prod-*"]);
    }

    #[test]
    fn expand_with_globs() {
        let known = vec![
            "arte".to_string(),
            "prod-1".to_string(),
            "prod-2".to_string(),
            "staging".to_string(),
        ];
        let got = expand_members(&["prod-*".to_string(), "arte".to_string()], &known);
        assert_eq!(
            got,
            vec![
                "arte".to_string(),
                "prod-1".to_string(),
                "prod-2".to_string()
            ]
        );
    }

    #[test]
    fn expand_skips_unknown_literal() {
        let known = vec!["arte".to_string()];
        let got = expand_members(&["nope".to_string()], &known);
        assert!(got.is_empty());
    }
}
