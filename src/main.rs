//! `inspect` — operational debugging CLI.
//!
//! Phase 0 entry point: command parsing, dispatch, and exit-code mapping.

use std::process::ExitCode;

use clap::Parser;

mod cli;
mod commands;
mod config;
mod error;
mod paths;
mod redact;
mod ssh;

use cli::{Cli, Command};
use error::ExitKind;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = dispatch(cli);
    match result {
        Ok(kind) => ExitCode::from(kind.code()),
        Err(err) => {
            eprintln!("error: {err}");
            // Surface a chain of causes if available.
            let mut source = err.source();
            while let Some(cause) = source {
                eprintln!("  caused by: {cause}");
                source = cause.source();
            }
            ExitCode::from(ExitKind::Error.code())
        }
    }
}

fn dispatch(cli: Cli) -> anyhow::Result<ExitKind> {
    match cli.command {
        Command::Add(args) => commands::add::run(args),
        Command::List(args) => commands::list::run(args),
        Command::Remove(args) => commands::remove::run(args),
        Command::Test(args) => commands::test::run(args),
        Command::Show(args) => commands::show::run(args),
        Command::Connect(args) => commands::connect::run(args),
        Command::Disconnect(args) => commands::disconnect::run(args),
        Command::Connections(args) => commands::connections::run(args),
        Command::DisconnectAll(args) => commands::disconnect_all::run(args),
        Command::Setup(_)
        | Command::Discover(_)
        | Command::Status(_)
        | Command::Health(_)
        | Command::Logs(_)
        | Command::Grep(_)
        | Command::Cat(_)
        | Command::Ls(_)
        | Command::Find(_)
        | Command::Ps(_)
        | Command::Volumes(_)
        | Command::Images(_)
        | Command::Network(_)
        | Command::Ports(_)
        | Command::Why(_)
        | Command::Connectivity(_)
        | Command::Recipe(_)
        | Command::Search(_)
        | Command::Restart(_)
        | Command::Stop(_)
        | Command::Start(_)
        | Command::Reload(_)
        | Command::Cp(_)
        | Command::Edit(_)
        | Command::Rm(_)
        | Command::Mkdir(_)
        | Command::Touch(_)
        | Command::Chmod(_)
        | Command::Chown(_)
        | Command::Exec(_)
        | Command::Alias(_)
        | Command::Audit(_)
        | Command::Revert(_)
        | Command::Fleet(_) => commands::placeholders::run(cli.command),
    }
}
