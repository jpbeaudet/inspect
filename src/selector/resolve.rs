//! Selector resolution against configured namespaces and their cached
//! profiles. Produces a flat list of [`ResolvedTarget`]s ready for the
//! caller (read/write verbs) to fan out over.
//!
//! Resolution order (bible §6.3):
//! 1. container short name (exact match against `profile.services[*].name`)
//! 2. profile-level aliases (`profile.aliases`)
//! 3. profile-level groups (`profile.groups`)
//! 4. globs / regex over discovered service names
//! 5. subtractive `~` is applied last as set difference
//!
//! Empty results produce a [`SelectorError::NoMatches`] carrying everything
//! the user might have typo'd, in pre-sorted form, so the diagnostic is
//! helpful instead of a bare "no matches".

use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;
use thiserror::Error;

use super::ast::{Selector, ServerAtom, ServerSpec, ServiceAtom, ServiceSpec};
use super::parser::{parse_selector, SelectorParseError};
use crate::alias;
use crate::config::resolver as ns_resolver;
use crate::error::ConfigError;
use crate::profile::cache::load_profile;
use crate::profile::schema::{Profile, ServiceKind};

/// One concrete (namespace, target) pair after selector resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTarget {
    pub namespace: String,
    pub kind: TargetKind,
    pub path: Option<String>,
}

/// What was selected on the remote side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetKind {
    /// A container/service. `name` matches `Profile::services[*].name`.
    Service { name: String },
    /// Host-level (`_` service). No container.
    Host,
}

#[derive(Debug, Error)]
pub enum SelectorError {
    #[error(transparent)]
    Parse(#[from] SelectorParseError),

    #[error(transparent)]
    Alias(#[from] alias::AliasError),

    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error("no namespaces are configured; run 'inspect add <name>' first")]
    NoNamespacesConfigured,

    #[error(
        "selector '{selector}' matched no targets.\n  \
         servers tried: {servers}\n  \
         services available: {services}\n  \
         groups available: {groups}\n  \
         aliases available: {aliases}\n  \
         hint: run 'inspect profile <ns>' to see what was discovered, \
         or 'inspect setup <ns> --force' to refresh the cache"
    )]
    NoMatches {
        selector: String,
        servers: String,
        services: String,
        groups: String,
        aliases: String,
    },
}

/// Resolve a textual selector all the way down to concrete targets.
///
/// This performs (in order):
///   1. alias expansion (verb-style required)
///   2. parse into AST
///   3. namespace match
///   4. per-namespace service match against the cached profile
pub fn resolve(input: &str) -> Result<Vec<ResolvedTarget>, SelectorError> {
    let expanded = alias::expand_for_verb(input)?;
    let ast = parse_selector(&expanded)?;
    resolve_ast(&ast)
}

/// Resolve an already-parsed selector. Useful when the caller wants to
/// validate the shape without performing alias expansion (tests).
pub fn resolve_ast(sel: &Selector) -> Result<Vec<ResolvedTarget>, SelectorError> {
    let all_namespaces = ns_resolver::list_all()?;
    if all_namespaces.is_empty() {
        return Err(SelectorError::NoNamespacesConfigured);
    }
    let known_names: Vec<String> = all_namespaces.iter().map(|n| n.name.clone()).collect();

    // Step 1: filter namespaces.
    //
    // Phase 11 fleet override: when the private env-var pair
    // `INSPECT_INTERNAL_FLEET_FORCE_NS` + `INSPECT_INTERNAL_FLEET_PARENT_PID`
    // is set AND the parent-pid value matches our actual parent process
    // (via `getppid()` on unix), the fleet orchestrator has already
    // chosen the namespace and the user's server spec must be ignored.
    // The pid-pairing check ensures a stray exported value in a user
    // shell can't silently scope every subsequent `inspect` invocation
    // — without a matching pair we fall through to the user's selector.
    let chosen_namespaces = match fleet_forced_namespace(&known_names) {
        Some(ns) => vec![ns],
        None => match_servers(&sel.server, &known_names),
    };

    // Collect "what was available" for diagnostics.
    let mut all_services: BTreeSet<String> = BTreeSet::new();
    let mut all_groups: BTreeSet<String> = BTreeSet::new();
    let mut all_pf_aliases: BTreeSet<String> = BTreeSet::new();
    let mut profiles: BTreeMap<String, Profile> = BTreeMap::new();
    for ns in &chosen_namespaces {
        if let Some(p) = load_profile(ns).map_err(|e| {
            SelectorError::Config(ConfigError::Io {
                path: format!("profile '{ns}'"),
                source: std::io::Error::other(e.to_string()),
            })
        })? {
            for s in &p.services {
                all_services.insert(s.name.clone());
            }
            for g in p.groups.keys() {
                all_groups.insert(g.clone());
            }
            for a in p.aliases.keys() {
                all_pf_aliases.insert(a.clone());
            }
            profiles.insert(ns.clone(), p);
        }
    }

    let mut targets: Vec<ResolvedTarget> = Vec::new();
    for ns in &chosen_namespaces {
        let profile = profiles.get(ns);
        for t in resolve_services_for_ns(ns, sel, profile)? {
            targets.push(t);
        }
    }

    if targets.is_empty() {
        let global_aliases: BTreeSet<String> = alias::list()
            .unwrap_or_default()
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        let combined_aliases: BTreeSet<String> =
            all_pf_aliases.union(&global_aliases).cloned().collect();
        return Err(SelectorError::NoMatches {
            selector: sel.source.clone(),
            servers: fmt_set(&chosen_namespaces.iter().cloned().collect()),
            services: fmt_set(&all_services),
            groups: fmt_set(&all_groups),
            aliases: fmt_set(&combined_aliases),
        });
    }
    Ok(targets)
}

fn fmt_set(s: &BTreeSet<String>) -> String {
    if s.is_empty() {
        "(none)".to_string()
    } else {
        s.iter().cloned().collect::<Vec<_>>().join(", ")
    }
}

/// Internal env-var pair set by `inspect fleet` to pin selector
/// resolution to a single namespace. The names are intentionally tagged
/// `INTERNAL` so a stray export in a user shell is obviously
/// out-of-band.
const FLEET_FORCE_NS_VAR: &str = "INSPECT_INTERNAL_FLEET_FORCE_NS";
const FLEET_FORCE_PARENT_PID_VAR: &str = "INSPECT_INTERNAL_FLEET_PARENT_PID";

/// Resolve the fleet override to a forced namespace, validating both
/// the env-var pair and the parent-pid pairing. Returns `Some(ns)` only
/// when every check passes AND the namespace is configured.
fn fleet_forced_namespace(known: &[String]) -> Option<String> {
    let forced = std::env::var(FLEET_FORCE_NS_VAR)
        .ok()
        .filter(|s| !s.is_empty())?;
    let claimed_pid: u32 = std::env::var(FLEET_FORCE_PARENT_PID_VAR)
        .ok()?
        .parse()
        .ok()?;
    if !ppid_matches(claimed_pid) {
        return None;
    }
    if known.iter().any(|n| n == &forced) {
        Some(forced)
    } else {
        None
    }
}

#[cfg(unix)]
fn ppid_matches(claimed: u32) -> bool {
    // Safe: getppid is async-signal-safe and has no preconditions.
    let actual = unsafe { libc::getppid() };
    actual >= 0 && (actual as u32) == claimed
}

#[cfg(not(unix))]
fn ppid_matches(_claimed: u32) -> bool {
    // Without a portable parent-pid syscall, fall back to "rename is
    // sufficient mitigation" — honor the override unconditionally on
    // non-unix.
    true
}

/// Match the server-spec against the configured namespace set.
fn match_servers(spec: &ServerSpec, all: &[String]) -> Vec<String> {
    match spec {
        ServerSpec::All => all.to_vec(),
        ServerSpec::Atoms(atoms) => {
            let mut included: BTreeSet<String> = BTreeSet::new();
            let mut excluded: BTreeSet<String> = BTreeSet::new();
            let mut had_positive = false;
            for atom in atoms {
                match atom {
                    ServerAtom::Pattern(p) => {
                        had_positive = true;
                        for name in all {
                            if pattern_matches(p, name) {
                                included.insert(name.clone());
                            }
                        }
                    }
                    ServerAtom::Exclude(p) => {
                        for name in all {
                            if pattern_matches(p, name) {
                                excluded.insert(name.clone());
                            }
                        }
                    }
                }
            }
            // If only subtractive atoms were given, treat as `all - excludes`.
            if !had_positive {
                included = all.iter().cloned().collect();
            }
            included.difference(&excluded).cloned().collect()
        }
    }
}

fn resolve_services_for_ns(
    ns: &str,
    sel: &Selector,
    profile: Option<&Profile>,
) -> Result<Vec<ResolvedTarget>, SelectorError> {
    let path = sel.path.as_ref().map(|p| p.0.clone());

    match &sel.service {
        // No service portion: treat as host-level by default. The verb
        // layer can still re-interpret this for verbs that fan out across
        // all services (e.g. `status arte` with no service portion).
        None => Ok(vec![ResolvedTarget {
            namespace: ns.to_string(),
            kind: TargetKind::Host,
            path: path.clone(),
        }]),
        Some(ServiceSpec::Host) => Ok(vec![ResolvedTarget {
            namespace: ns.to_string(),
            kind: TargetKind::Host,
            path: path.clone(),
        }]),
        Some(ServiceSpec::All) => {
            let mut out = Vec::new();
            if let Some(p) = profile {
                for s in &p.services {
                    if matches!(s.kind, ServiceKind::Container | ServiceKind::Systemd) {
                        out.push(ResolvedTarget {
                            namespace: ns.to_string(),
                            kind: TargetKind::Service { name: s.name.clone() },
                            path: path.clone(),
                        });
                    }
                }
            }
            Ok(out)
        }
        Some(ServiceSpec::Atoms(atoms)) => {
            let names: Vec<String> = profile
                .map(|p| p.services.iter().map(|s| s.name.clone()).collect())
                .unwrap_or_default();
            let groups: BTreeMap<String, Vec<String>> = profile
                .map(|p| p.groups.clone())
                .unwrap_or_default();
            let pf_aliases: BTreeMap<String, String> = profile
                .map(|p| p.aliases.clone())
                .unwrap_or_default();

            let mut included: BTreeSet<String> = BTreeSet::new();
            let mut excluded: BTreeSet<String> = BTreeSet::new();
            let mut had_positive = false;

            for atom in atoms {
                match atom {
                    ServiceAtom::Pattern(p) => {
                        had_positive = true;
                        // 1) exact short-name match.
                        if names.iter().any(|n| n == p) {
                            included.insert(p.clone());
                            continue;
                        }
                        // 2) profile alias (single-level: alias body must be
                        //    a plain service name).
                        if let Some(target) = pf_aliases.get(p) {
                            if names.iter().any(|n| n == target) {
                                included.insert(target.clone());
                                continue;
                            }
                        }
                        // 3) group expansion.
                        if let Some(members) = groups.get(p) {
                            for m in members {
                                if names.iter().any(|n| n == m) {
                                    included.insert(m.clone());
                                }
                            }
                            continue;
                        }
                        // 4) glob.
                        for n in &names {
                            if pattern_matches(p, n) {
                                included.insert(n.clone());
                            }
                        }
                    }
                    ServiceAtom::Regex(body) => {
                        had_positive = true;
                        let re = Regex::new(body).map_err(|e| {
                            SelectorError::Parse(SelectorParseError::InvalidChar {
                                src: body.clone(),
                                ch: '?',
                                pos: e.to_string().len(),
                            })
                        })?;
                        for n in &names {
                            if re.is_match(n) {
                                included.insert(n.clone());
                            }
                        }
                    }
                    ServiceAtom::Exclude(p) => {
                        for n in &names {
                            if pattern_matches(p, n) {
                                excluded.insert(n.clone());
                            }
                        }
                        if let Some(members) = groups.get(p) {
                            for m in members {
                                excluded.insert(m.clone());
                            }
                        }
                    }
                }
            }
            if !had_positive {
                included = names.iter().cloned().collect();
            }
            let final_names: BTreeSet<String> =
                included.difference(&excluded).cloned().collect();
            Ok(final_names
                .into_iter()
                .map(|name| ResolvedTarget {
                    namespace: ns.to_string(),
                    kind: TargetKind::Service { name },
                    path: path.clone(),
                })
                .collect())
        }
    }
}

/// Glob-style match: `*`, `?`, `[...]`. Plain strings match exactly.
fn pattern_matches(pat: &str, name: &str) -> bool {
    if !pat.contains(['*', '?', '[']) {
        return pat == name;
    }
    let re_str = glob_to_regex(pat);
    Regex::new(&re_str).map(|r| r.is_match(name)).unwrap_or(false)
}

/// Translate a shell-style glob to an anchored regex.
fn glob_to_regex(pat: &str) -> String {
    let mut out = String::with_capacity(pat.len() + 4);
    out.push('^');
    let mut chars = pat.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            '[' => {
                out.push('[');
                for cc in chars.by_ref() {
                    out.push(cc);
                    if cc == ']' {
                        break;
                    }
                }
            }
            // Regex metacharacters that need escaping.
            '.' | '+' | '(' | ')' | '|' | '^' | '$' | '{' | '}' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out.push('$');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_translation() {
        assert!(pattern_matches("prod-*", "prod-1"));
        assert!(pattern_matches("prod-*", "prod-foo"));
        assert!(!pattern_matches("prod-*", "stagingprod"));
        assert!(pattern_matches("p?lse", "pulse"));
        assert!(pattern_matches("a.b", "a.b"));
        assert!(!pattern_matches("a.b", "axb"));
    }

    #[test]
    fn server_match_set_subtraction() {
        let all = vec!["arte".to_string(), "prod".to_string(), "staging".to_string()];
        let spec = ServerSpec::Atoms(vec![ServerAtom::Exclude("staging".to_string())]);
        let mut got = match_servers(&spec, &all);
        got.sort();
        assert_eq!(got, vec!["arte".to_string(), "prod".to_string()]);
    }

    #[test]
    fn server_match_glob() {
        let all = vec!["prod-1".to_string(), "prod-2".to_string(), "staging".to_string()];
        let spec = ServerSpec::Atoms(vec![ServerAtom::Pattern("prod-*".to_string())]);
        let mut got = match_servers(&spec, &all);
        got.sort();
        assert_eq!(got, vec!["prod-1".to_string(), "prod-2".to_string()]);
    }
}
