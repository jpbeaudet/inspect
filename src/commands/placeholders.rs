//! Stubs for verbs whose real implementation lands in later phases.
//!
//! As of Phase 11 every verb is wired to a real handler; this module is
//! kept around as a forward-compatible scaffold for future additions.

use crate::cli::Command;
use crate::error::ExitKind;

#[allow(dead_code)]
pub fn run(cmd: Command) -> anyhow::Result<ExitKind> {
    let (verb, phase) = describe(&cmd);
    println!("SUMMARY: '{verb}' is not implemented yet (scheduled for {phase})");
    println!("DATA:    (none)");
    println!("NEXT:    track progress in IMPLEMENTATION_PLAN.md");
    Ok(ExitKind::Error)
}

#[allow(dead_code)]
fn describe(_cmd: &Command) -> (&'static str, &'static str) {
    ("(internal)", "implemented")
}
