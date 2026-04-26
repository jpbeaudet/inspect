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
mod sys;
mod verbs;

use cli::{Cli, Command};
use error::ExitKind;

fn main() -> ExitCode {
    // Install SIGINT/SIGTERM handlers as the very first thing — every
    // long-running loop in the engine and the SSH poller cooperates on
    // the global cancel flag (see `exec::cancel`).
    exec::cancel::install_handlers();

    let cli = Cli::parse();
    let result = dispatch(cli);
    // Cancellation (audit §2.2): regardless of which dispatch arm
    // returned, a tripped flag means SIGINT/SIGTERM arrived. Map to
    // the conventional shell code 130 (= 128 + SIGINT) so wrappers
    // and CI runners can detect it.
    if exec::cancel::is_cancelled() {
        if let Err(ref err) = result {
            // Print only if the inner layer hasn't already rendered an
            // envelope on stdout. We can't tell here, so be terse:
            // a single line on stderr is the worst case for scripts.
            let msg = err.to_string();
            if !msg.contains("cancelled") {
                eprintln!("inspect: cancelled by signal");
            }
        } else {
            // Success path: the verb finished its work but the user
            // still pressed Ctrl+C. Honor their intent and return 130
            // anyway — that's what `git`, `kubectl`, and `ssh` do.
            // No extra output: the verb already wrote its envelope.
        }
        return ExitCode::from(130);
    }
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
        Command::Fleet(args) => commands::fleet::run(args),
    }
}
