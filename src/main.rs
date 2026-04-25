//! `inspect` — operational debugging CLI.
//!
//! Phase 0 entry point: command parsing, dispatch, and exit-code mapping.

use std::process::ExitCode;

use clap::Parser;

mod alias;
mod cli;
mod commands;
mod config;
mod discovery;
mod error;
mod paths;
mod profile;
mod redact;
mod selector;
mod ssh;
mod verbs;

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
        Command::Setup(args) | Command::Discover(args) => commands::setup::run(args),
        Command::Profile(args) => commands::profile::run(args),
        Command::Alias(args) => commands::alias::run(args),
        Command::Resolve(args) => commands::resolve::run(args),
        Command::Status(args) => verbs::status::run(args),
        Command::Health(args) => verbs::health::run(args),
        Command::Logs(args) => verbs::logs::run(args),
        Command::Grep(args) => verbs::grep::run(args),
        Command::Cat(args) => verbs::cat::run(args),
        Command::Ls(args) => verbs::ls::run(args),
        Command::Find(args) => verbs::find::run(args),
        Command::Ps(args) => verbs::ps::run(args),
        Command::Volumes(args) => verbs::volumes::run(args),
        Command::Images(args) => verbs::images::run(args),
        Command::Network(args) => verbs::network::run(args),
        Command::Ports(args) => verbs::ports::run(args),
        Command::Why(_)
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
        | Command::Audit(_)
        | Command::Revert(_)
        | Command::Fleet(_) => commands::placeholders::run(cli.command),
    }
}
