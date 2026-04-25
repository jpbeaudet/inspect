//! Universal selector grammar (bible §6).
//!
//! ```text
//! <selector> ::= <server-spec> [ "/" <service-spec> ] [ ":" <path-spec> ]
//!             |  "@" <alias-name>
//! ```
//!
//! Phase 3 ships:
//! - parsing (`parse_selector`) into an [`AST`]
//! - alias-aware pre-parse expansion (`expand_aliases`)
//! - resolution against the configured namespaces and their cached profiles
//!   (`resolve`)
//! - friendly empty-result diagnostics (`SelectorError::NoMatches`)
//!
//! Selectors here are the *verb-style* form (e.g. `arte/atlas:/etc/x`).
//! LogQL-style selectors (`{server=...}`) are detected and rejected with a
//! pointer to the LogQL search command.

pub mod ast;
pub mod parser;
pub mod resolve;

#[allow(unused_imports)]
pub use ast::{PathSpec, Selector, ServerSpec, ServiceSpec};
#[allow(unused_imports)]
pub use parser::{parse_selector, SelectorParseError};
#[allow(unused_imports)]
pub use resolve::{resolve, ResolvedTarget, SelectorError};
