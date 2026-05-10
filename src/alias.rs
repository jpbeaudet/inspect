//! Saved-selector aliases (bible §6.7).
//!
//! - Storage: `~/.inspect/aliases.toml`, mode 0600, dir 0700.
//! - Two flavors:
//!   - **verb-style**: any selector that parses as a verb selector
//!     (`arte/pulse`, `prod-*/storage`, `~/foo`).
//!   - **logql-style**: a `{server=...}` LogQL selector. Cannot be used in
//!     verb commands; produces a friendly error pointing the user at
//!     `inspect search`.
//! - : aliases gain `$param` placeholders. Call sites are
//!   `@name(k=v,k=v)`; bare `@name` continues to work for parameterless
//!   aliases. Aliases may chain other aliases (depth cap 5; cycles
//!   rejected at `alias add` time via DFS over the alias graph).
//!   `$$` in a body is a literal `$` escape.
//!
//! On-disk shape grew an optional `parameters: []` cache in v0.1.1. Earlier
//! `aliases.toml` files (no `parameters` field) deserialize unchanged.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::error::ConfigError;
use crate::paths;
use crate::selector::parser::{parse_selector, SelectorParseError};

/// Maximum alias-chain expansion depth. Beyond this the runtime
/// errors with the full chain printed. (`add()` already rejects
/// definitional cycles, so depth-cap is a runtime guard against
/// hand-edited `aliases.toml` files that introduce a cycle the
/// in-process state never re-validated.)
pub const MAX_CHAIN_DEPTH: usize = 5;

/// Top-level on-disk model.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AliasFile {
    #[serde(default)]
    pub aliases: BTreeMap<String, AliasEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AliasEntry {
    /// Raw selector text. Stored verbatim (placeholders `$name` are
    /// expanded at call time via the cached `parameters` list).
    pub selector: String,
    /// Optional human description shown by `alias list`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Cached list of `$param` placeholder names extracted
    /// from `selector` at `alias add` time, in first-occurrence order.
    /// `None` means "earlier entry, no parameters" (deserializes from
    /// older files unchanged); `Some(empty)` means "parameter-aware entry that
    /// happens to take no params". Both forms are operationally
    /// equivalent for parameterless aliases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Vec<String>>,
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

    /// A `@name(k=v,...)` call site is missing a required parameter.
    /// `all_required` is rendered into the error message so the operator
    /// can see every param the alias declares without a separate lookup.
    #[error(
        "alias '@{name}' requires param '{required}' \
         (declared params: {declared}; call as @{name}({example}))"
    )]
    MissingParam {
        name: String,
        required: String,
        declared: String,
        example: String,
    },

    /// A `@name(k=v,...)` call site supplied a param the alias body
    /// does not declare.
    #[error(
        "alias '@{name}' got unknown param '{extra}' \
         (declared params: {declared}). Did the alias body change recently? \
         hint: 'inspect alias show {name}' lists the current params"
    )]
    ExtraParam {
        name: String,
        extra: String,
        declared: String,
    },

    /// `Add()` refuses to write an alias whose body would form a
    /// cycle in the alias graph. The chain is printed `a -> b -> a` so
    /// the operator sees exactly which existing alias closes the cycle.
    #[error("circular alias reference: {chain}; refusing to write")]
    CircularReference { chain: String },

    /// Runtime guard for `MAX_CHAIN_DEPTH`. Definitional cycles are
    /// caught by `add()`; this fires when a hand-edited `aliases.toml`
    /// or a chain longer than the cap is invoked.
    #[error(
        "alias chain depth exceeded ({MAX_CHAIN_DEPTH}): {chain}; \
         hint: shorten the chain or split into a single alias"
    )]
    ChainDepthExceeded { chain: String },

    /// A call-site `@name(...)` is malformed.
    #[error("alias call site is malformed: {hint}")]
    BadCallSyntax { hint: String },
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

/// Classify an alias body by static prefix. The classification is done
/// **before** `$param` substitution because the body shape (LogQL-vs-
/// verb) is a static property of the alias declaration, not of any one
/// call site's params.
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
///
/// The body may contain `$<ident>` placeholders (extracted into the
/// cached `parameters` list) and `@other(...)` references to other
/// aliases. Definitional cycles are caught here via DFS over the
/// existing alias graph; the runtime depth-cap (`MAX_CHAIN_DEPTH`) is a
/// secondary guard against hand-edited files that bypass this check.
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
    // Verb-style aliases must have a substituted body that parses as a
    // selector. We can't validate that without supplying placeholder
    // values — parameterless aliases get the strict check, parameterized
    // ones rely on call-time validation.
    let parameters = extract_parameters(body);
    if classify(body) == AliasKind::Verb && parameters.is_empty() {
        // No `$param` placeholders and no `@other(...)` chain markers
        // mean the body is a static selector that must parse today.
        if !body_contains_alias_ref(body) {
            if let Err(e) = parse_selector(body) {
                return Err(AliasError::BadBody {
                    name: name.to_string(),
                    source: e,
                });
            }
        }
    }

    let mut file = load()?;
    if file.aliases.contains_key(name) && !force {
        return Err(AliasError::Exists(name.to_string()));
    }

    // Definitional-cycle detection: walk the would-be alias graph (the
    // existing file plus the candidate insertion) and DFS from `name`.
    // Reject the write if the candidate's body's `@other(...)`
    // references reach back to `name` through any path.
    let mut candidate = file.clone();
    candidate.aliases.insert(
        name.to_string(),
        AliasEntry {
            selector: body.to_string(),
            description: description.clone(),
            parameters: Some(parameters.clone()),
        },
    );
    if let Some(chain) = find_cycle(&candidate, name) {
        return Err(AliasError::CircularReference {
            chain: chain.join(" -> "),
        });
    }

    file.aliases.insert(
        name.to_string(),
        AliasEntry {
            selector: body.to_string(),
            description,
            parameters: Some(parameters),
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

// ===========================================================================
// Parameter extraction + substitution + call-site parsing.
// ===========================================================================

/// One placeholder occurrence parsed out of an alias body.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Placeholder {
    /// Byte range in the body covering the entire placeholder token
    /// (`$svc`, `${svc}`, or `${svc:-default}`).
    span: std::ops::Range<usize>,
    /// Parameter name.
    name: String,
    /// Default value if the body used the `${name:-default}` form.
    /// `None` means the placeholder is required.
    default: Option<String>,
}

/// Walk a body and yield every placeholder occurrence. Recognizes
/// three forms: `$ident`, `${ident}`, and `${ident:-default}`. `$$`
/// is the literal-`$` escape. Defaults may not contain a literal `}`
/// directly; use `\}` to embed one.
fn scan_placeholders(body: &str) -> Vec<Placeholder> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() {
            let next = bytes[i + 1];
            if next == b'$' {
                i += 2;
                continue;
            }
            if next == b'{' {
                if let Some(ph) = parse_braced_placeholder(body, i) {
                    let end = ph.span.end;
                    out.push(ph);
                    i = end;
                    continue;
                }
            }
            if next.is_ascii_alphabetic() || next == b'_' {
                let start = i + 1;
                let mut j = start;
                while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                    j += 1;
                }
                out.push(Placeholder {
                    span: i..j,
                    name: body[start..j].to_string(),
                    default: None,
                });
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Parse a `${name}` or `${name:-default}` token starting at `body[i]`
/// (where `body[i] == '$'` and `body[i + 1] == '{'`). Returns `None`
/// for malformed tokens (missing `}`, empty/invalid name) so the
/// scanner emits the literal `${...` text verbatim — operators see
/// their typo rather than a silent disappearance.
fn parse_braced_placeholder(body: &str, i: usize) -> Option<Placeholder> {
    let bytes = body.as_bytes();
    debug_assert!(bytes[i] == b'$' && bytes.get(i + 1) == Some(&b'{'));

    let name_start = i + 2;
    if name_start >= bytes.len() {
        return None;
    }
    let first = bytes[name_start];
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return None;
    }
    let mut j = name_start;
    while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
        j += 1;
    }
    let name = body[name_start..j].to_string();

    if j < bytes.len() && bytes[j] == b'}' {
        return Some(Placeholder {
            span: i..j + 1,
            name,
            default: None,
        });
    }

    if j + 1 < bytes.len() && bytes[j] == b':' && bytes[j + 1] == b'-' {
        let default_start = j + 2;
        let mut k = default_start;
        let mut default_value = String::new();
        while k < bytes.len() {
            let c = bytes[k];
            if c == b'\\' && k + 1 < bytes.len() {
                default_value.push(bytes[k + 1] as char);
                k += 2;
                continue;
            }
            if c == b'}' {
                return Some(Placeholder {
                    span: i..k + 1,
                    name,
                    default: Some(default_value),
                });
            }
            default_value.push(c as char);
            k += 1;
        }
        return None;
    }

    None
}

/// Scan an alias body for placeholder names. Returns the list of
/// unique parameter names in first-occurrence order. Recognizes
/// `$ident`, `${ident}`, and `${ident:-default}` forms; `$$` is the
/// literal-`$` escape. Placeholders inside double-quoted strings are
/// recognized too (the LogQL example `{server="$svc"}` relies on this).
pub fn extract_parameters(body: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for ph in scan_placeholders(body) {
        if !names.contains(&ph.name) {
            names.push(ph.name);
        }
    }
    names
}

/// Return the per-parameter default values declared in the body via
/// the `${name:-default}` form. A parameter that appears in both a
/// bare `$name` form and a `${name:-default}` form is treated as
/// optional (the default applies whenever the call site omits the
/// param). When the same name appears with two distinct defaults,
/// first-occurrence wins (consistent with `extract_parameters`'
/// first-occurrence ordering rule).
pub fn extract_defaults(body: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for ph in scan_placeholders(body) {
        if let Some(d) = ph.default {
            out.entry(ph.name).or_insert(d);
        }
    }
    out
}

/// Substitute placeholders in `body` using `params`, falling back to
/// per-parameter defaults declared via `${name:-default}` when a param
/// is omitted. Errors if any required (no-default) placeholder is
/// missing or any extra param is supplied. `$$` becomes `$`.
pub fn substitute_params(
    body: &str,
    name: &str,
    params: &BTreeMap<String, String>,
) -> Result<String, AliasError> {
    let placeholders = scan_placeholders(body);
    let declared_set: std::collections::BTreeSet<&str> =
        placeholders.iter().map(|p| p.name.as_str()).collect();
    let declared_list: Vec<String> = {
        let mut v: Vec<String> = Vec::new();
        for p in &placeholders {
            if !v.contains(&p.name) {
                v.push(p.name.clone());
            }
        }
        v
    };
    let declared_str = if declared_list.is_empty() {
        "(none)".to_string()
    } else {
        declared_list.join(", ")
    };

    for given in params.keys() {
        if !declared_set.contains(given.as_str()) {
            return Err(AliasError::ExtraParam {
                name: name.to_string(),
                extra: given.clone(),
                declared: declared_str.clone(),
            });
        }
    }

    let mut out = String::with_capacity(body.len());
    let bytes = body.as_bytes();
    let mut cursor = 0;
    let mut ph_iter = placeholders.iter().peekable();

    while cursor < bytes.len() {
        if bytes[cursor] == b'$' && cursor + 1 < bytes.len() && bytes[cursor + 1] == b'$' {
            out.push('$');
            cursor += 2;
            continue;
        }

        match ph_iter.peek() {
            Some(ph) if ph.span.start == cursor => {
                let value = if let Some(v) = params.get(&ph.name) {
                    v.clone()
                } else if let Some(d) = ph.default.clone() {
                    d
                } else {
                    let example = declared_list
                        .iter()
                        .map(|p| format!("{p}=..."))
                        .collect::<Vec<_>>()
                        .join(",");
                    return Err(AliasError::MissingParam {
                        name: name.to_string(),
                        required: ph.name.clone(),
                        declared: declared_str.clone(),
                        example,
                    });
                };
                out.push_str(&value);
                cursor = ph.span.end;
                ph_iter.next();
                continue;
            }
            _ => {
                out.push(bytes[cursor] as char);
                cursor += 1;
            }
        }
    }
    Ok(out)
}

/// One parsed call site: `@name`, `@name()`, or `@name(k=v,...)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSite {
    pub name: String,
    pub params: BTreeMap<String, String>,
    /// Byte length of the matched substring in the input. Used by the
    /// LogQL alias_subst pass to advance its scanner past the match.
    pub span_len: usize,
}

/// Try to parse a call-site reference at the start of `s`. Returns
/// `None` if `s` doesn't start with `@<name>` (the caller emits the
/// `@` verbatim and continues scanning). Returns `Some(CallSite)` on
/// success — the caller should slice `s[..span_len]` as the matched
/// span and `&s[span_len..]` as the rest.
pub fn try_parse_call_site_prefix(s: &str) -> Result<Option<CallSite>, AliasError> {
    let bytes = s.as_bytes();
    if bytes.is_empty() || bytes[0] != b'@' {
        return Ok(None);
    }

    let mut j = 1;
    while j < bytes.len()
        && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_' || bytes[j] == b'-')
    {
        j += 1;
    }
    if j == 1 {
        return Ok(None);
    }

    let name = s[1..j].to_string();
    if validate_alias_name(&name).is_err() {
        return Ok(None);
    }

    let trailing = &s[j..];
    if !trailing.starts_with('(') {
        return Ok(Some(CallSite {
            name,
            params: BTreeMap::new(),
            span_len: j,
        }));
    }

    let close_rel = trailing
        .find(')')
        .ok_or_else(|| AliasError::BadCallSyntax {
            hint: format!("alias '@{name}' call site is missing ')'"),
        })?;

    let inside = &trailing[1..close_rel];
    let params = parse_param_list(&name, inside)?;
    let span_len = j + close_rel + 1;
    Ok(Some(CallSite {
        name,
        params,
        span_len,
    }))
}

fn parse_param_list(
    alias_name: &str,
    inside: &str,
) -> Result<BTreeMap<String, String>, AliasError> {
    let s = inside.trim();
    if s.is_empty() {
        return Ok(BTreeMap::new());
    }
    let pairs = split_top_level_commas(s);
    let mut params = BTreeMap::new();
    for pair in pairs {
        let (k, v) = parse_kv(alias_name, &pair)?;
        if params.contains_key(&k) {
            return Err(AliasError::BadCallSyntax {
                hint: format!("alias '@{alias_name}' got param '{k}' more than once"),
            });
        }
        params.insert(k, v);
    }
    Ok(params)
}

fn split_top_level_commas(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'"' {
            in_string = !in_string;
            current.push(c as char);
            i += 1;
            continue;
        }
        if in_string && c == b'\\' && i + 1 < bytes.len() {
            current.push(c as char);
            current.push(bytes[i + 1] as char);
            i += 2;
            continue;
        }
        if c == b',' && !in_string {
            parts.push(std::mem::take(&mut current));
            i += 1;
            continue;
        }
        current.push(c as char);
        i += 1;
    }
    parts.push(current);
    parts
}

fn parse_kv(alias_name: &str, pair: &str) -> Result<(String, String), AliasError> {
    let p = pair.trim();
    let eq = p.find('=').ok_or_else(|| AliasError::BadCallSyntax {
        hint: format!("alias '@{alias_name}' param '{p}' must be in 'key=value' form"),
    })?;
    let k = p[..eq].trim().to_string();
    let raw_v = p[eq + 1..].trim();
    if k.is_empty() {
        return Err(AliasError::BadCallSyntax {
            hint: format!("alias '@{alias_name}' has an empty param name"),
        });
    }
    if !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(AliasError::BadCallSyntax {
            hint: format!("alias '@{alias_name}' param name '{k}' must match [a-zA-Z0-9_]+"),
        });
    }

    let v = if raw_v.len() >= 2 && raw_v.starts_with('"') && raw_v.ends_with('"') {
        unescape_quoted(&raw_v[1..raw_v.len() - 1])
    } else {
        raw_v.to_string()
    };
    Ok((k, v))
}

fn unescape_quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'"' => out.push('"'),
                b'\\' => out.push('\\'),
                b'n' => out.push('\n'),
                b't' => out.push('\t'),
                b => out.push(b as char),
            }
            i += 2;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

// ===========================================================================
// Chain expansion.
// ===========================================================================

/// Recursively expand the alias `name` with the given `params`.
/// `chain` tracks the path so cycles surface as `CircularReference`
/// (a hand-edited `aliases.toml` could introduce one); `depth` enforces
/// `MAX_CHAIN_DEPTH`.
pub fn expand_recursive(
    name: &str,
    params: &BTreeMap<String, String>,
    depth: usize,
    chain: &mut Vec<String>,
) -> Result<(String, AliasKind), AliasError> {
    if depth >= MAX_CHAIN_DEPTH {
        return Err(AliasError::ChainDepthExceeded {
            chain: chain.join(" -> "),
        });
    }
    let entry = get(name)?.ok_or_else(|| AliasError::Unknown(name.to_string()))?;
    let substituted = substitute_params(&entry.selector, name, params)?;
    let final_body = expand_inner_aliases(&substituted, depth, chain)?;
    let kind = classify(&final_body);
    Ok((final_body, kind))
}

/// Walk a substituted body and recursively expand every `@other(...)`
/// reference outside of double-quoted strings. The string-context skip
/// matches the LogQL alias_subst convention so a quoted literal `"@x"`
/// is preserved verbatim.
fn expand_inner_aliases(
    body: &str,
    depth: usize,
    chain: &mut Vec<String>,
) -> Result<String, AliasError> {
    let mut out = String::with_capacity(body.len());
    let bytes = body.as_bytes();
    let mut i = 0;
    let mut in_string = false;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'"' {
            in_string = !in_string;
            out.push(c as char);
            i += 1;
            continue;
        }
        if in_string {
            if c == b'\\' && i + 1 < bytes.len() {
                out.push(c as char);
                out.push(bytes[i + 1] as char);
                i += 2;
                continue;
            }
            out.push(c as char);
            i += 1;
            continue;
        }
        if c == b'@' {
            let rest = &body[i..];
            if let Some(cs) = try_parse_call_site_prefix(rest)? {
                if chain.contains(&cs.name) {
                    let mut full = chain.clone();
                    full.push(cs.name.clone());
                    return Err(AliasError::CircularReference {
                        chain: full.join(" -> "),
                    });
                }
                chain.push(cs.name.clone());
                let (expanded, _) = expand_recursive(&cs.name, &cs.params, depth + 1, chain)?;
                chain.pop();
                out.push_str(&expanded);
                i += cs.span_len;
                continue;
            }
        }
        out.push(c as char);
        i += 1;
    }
    Ok(out)
}

// ===========================================================================
// Cycle detection at `add()` time.
// ===========================================================================

/// Return `true` if `body` contains an `@<ident>[(...)]` reference
/// outside of double-quoted strings. Used by `add()` to skip the
/// strict-parse check when the body is a chain — chain bodies rely on
/// runtime substitution and are validated by the cycle DFS instead.
fn body_contains_alias_ref(body: &str) -> bool {
    let bytes = body.as_bytes();
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'"' {
            in_string = !in_string;
            i += 1;
            continue;
        }
        if in_string {
            if c == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if c == b'@' && i + 1 < bytes.len() {
            let n = bytes[i + 1];
            if n.is_ascii_alphanumeric() || n == b'_' {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// Static targets referenced by `@<name>[(...)]` in `body`, outside of
/// double-quoted strings. Used by the cycle-detection DFS at `add()`
/// time. Param values are *not* inspected — a `@a(svc=$x)` reference
/// only adds `a` to the graph (the `$x` is not a chain edge).
fn referenced_aliases(body: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let bytes = body.as_bytes();
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'"' {
            in_string = !in_string;
            i += 1;
            continue;
        }
        if in_string {
            if c == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if c == b'@' {
            let rest = &body[i..];
            if let Ok(Some(cs)) = try_parse_call_site_prefix(rest) {
                refs.push(cs.name);
                i += cs.span_len;
                continue;
            }
        }
        i += 1;
    }
    refs
}

/// DFS the alias graph rooted at `start`, following `@other(...)`
/// references in each visited body. Returns `Some(chain)` if a cycle
/// is reachable from `start`, `None` otherwise. Missing-target
/// references (alias `@x` referencing `@y` where `y` does not exist)
/// are tolerated here — the runtime expansion will surface
/// `AliasError::Unknown` at call time, and rejecting unknowns at
/// definition time would prevent legitimate "define b before a"
/// migration ordering.
fn find_cycle(file: &AliasFile, start: &str) -> Option<Vec<String>> {
    let mut path: Vec<String> = vec![start.to_string()];
    let mut on_path: std::collections::BTreeSet<String> =
        std::iter::once(start.to_string()).collect();
    let start_entry = file.aliases.get(start)?;
    let mut stack: Vec<std::vec::IntoIter<String>> =
        vec![referenced_aliases(&start_entry.selector).into_iter()];

    while let Some(top) = stack.last_mut() {
        match top.next() {
            None => {
                let popped = path.pop();
                if let Some(p) = popped {
                    on_path.remove(&p);
                }
                stack.pop();
            }
            Some(next) => {
                if on_path.contains(&next) {
                    let mut chain = path.clone();
                    chain.push(next);
                    return Some(chain);
                }
                let next_entry = match file.aliases.get(&next) {
                    Some(e) => e,
                    None => continue,
                };
                let next_refs = referenced_aliases(&next_entry.selector);
                path.push(next.clone());
                on_path.insert(next);
                stack.push(next_refs.into_iter());
            }
        }
    }
    None
}

// ===========================================================================
// Public expansion entry points (used by selector resolve + verb dispatch).
// ===========================================================================

/// Expand a verb-style call site (`@name[(k=v,...)]`) or pass through
/// a non-alias selector. Returns `(expanded_body, classification)`.
/// Trailing text after the call site is rejected — verb selectors are
/// the entire argument.
pub fn expand(input: &str) -> Result<(String, AliasKind), AliasError> {
    let t = input.trim();
    let Some(cs) = try_parse_call_site_prefix(t)? else {
        return Ok((t.to_string(), classify(t)));
    };
    if cs.span_len != t.len() {
        return Err(AliasError::BadCallSyntax {
            hint: format!(
                "trailing text after '@{}' alias reference is not allowed in a verb selector",
                cs.name
            ),
        });
    }
    let mut chain = vec![cs.name.clone()];
    expand_recursive(&cs.name, &cs.params, 0, &mut chain)
}

/// Expand and require verb-style classification. Used by every
/// read/write verb's selector argument.
pub fn expand_for_verb(input: &str) -> Result<String, AliasError> {
    let t = input.trim();
    if t.starts_with('@') {
        let cs_opt = try_parse_call_site_prefix(t)?;
        if let Some(cs) = cs_opt.as_ref() {
            if cs.span_len != t.len() {
                return Err(AliasError::BadCallSyntax {
                    hint: format!(
                        "trailing text after '@{}' alias reference is not allowed in a verb selector",
                        cs.name
                    ),
                });
            }
            let (body, kind) = expand(t)?;
            if kind == AliasKind::LogQl {
                return Err(AliasError::LogQlInVerbContext {
                    name: cs.name.clone(),
                    suggestion: format!("{}-v", cs.name),
                });
            }
            return Ok(body);
        }
    }
    Ok(t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::MutexGuard;

    fn lock() -> MutexGuard<'static, ()> {
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
    fn refuses_unknown_alias() {
        let _h = temp_home();
        let err = expand("@ghost").unwrap_err();
        assert!(matches!(err, AliasError::Unknown(_)));
    }

    #[test]
    fn refuses_bad_verb_body() {
        let _h = temp_home();
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

    // Parameterized-alias unit tests --------------------------------------

    #[test]
    fn l3_extract_parameters_basic() {
        assert_eq!(extract_parameters("static"), Vec::<String>::new());
        assert_eq!(
            extract_parameters("{service=\"$svc\"}"),
            vec!["svc".to_string()]
        );
        assert_eq!(
            extract_parameters("$a $b $a"),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(extract_parameters("$$svc"), Vec::<String>::new());
    }

    #[test]
    fn l3_substitute_params_basic() {
        let mut p = BTreeMap::new();
        p.insert("svc".into(), "pulse".into());
        let out = substitute_params("{service=\"$svc\"}", "x", &p).unwrap();
        assert_eq!(out, "{service=\"pulse\"}");
    }

    #[test]
    fn l3_substitute_params_dollar_dollar_escape() {
        let p = BTreeMap::new();
        let out = substitute_params("price: $$svc", "x", &p).unwrap();
        assert_eq!(out, "price: $svc");
    }

    #[test]
    fn l3_substitute_params_missing_errors() {
        let p = BTreeMap::new();
        let err = substitute_params("$svc", "x", &p).unwrap_err();
        match err {
            AliasError::MissingParam { required, .. } => assert_eq!(required, "svc"),
            other => panic!("wrong variant: {:?}", other),
        }
    }

    #[test]
    fn l3_substitute_params_extra_errors() {
        let mut p = BTreeMap::new();
        p.insert("svc".into(), "pulse".into());
        p.insert("nope".into(), "x".into());
        let err = substitute_params("$svc", "x", &p).unwrap_err();
        assert!(matches!(err, AliasError::ExtraParam { .. }));
    }

    #[test]
    fn l3_call_site_bare() {
        let cs = try_parse_call_site_prefix("@plogs").unwrap().unwrap();
        assert_eq!(cs.name, "plogs");
        assert!(cs.params.is_empty());
        assert_eq!(cs.span_len, 6);
    }

    #[test]
    fn l3_call_site_empty_parens() {
        let cs = try_parse_call_site_prefix("@plogs()").unwrap().unwrap();
        assert_eq!(cs.name, "plogs");
        assert!(cs.params.is_empty());
    }

    #[test]
    fn l3_call_site_with_params() {
        let cs = try_parse_call_site_prefix("@svc-logs(svc=pulse,env=prod)")
            .unwrap()
            .unwrap();
        assert_eq!(cs.name, "svc-logs");
        assert_eq!(cs.params.get("svc").unwrap(), "pulse");
        assert_eq!(cs.params.get("env").unwrap(), "prod");
    }

    #[test]
    fn l3_call_site_quoted_value_with_comma() {
        let cs = try_parse_call_site_prefix("@a(pat=\"foo,bar\",b=2)")
            .unwrap()
            .unwrap();
        assert_eq!(cs.params.get("pat").unwrap(), "foo,bar");
        assert_eq!(cs.params.get("b").unwrap(), "2");
    }

    #[test]
    fn l3_call_site_missing_close_paren() {
        let err = try_parse_call_site_prefix("@a(svc=pulse").unwrap_err();
        assert!(matches!(err, AliasError::BadCallSyntax { .. }));
    }

    #[test]
    fn l3_chain_works_to_depth_3() {
        let _h = temp_home();
        add("svc-logs", "{service=\"$svc\"}", None, false).unwrap();
        add(
            "prod-pulse",
            "@svc-logs(svc=pulse) |= \"$pat\"",
            None,
            false,
        )
        .unwrap();
        add("prod-pulse-err", "@prod-pulse(pat=ERROR)", None, false).unwrap();
        let (out, kind) = expand("@prod-pulse-err").unwrap();
        assert_eq!(kind, AliasKind::LogQl);
        assert!(out.contains("service=\"pulse\""));
        assert!(out.contains("|= \"ERROR\""));
    }

    #[test]
    fn l3_circular_reference_rejected_at_add_time() {
        let _h = temp_home();
        add("a", "@b(x=1)", None, false).unwrap();
        // The strict-parse check is skipped for chain bodies, so this
        // passes the body-validation step. The cycle DFS catches it.
        let err = add("b", "@a(y=2)", None, false).unwrap_err();
        match err {
            AliasError::CircularReference { chain } => {
                assert!(chain.contains("a"));
                assert!(chain.contains("b"));
            }
            other => panic!("wrong variant: {:?}", other),
        }
    }

    #[test]
    fn l3_self_cycle_rejected_at_add_time() {
        let _h = temp_home();
        let err = add("a", "@a(x=1)", None, false).unwrap_err();
        assert!(matches!(err, AliasError::CircularReference { .. }));
    }

    #[test]
    fn l3_parameters_field_populated_on_add() {
        let _h = temp_home();
        add("svc-logs", "{service=\"$svc\"} |= \"$pat\"", None, false).unwrap();
        let entry = get("svc-logs").unwrap().unwrap();
        assert_eq!(
            entry.parameters.unwrap(),
            vec!["svc".to_string(), "pat".to_string()]
        );
    }

    #[test]
    fn l3_pre_l3_entry_without_parameters_field_loads() {
        let _h = temp_home();
        let path = paths::aliases_toml();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "[aliases.legacy]\nselector = \"arte/pulse\"\n").unwrap();
        paths::set_file_mode_0600(&path).unwrap();
        let entry = get("legacy").unwrap().unwrap();
        assert_eq!(entry.selector, "arte/pulse");
        assert!(entry.parameters.is_none());
    }

    #[test]
    fn l3_bare_at_name_unchanged_for_parameterless_alias() {
        let _h = temp_home();
        add("plogs", "arte/pulse", None, false).unwrap();
        assert_eq!(expand_for_verb("@plogs").unwrap(), "arte/pulse");
    }

    #[test]
    fn l3_default_value_used_when_param_omitted() {
        let p = BTreeMap::new();
        let out = substitute_params("svc=${svc:-pulse}", "x", &p).unwrap();
        assert_eq!(out, "svc=pulse");
    }

    #[test]
    fn l3_default_value_overridden_when_param_provided() {
        let mut p = BTreeMap::new();
        p.insert("svc".into(), "atlas".into());
        let out = substitute_params("svc=${svc:-pulse}", "x", &p).unwrap();
        assert_eq!(out, "svc=atlas");
    }

    #[test]
    fn l3_required_and_optional_params_in_same_body() {
        let mut p = BTreeMap::new();
        p.insert("svc".into(), "pulse".into());
        // svc is required (bare $svc), lvl is optional ($lvl with default).
        let out = substitute_params("$svc/${lvl:-INFO}", "x", &p).unwrap();
        assert_eq!(out, "pulse/INFO");
    }

    #[test]
    fn l3_required_param_missing_still_errors_when_others_have_defaults() {
        let p = BTreeMap::new();
        // $svc is required (no default); ${lvl:-INFO} is optional.
        let err = substitute_params("$svc/${lvl:-INFO}", "x", &p).unwrap_err();
        match err {
            AliasError::MissingParam { required, .. } => assert_eq!(required, "svc"),
            other => panic!("wrong variant: {:?}", other),
        }
    }

    #[test]
    fn l3_braced_form_without_default_works() {
        let mut p = BTreeMap::new();
        p.insert("svc".into(), "pulse".into());
        let out = substitute_params("${svc}-suffix", "x", &p).unwrap();
        assert_eq!(out, "pulse-suffix");
    }

    #[test]
    fn l3_default_with_escaped_brace_preserves_literal() {
        // Default value contains an escaped `}` which becomes a literal
        // `}` after substitution.
        let p = BTreeMap::new();
        let out = substitute_params("${svc:-{a\\}}", "x", &p).unwrap();
        assert_eq!(out, "{a}");
    }

    #[test]
    fn l3_extract_defaults_returns_declared_defaults() {
        let d = extract_defaults("$req-${opt:-X}-${other:-Y}");
        assert_eq!(d.get("opt").map(String::as_str), Some("X"));
        assert_eq!(d.get("other").map(String::as_str), Some("Y"));
        assert!(!d.contains_key("req"));
    }

    #[test]
    fn l3_malformed_braced_placeholder_emits_literal() {
        // `${ ` (space) is not a valid name; the scanner skips the
        // `${...` and the substituter emits the literal text. The body
        // declares no parameters in this case.
        let p = BTreeMap::new();
        let out = substitute_params("price: ${ ", "x", &p).unwrap();
        assert_eq!(out, "price: ${ ");
    }
}
