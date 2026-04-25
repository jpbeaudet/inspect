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
        Command::Search(_) => ("search", "Phases 6/7"),
        Command::Restart(_) => ("restart", "Phase 5"),
        Command::Stop(_) => ("stop", "Phase 5"),
        Command::Start(_) => ("start", "Phase 5"),
        Command::Reload(_) => ("reload", "Phase 5"),
        Command::Cp(_) => ("cp", "Phase 5"),
        Command::Edit(_) => ("edit", "Phase 5"),
        Command::Rm(_) => ("rm", "Phase 5"),
        Command::Mkdir(_) => ("mkdir", "Phase 5"),
        Command::Touch(_) => ("touch", "Phase 5"),
        Command::Chmod(_) => ("chmod", "Phase 5"),
        Command::Chown(_) => ("chown", "Phase 5"),
        Command::Exec(_) => ("exec", "Phase 5"),
        Command::Audit(_) => ("audit", "Phase 5"),
        Command::Revert(_) => ("revert", "Phase 5"),
        Command::Fleet(_) => ("fleet", "Phase 11"),
        _ => ("(internal)", "implemented"),
    }
}
