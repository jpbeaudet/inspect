//! Saved-selector aliases (bible §6.7).
//!
//! - Storage: `~/.inspect/aliases.toml`, mode 0600, dir 0700.
//! - Two flavors:
//!   - **verb-style**: any selector that parses as a verb selector
//!     (`arte/pulse`, `prod-*/storage`, `~/foo`).
//!   - **logql-style**: a `{server=...}` LogQL selector. Cannot be used in
//!     verb commands; produces a friendly error pointing the user at
//!     `inspect search`.
//! - v1: no chaining (alias bodies cannot reference other aliases).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::error::ConfigError;
use crate::paths;
use crate::selector::parser::{parse_selector, SelectorParseError};

/// Top-level on-disk model.
#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AliasFile {
    #[serde(default)]
    pub aliases: BTreeMap<String, AliasEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AliasEntry {
    /// Raw selector text. Stored verbatim.
    pub selector: String,
    /// Optional human description shown by `alias list`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// What kind of selector this alias holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AliasKind {
    Verb,
    LogQl,
}

impl AliasKind {
    pub fn label(self) -> &'static str {
        match self {
            AliasKind::Verb => "verb",
            AliasKind::LogQl => "logql",
        }
    }
}

#[derive(Debug, Error)]
pub enum AliasError {
    #[error(
        "alias '@{name}' is a LogQL selector, not a verb selector.\n\
         For verb commands, run: inspect alias add {suggestion} '<verb-selector>'"
    )]
    LogQlInVerbContext { name: String, suggestion: String },

    #[error("alias '@{0}' is not defined; run 'inspect alias list' to see available aliases")]
    Unknown(String),

    #[error(
        "alias '@{name}' references another alias ('@{target}'); chaining is not supported in v1"
    )]
    Chain { name: String, target: String },

    #[error("alias name '{0}' is invalid: must be [a-z0-9][a-z0-9_-]{{0,62}}")]
    InvalidName(String),

    #[error("alias '@{0}' already exists; pass --force to overwrite")]
    Exists(String),

    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error("alias body for '@{name}' cannot be parsed as a verb selector: {source}")]
    BadBody {
        name: String,
        #[source]
        source: SelectorParseError,
    },
}

/// Validate alias name shape. Same rules as namespace names.
pub fn validate_alias_name(name: &str) -> Result<(), AliasError> {
    let ok = !name.is_empty()
        && name.len() <= 63
        && name
            .chars()
            .next()
            .map(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            .unwrap_or(false)
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(AliasError::InvalidName(name.to_string()))
    }
}

/// Classify an alias body by static prefix.
pub fn classify(body: &str) -> AliasKind {
    let t = body.trim_start();
    if t.starts_with('{') {
        AliasKind::LogQl
    } else {
        AliasKind::Verb
    }
}

/// Read aliases from disk. Returns an empty map if the file doesn't exist.
pub fn load() -> Result<AliasFile, AliasError> {
    let path = paths::aliases_toml();
    if !path.exists() {
        return Ok(AliasFile::default());
    }
    paths::check_file_mode_0600(&path)?;
    let body = std::fs::read_to_string(&path).map_err(|e| ConfigError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    let parsed: AliasFile = toml::from_str(&body).map_err(|e| ConfigError::Parse {
        path: path.display().to_string(),
        source: e,
    })?;
    Ok(parsed)
}

/// Write aliases to disk atomically with mode 0600.
pub fn save(file: &AliasFile) -> Result<(), AliasError> {
    paths::ensure_home()?;
    let path = paths::aliases_toml();
    let body = toml::to_string_pretty(file).map_err(ConfigError::from)?;

    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(|e| ConfigError::Io {
        path: dir.display().to_string(),
        source: e,
    })?;
    use std::io::Write;
    tmp.write_all(body.as_bytes())
        .map_err(|e| ConfigError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
    tmp.flush().map_err(|e| ConfigError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    tmp.as_file().sync_all().map_err(|e| ConfigError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    let tp = tmp.into_temp_path();
    tp.persist(&path).map_err(|e| ConfigError::Io {
        path: path.display().to_string(),
        source: e.error,
    })?;
    paths::set_file_mode_0600(&path)?;
    Ok(())
}

/// Add or replace an alias.
pub fn add(
    name: &str,
    body: &str,
    description: Option<String>,
    force: bool,
) -> Result<(), AliasError> {
    validate_alias_name(name)?;
    if body.trim().is_empty() {
        return Err(AliasError::BadBody {
            name: name.to_string(),
            source: SelectorParseError::Empty,
        });
    }
    // Reject chaining: an alias cannot reference another alias.
    if body.trim_start().starts_with('@') {
        let target: String = body
            .trim_start()
            .trim_start_matches('@')
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string();
        return Err(AliasError::Chain {
            name: name.to_string(),
            target,
        });
    }
    // Sanity: verb-style aliases must parse.
    if classify(body) == AliasKind::Verb {
        if let Err(e) = parse_selector(body) {
            return Err(AliasError::BadBody {
                name: name.to_string(),
                source: e,
            });
        }
    }
    let mut file = load()?;
    if file.aliases.contains_key(name) && !force {
        return Err(AliasError::Exists(name.to_string()));
    }
    file.aliases.insert(
        name.to_string(),
        AliasEntry {
            selector: body.to_string(),
            description,
        },
    );
    save(&file)
}

/// Remove an alias. Returns `Ok(false)` if it didn't exist.
pub fn remove(name: &str) -> Result<bool, AliasError> {
    validate_alias_name(name)?;
    let mut file = load()?;
    let removed = file.aliases.remove(name).is_some();
    if removed {
        save(&file)?;
    }
    Ok(removed)
}

/// Lookup alias body by name.
pub fn get(name: &str) -> Result<Option<AliasEntry>, AliasError> {
    Ok(load()?.aliases.get(name).cloned())
}

/// List aliases sorted by name.
pub fn list() -> Result<Vec<(String, AliasEntry)>, AliasError> {
    Ok(load()?.aliases.into_iter().collect::<Vec<_>>())
}

/// Expand a `@name` token into its body, enforcing the no-chain rule. If
/// `input` doesn't start with `@`, returns `Ok(input.to_string())`.
pub fn expand(input: &str) -> Result<(String, AliasKind), AliasError> {
    let t = input.trim();
    if let Some(rest) = t.strip_prefix('@') {
        let name = rest;
        validate_alias_name(name)?;
        let entry = get(name)?.ok_or_else(|| AliasError::Unknown(name.to_string()))?;
        let kind = classify(&entry.selector);
        return Ok((entry.selector, kind));
    }
    Ok((t.to_string(), classify(t)))
}

/// Expand and require verb form. Used by all read/write verbs.
pub fn expand_for_verb(input: &str) -> Result<String, AliasError> {
    let t = input.trim();
    if let Some(rest) = t.strip_prefix('@') {
        let name = rest.to_string();
        let (body, kind) = expand(t)?;
        if kind == AliasKind::LogQl {
            return Err(AliasError::LogQlInVerbContext {
                name: name.clone(),
                suggestion: format!("{name}-v"),
            });
        }
        return Ok(body);
    }
    Ok(t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::MutexGuard;

    fn lock() -> MutexGuard<'static, ()> {
        // Crate-wide mutex shared with every other module that mutates
        // INSPECT_HOME in tests. See src/paths.rs::TEST_ENV_LOCK.
        crate::paths::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    struct Home {
        _g: MutexGuard<'static, ()>,
        _d: tempfile::TempDir,
    }
    fn temp_home() -> Home {
        let g = lock();
        let d = tempfile::tempdir().unwrap();
        std::env::set_var(crate::paths::INSPECT_HOME_ENV, d.path());
        Home { _g: g, _d: d }
    }

    #[test]
    fn add_list_remove_round_trip() {
        let _h = temp_home();
        add("plogs", "arte/pulse", None, false).unwrap();
        let entries = list().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "plogs");
        assert!(remove("plogs").unwrap());
        assert_eq!(list().unwrap().len(), 0);
    }

    #[test]
    fn classify_branches() {
        assert_eq!(classify("arte/pulse"), AliasKind::Verb);
        assert_eq!(classify("{server=\"arte\"}"), AliasKind::LogQl);
    }

    #[test]
    fn expand_logql_in_verb_errors() {
        let _h = temp_home();
        add("q", "{server=\"arte\"}", None, false).unwrap();
        let err = expand_for_verb("@q").unwrap_err();
        assert!(matches!(err, AliasError::LogQlInVerbContext { .. }));
    }

    #[test]
    fn refuses_chain() {
        let _h = temp_home();
        let err = add("a", "@b", None, false).unwrap_err();
        assert!(matches!(err, AliasError::Chain { .. }));
    }

    #[test]
    fn refuses_unknown_alias() {
        let _h = temp_home();
        let err = expand("@ghost").unwrap_err();
        assert!(matches!(err, AliasError::Unknown(_)));
    }

    #[test]
    fn refuses_bad_verb_body() {
        let _h = temp_home();
        let err = add("bad", "{}", None, false);
        // `{}` classifies as logql so it's accepted at add-time. We cover
        // the bad-verb case with an unparseable body:
        let _ = err;
        let err = add("bad2", "arte/", None, false).unwrap_err();
        assert!(matches!(err, AliasError::BadBody { .. }));
    }

    #[test]
    fn force_overwrites() {
        let _h = temp_home();
        add("a", "arte/pulse", None, false).unwrap();
        let err = add("a", "arte/atlas", None, false).unwrap_err();
        assert!(matches!(err, AliasError::Exists(_)));
        add("a", "arte/atlas", None, true).unwrap();
        let e = get("a").unwrap().unwrap();
        assert_eq!(e.selector, "arte/atlas");
    }
}
