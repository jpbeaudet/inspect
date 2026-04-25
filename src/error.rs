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
#[allow(dead_code)]
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
    MissingField { namespace: String, field: &'static str },

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
