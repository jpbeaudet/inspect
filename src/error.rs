//! Error and exit-code taxonomy.
//!
//! The bible mandates the following exit-code contract:
//!
//! ```text
//! 0 = success / dry-run
//! 1 = no matches (search/grep)
//! 2 = error
//! ```

use thiserror::Error;

/// Logical exit kinds that map to documented exit codes.
#[derive(Debug, Clone, Copy)]
pub enum ExitKind {
    Success,
    NoMatches,
    Error,
}

impl ExitKind {
    pub fn code(self) -> u8 {
        match self {
            ExitKind::Success => 0,
            ExitKind::NoMatches => 1,
            ExitKind::Error => 2,
        }
    }
}

/// Errors raised by the namespace configuration subsystem.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("invalid namespace name '{0}': must match [a-z0-9][a-z0-9_-]{{0,62}}")]
    InvalidNamespaceName(String),

    #[error("namespace '{0}' is not configured")]
    UnknownNamespace(String),

    #[error("namespace '{0}' already exists; pass --force to overwrite")]
    NamespaceExists(String),

    #[error("missing required field '{field}' for namespace '{namespace}'")]
    MissingField {
        namespace: String,
        field: &'static str,
    },

    #[error("config file '{path}' has unsafe permissions {mode:o}; expected 0600")]
    UnsafePermissions { path: String, mode: u32 },

    #[error("conflicting key sources: only one of key_path or key_inline may be set")]
    ConflictingKeySources,

    #[error("config IO error at '{path}': {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("config parse error at '{path}': {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },

    #[error("config serialize error: {0}")]
    Serialize(#[from] toml::ser::Error),
}

// ===========================================================================
// HP-5 — error → help linkage.
//
// Every user-facing error message ("error: …") is routed through [`emit`] so
// the renderer can append a single `see: inspect help <topic>` cross-link.
//
// The mapping lives in [`ERROR_CATALOG`]: a stable, ordered list of
// `(code, summary, help_topic)` rows. The catalog is the single source of
// truth shared by:
//   * [`topic_for_message`]   — substring match used by [`emit`] when the
//                                caller doesn't supply an explicit topic;
//   * [`crate::help::json`]   — surfaces it under `errors[]` (HP-4);
//   * [`tests/help_contract`] — guards that every catalog row is reachable
//                                and every error site uses [`emit`].
//
// The plan §7 mapping table is the canonical one. New error sites must
// either:
//   1. match an existing fragment in [`ERROR_CATALOG`] (preferred — keeps
//      the message wording uniform), or
//   2. add a new row with an explicit topic (or `None` for programmer
//      errors that have no useful topic).
//
// The CI guard in `tests/help_contract.rs::no_raw_error_eprintln` enforces
// that no `eprintln!("error: …")` survives outside this module; every site
// must funnel through `emit` so the cross-link is consistent.
// ===========================================================================

/// One row of the error catalog.
#[derive(Debug, Clone, Copy)]
pub struct ErrorEntry {
    /// Stable, machine-readable code. Used by `--json`.
    pub code: &'static str,
    /// Substring that must appear in the message at any of the call
    /// sites that should resolve to `help_topic`. The match is plain
    /// `contains()` — order in the catalog matters: more-specific rows
    /// must precede more-generic ones.
    pub fragment: &'static str,
    /// One-line description for `--json`. Not printed at runtime.
    pub summary: &'static str,
    /// The help topic that explains this error. `None` for programmer
    /// errors that have no useful user-facing topic.
    pub help_topic: Option<&'static str>,
}

/// Canonical error → topic catalog.
///
/// Order is significant: the more specific fragments come first because
/// [`topic_for_message`] returns the first match.
pub static ERROR_CATALOG: &[ErrorEntry] = &[
    // ---- selectors ----------------------------------------------------
    ErrorEntry {
        code: "EmptyTargets",
        fragment: "matched no",
        summary: "selector did not resolve to any targets",
        help_topic: Some("selectors"),
    },
    ErrorEntry {
        code: "RequiresPath",
        fragment: "requires a :path",
        summary: "verb requires a `:path` suffix on its selector",
        help_topic: Some("selectors"),
    },
    ErrorEntry {
        code: "BadSelectorGrammar",
        fragment: "selector grammar",
        summary: "selector failed to parse",
        help_topic: Some("selectors"),
    },
    // ---- aliases ------------------------------------------------------
    ErrorEntry {
        code: "UnknownAlias",
        fragment: "unknown alias",
        summary: "alias name is not registered",
        help_topic: Some("aliases"),
    },
    ErrorEntry {
        code: "BadAliasType",
        fragment: "is a LogQL selector, not a verb selector",
        summary: "verb received a LogQL alias (or vice versa)",
        help_topic: Some("aliases"),
    },
    // ---- search / LogQL ----------------------------------------------
    ErrorEntry {
        code: "EmptyQuery",
        fragment: "empty query",
        summary: "search query is empty",
        help_topic: Some("search"),
    },
    ErrorEntry {
        code: "BadLogQL",
        fragment: "logql",
        summary: "LogQL query failed to parse",
        help_topic: Some("search"),
    },
    // ---- formats ------------------------------------------------------
    ErrorEntry {
        code: "MutuallyExclusiveFormat",
        fragment: "mutually exclusive",
        summary: "more than one output format flag was set",
        help_topic: Some("formats"),
    },
    // ---- write --------------------------------------------------------
    ErrorEntry {
        code: "CpRemoteRemote",
        fragment: "remote→remote",
        summary: "cp does not support remote→remote in v1",
        help_topic: Some("write"),
    },
    ErrorEntry {
        code: "CpNeedsRemote",
        fragment: "needs at least one remote endpoint",
        summary: "cp invocation has no remote endpoint",
        help_topic: Some("write"),
    },
    ErrorEntry {
        code: "ExecMissingCommand",
        fragment: "exec requires a command",
        summary: "exec was invoked without a command after `--`",
        help_topic: Some("write"),
    },
    ErrorEntry {
        code: "MissingApply",
        fragment: "--apply",
        summary: "mutating verb requires `--apply` to execute",
        help_topic: Some("write"),
    },
    // ---- safety -------------------------------------------------------
    ErrorEntry {
        code: "AuditEntryNotFound",
        fragment: "audit entry",
        summary: "no audit row matches the given id",
        help_topic: Some("safety"),
    },
    ErrorEntry {
        code: "AuditNoPath",
        fragment: "audit selector",
        summary: "audit row has no path to revert",
        help_topic: Some("safety"),
    },
    ErrorEntry {
        code: "RevertHashMismatch",
        fragment: "hash mismatch",
        summary: "snapshot hash does not match current state",
        help_topic: Some("safety"),
    },
    // ---- discovery / namespaces --------------------------------------
    ErrorEntry {
        code: "NamespaceNotConfigured",
        fragment: "is not configured",
        summary: "namespace has no configured credentials",
        help_topic: Some("discovery"),
    },
    ErrorEntry {
        code: "NoNamespacesConfigured",
        fragment: "no namespaces are configured",
        summary: "no namespaces have been registered yet",
        help_topic: Some("quickstart"),
    },
    ErrorEntry {
        code: "NamespaceExists",
        fragment: "already exists",
        summary: "namespace exists; pass --force to overwrite",
        help_topic: Some("discovery"),
    },
    ErrorEntry {
        code: "InvalidNamespaceName",
        fragment: "invalid namespace name",
        summary: "namespace name does not match the [a-z0-9][a-z0-9_-]* shape",
        help_topic: Some("discovery"),
    },
    // ---- ssh ----------------------------------------------------------
    ErrorEntry {
        code: "SshConnectFailed",
        fragment: "ssh",
        summary: "ssh handshake failed",
        help_topic: Some("ssh"),
    },
    ErrorEntry {
        code: "MaxSessionsExceeded",
        fragment: "MaxSessions",
        summary: "ssh MaxSessions cap reached",
        help_topic: Some("ssh"),
    },
    // ---- recipes ------------------------------------------------------
    ErrorEntry {
        code: "RecipeNotFound",
        fragment: "recipe",
        summary: "named recipe is not registered",
        help_topic: Some("recipes"),
    },
    // ---- help ---------------------------------------------------------
    ErrorEntry {
        code: "UnknownHelpTopic",
        fragment: "unknown help topic",
        summary: "help topic is not registered",
        help_topic: Some("examples"),
    },
];

/// Look up the help topic for a free-form error message via substring
/// match against [`ERROR_CATALOG`]. Returns `None` if no row matches.
pub fn topic_for_message(msg: &str) -> Option<&'static str> {
    let lower = msg.to_ascii_lowercase();
    for e in ERROR_CATALOG {
        if lower.contains(&e.fragment.to_ascii_lowercase()) {
            return e.help_topic;
        }
    }
    None
}

/// Print a user-facing error to stderr in the canonical shape:
///
/// ```text
/// error: <msg>
///   see: inspect help <topic>
/// ```
///
/// The `see:` line is appended only when [`topic_for_message`] returns
/// `Some`. Programmer-only errors (no row in the catalog) get the bare
/// `error:` line, matching the HP-0 baseline.
pub fn emit(msg: impl AsRef<str>) {
    let msg = msg.as_ref();
    eprintln!("error: {msg}");
    if let Some(topic) = topic_for_message(msg) {
        eprintln!("  see: inspect help {topic}");
    }
}

#[cfg(test)]
mod hp5_tests {
    use super::*;
    use crate::help::topics::TOPICS;

    #[test]
    fn every_catalog_row_points_at_a_real_topic_or_none() {
        let known: Vec<&'static str> = TOPICS.iter().map(|t| t.id).collect();
        for e in ERROR_CATALOG {
            if let Some(t) = e.help_topic {
                assert!(
                    known.contains(&t),
                    "ERROR_CATALOG row {:?} points at unknown topic {:?}",
                    e.code,
                    t
                );
            }
        }
    }

    #[test]
    fn catalog_codes_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for e in ERROR_CATALOG {
            assert!(seen.insert(e.code), "duplicate catalog code: {}", e.code);
        }
    }

    #[test]
    fn topic_for_message_resolves_known_fragments() {
        assert_eq!(
            topic_for_message("'arte/x' matched no targets"),
            Some("selectors")
        );
        assert_eq!(
            topic_for_message("rm requires a :path on selector"),
            Some("selectors")
        );
        assert_eq!(topic_for_message("empty query"), Some("search"));
        assert_eq!(
            topic_for_message("no audit entry matches id prefix 'abc'"),
            Some("safety")
        );
        assert_eq!(
            topic_for_message("unknown help topic 'foo'"),
            Some("examples")
        );
        assert_eq!(topic_for_message("totally unrelated message"), None);
    }
}
