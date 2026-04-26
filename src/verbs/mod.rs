//! Tier 1 read verbs (bible §7).
//!
//! Each verb resolves the selector, fans out across matched targets, runs
//! a small remote command via [`crate::ssh::exec`], and emits either a
//! human SUMMARY/DATA/NEXT block or a stable line-delimited JSON record
//! stream. All verbs honor the bible's exit-code contract:
//!
//! - `0` — success (matches found, or success-by-design verbs)
//! - `1` — no matches (search-shaped verbs only: `grep`, `find`)
//! - `2` — error (any failure path)

pub mod cat;
pub mod correlation;
pub mod dispatch;
pub mod duration;
pub mod find;
pub mod grep;
pub mod health;
pub mod images;
pub mod logs;
pub mod ls;
pub mod network;
pub mod output;
pub mod ports;
pub mod ps;
pub mod quote;
pub mod runtime;
pub mod status;
pub mod volumes;
pub mod write;

#[allow(unused_imports)]
pub use output::{Envelope, JsonOut, Renderer};
#[allow(unused_imports)]
pub use runtime::RemoteRunner;
