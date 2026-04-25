//! Stubs for verbs whose real implementation lands in later phases.
//!
//! Returns a structured "not implemented" message that follows the
//! SUMMARY/DATA/NEXT contract, so consumers can already pin against the
//! eventual surface.

use crate::cli::Command;
use crate::error::ExitKind;

pub fn run(cmd: Command) -> anyhow::Result<ExitKind> {
    let (verb, phase) = describe(&cmd);
    println!("SUMMARY: '{verb}' is not implemented in Phase 0");
    println!("DATA:    scheduled for {phase}");
    println!("NEXT:    track progress in IMPLEMENTATION_PLAN.md");
    Ok(ExitKind::Error)
}

fn describe(cmd: &Command) -> (&'static str, &'static str) {
    match cmd {
        Command::Status(_) => ("status", "Phase 4"),
        Command::Health(_) => ("health", "Phase 4"),
        Command::Logs(_) => ("logs", "Phase 4"),
        Command::Grep(_) => ("grep", "Phase 4"),
        Command::Cat(_) => ("cat", "Phase 4"),
        Command::Ls(_) => ("ls", "Phase 4"),
        Command::Find(_) => ("find", "Phase 4"),
        Command::Ps(_) => ("ps", "Phase 4"),
        Command::Volumes(_) => ("volumes", "Phase 4"),
        Command::Images(_) => ("images", "Phase 4"),
        Command::Network(_) => ("network", "Phase 4"),
        Command::Ports(_) => ("ports", "Phase 4"),
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
        // Phase 0/1/2/3 verbs should never be routed here.
        Command::Add(_)
        | Command::List(_)
        | Command::Remove(_)
        | Command::Test(_)
        | Command::Show(_)
        | Command::Connect(_)
        | Command::Disconnect(_)
        | Command::Connections(_)
        | Command::DisconnectAll(_)
        | Command::Setup(_)
        | Command::Discover(_)
        | Command::Profile(_)
        | Command::Alias(_)
        | Command::Resolve(_) => ("(internal)", "Phases 0/1/2/3"),
    }
}
