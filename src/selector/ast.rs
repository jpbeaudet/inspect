//! Selector AST.

/// A parsed selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selector {
    pub server: ServerSpec,
    pub service: Option<ServiceSpec>,
    pub path: Option<PathSpec>,
    /// Original textual form (post alias expansion). Useful for diagnostics
    /// and audit logs.
    pub source: String,
}

/// Server side: which namespace(s) this selector targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerSpec {
    /// Bible: `all` keyword. Targets every configured namespace.
    All,
    /// One or more atoms (names / globs / subtractive `~name`).
    Atoms(Vec<ServerAtom>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerAtom {
    /// Plain name or shell-style glob containing `*` / `?` / `[...]`.
    Pattern(String),
    /// Subtractive: `~prod` or `~prod-*`.
    Exclude(String),
}

/// Service side: which services on each matched server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceSpec {
    /// `_` — host-level operations (no container).
    Host,
    /// `*` — every service.
    All,
    /// One or more atoms.
    Atoms(Vec<ServiceAtom>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceAtom {
    /// `pulse`, `pulse-*`, `pulse?`.
    Pattern(String),
    /// `/milvus-\d+/` — slashes are part of the syntax, not the regex.
    Regex(String),
    /// `~synapse`.
    Exclude(String),
}

/// `:` after the service portion. We keep it as an opaque string; actual
/// path semantics live in the read/write verb engines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathSpec(pub String);
