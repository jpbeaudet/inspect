//! Safety contract for write verbs (bible §8.2).
//!
//! - Dry-run by default; `--apply` to execute.
//! - Atomic file edits.
//! - Audit log + content snapshots on every applied mutation.
//! - Interactive confirms for irreversibles; large-fanout interlock.
//!
//! This module is the single source of truth for those rules so each
//! write verb can reuse one implementation.

pub mod audit;
pub mod diff;
pub mod gate;
pub mod snapshot;

pub use audit::{validate_reason, AuditEntry, AuditStore};
pub use gate::{Confirm, SafetyGate};
pub use snapshot::SnapshotStore;
