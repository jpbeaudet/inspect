//! Process / OS level helpers (ulimit checks, etc.).
//!
//! Kept tiny and dependency-free so the rest of the crate stays
//! testable on any host (the helpers here all degrade to "couldn't
//! determine, assume unlimited" when the underlying syscall fails).

pub mod ulimit;
