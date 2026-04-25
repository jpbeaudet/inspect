//! Namespace configuration: schema, env-var resolver, file store, and
//! merged precedence resolver.
//!
//! Precedence (highest to lowest), per the bible §4.2:
//!
//! 1. Per-namespace env vars `INSPECT_<NS>_*`
//! 2. `~/.inspect/servers.toml`
//! 3. Interactive `inspect add` (handled by the command, not here)

pub mod env;
pub mod file;
pub mod namespace;
pub mod resolver;

#[allow(unused_imports)]
pub use namespace::{NamespaceConfig, NamespaceSource, ResolvedNamespace};
