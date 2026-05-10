//! Auto-discovery engine.
//!
//! Each probe is best-effort and emits a partial profile fragment plus
//! warnings. Missing tools or denied permissions degrade gracefully (bible
//! §5.3). The engine merges fragments into a single `Profile` and persists
//! it via the cache layer.

pub mod drift;
pub mod engine;
pub mod ports_parse;
pub mod probes;
pub mod ssh_precheck;

#[allow(unused_imports)]
pub use engine::{discover, DiscoverOptions};
