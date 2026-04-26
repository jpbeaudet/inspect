//! Write verbs (bible §8). Every verb routes through [`crate::safety`] so
//! the dry-run / `--apply` / audit-log / interlock contract is enforced
//! consistently. A verb implementation focuses on:
//!
//! 1. building the remote command(s) for each resolved target,
//! 2. producing a preview block for dry-run,
//! 3. recording an [`crate::safety::AuditEntry`] on apply.

pub mod chmod;
pub mod chown;
pub mod cp;
pub mod edit;
pub mod exec;
pub mod lifecycle; // restart / stop / start / reload
pub mod mkdir;
pub mod rm;
pub mod touch;
