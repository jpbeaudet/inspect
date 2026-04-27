//! YAML-driven multi-step orchestration (B9, v0.1.2).
//!
//! See [`exec::plan`] / [`exec::apply`] for the public entry points.
//! See `docs/MANUAL.md` (or `inspect help bundle`) for YAML schema
//! reference.

pub mod checks;
pub mod exec;
pub mod schema;
pub mod vars;

pub use exec::{apply, plan, ApplyOpts};
pub use schema::Bundle;
