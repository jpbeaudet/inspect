//! Stubs for verbs whose real implementation lands in later phases.

use crate::cli::Command;
use crate::error::ExitKind;

pub fn run(cmd: Command) -> anyhow::Result<ExitKind> {
    let (verb, phase) = describe(&cmd);
    println!("SUMMARY: '{verb}' is not implemented yet (scheduled for {phase})");
    println!("DATA:    (none)");
    println!("NEXT:    track progress in IMPLEMENTATION_PLAN.md");
    Ok(ExitKind::Error)
}

fn describe(cmd: &Command) -> (&'static str, &'static str) {
    match cmd {
        Command::Why(_) => ("why", "Phase 9"),
        Command::Connectivity(_) => ("connectivity", "Phase 9"),
        Command::Recipe(_) => ("recipe", "Phase 9"),
        Command::Fleet(_) => ("fleet", "Phase 11"),
        _ => ("(internal)", "implemented"),
    }
}
