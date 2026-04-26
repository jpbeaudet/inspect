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
mod exec;
mod format;
mod logql;
mod paths;
mod profile;
mod redact;
mod safety;
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
        Command::Search(args) => commands::search::run(args),
        Command::Restart(args) => verbs::write::lifecycle::restart(args),
        Command::Stop(args) => verbs::write::lifecycle::stop(args),
        Command::Start(args) => verbs::write::lifecycle::start(args),
        Command::Reload(args) => verbs::write::lifecycle::reload(args),
        Command::Cp(args) => verbs::write::cp::run(args),
        Command::Edit(args) => verbs::write::edit::run(args),
        Command::Rm(args) => verbs::write::rm::run(args),
        Command::Mkdir(args) => verbs::write::mkdir::run(args),
        Command::Touch(args) => verbs::write::touch::run(args),
        Command::Chmod(args) => verbs::write::chmod::run(args),
        Command::Chown(args) => verbs::write::chown::run(args),
        Command::Exec(args) => verbs::write::exec::run(args),
        Command::Audit(args) => commands::audit::run(args),
        Command::Revert(args) => commands::revert::run(args),
        Command::Why(args) => commands::why::run(args),
        Command::Connectivity(args) => commands::connectivity::run(args),
        Command::Recipe(args) => commands::recipe::run(args),
        Command::Fleet(_) => commands::placeholders::run(cli.command),
    }
}
