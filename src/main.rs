//! `inspect` — operational debugging CLI.
//!
//! Phase 0 entry point: command parsing, dispatch, and exit-code mapping.

use std::process::ExitCode;

use clap::Parser;

mod alias;
mod bundle;
mod cli;
mod commands;
mod config;
mod discovery;
mod error;
mod exec;
mod format;
mod help;
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

    // F3 (v0.1.3): `inspect help <verb>` is a synonym for
    // `inspect <verb> --help`. We rewrite argv *before* clap parsing
    // so the rendered output is byte-for-byte identical (clap renders
    // `--help` from the live subcommand tree; `render_long_help` on
    // a found subcommand omits the `inspect` bin prefix and the
    // `-V, --version` row, so they would drift). Editorial topics
    // win over verbs (e.g. `inspect help search` shows the search
    // topic, not `search --help`); only when the token is a verb
    // *and* not a topic do we rewrite.
    let raw: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let rewritten = rewrite_help_synonym(raw);
    let cli = Cli::parse_from(rewritten);
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
            // HP-5: route the top-level error through the central
            // helper so the `see: inspect help <topic>` line is
            // attached automatically whenever the message matches a
            // catalog fragment. Cause chain still rendered verbatim.
            error::emit(err.to_string());
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
        Command::Run(args) => verbs::run::run(args),
        Command::Watch(args) => verbs::watch::run(args),
        Command::Audit(args) => commands::audit::run(args),
        Command::Revert(args) => commands::revert::run(args),
        Command::Cache(args) => commands::cache::run(args),
        Command::Why(args) => commands::why::run(args),
        Command::Connectivity(args) => commands::connectivity::run(args),
        Command::Recipe(args) => commands::recipe::run(args),
        Command::Fleet(args) => commands::fleet::run(args),
        Command::Bundle(args) => commands::bundle::run(args),
        Command::Help(args) => commands::help::run(args),
    }
}

/// F3 (v0.1.3): rewrite `inspect help <verb> [extra...]` to
/// `inspect <verb> --help` when `<verb>` is a known top-level
/// subcommand AND is *not* an editorial help topic. Editorial topics
/// keep precedence (`inspect help search` → search topic page; only
/// `inspect search --help` shows clap's flag list). This rewrite
/// happens before clap parsing so the output is byte-for-byte
/// identical to `inspect <verb> --help`.
///
/// All other argv shapes are returned unchanged. In particular:
///
/// * `inspect help` (no token) — unchanged; renders the index.
/// * `inspect help <topic>` where the token is a topic — unchanged;
///   `commands::help::run` renders the editorial body.
/// * `inspect help <unknown>` — unchanged; `commands::help::run`
///   exits with `error: unknown command or topic: <name>` (F3).
fn rewrite_help_synonym(raw: Vec<std::ffi::OsString>) -> Vec<std::ffi::OsString> {
    if raw.len() < 3 {
        return raw;
    }
    if raw[1].as_os_str() != std::ffi::OsStr::new("help") {
        return raw;
    }
    let token = match raw[2].to_str() {
        Some(s) if !s.starts_with('-') => s,
        _ => return raw,
    };
    // Editorial topic wins over verb synonym.
    if help::topics::find(token).is_some() {
        return raw;
    }
    if !is_known_top_level_verb(token) {
        return raw;
    }
    // Build `inspect <verb> --help [extra...]`. Anything after the
    // verb position (typos, unrecognized flags) flows through clap's
    // own error machinery — we do not silently drop it.
    let mut out = Vec::with_capacity(raw.len() + 1);
    out.push(raw[0].clone());
    out.push(raw[2].clone());
    out.push(std::ffi::OsString::from("--help"));
    out.extend(raw.into_iter().skip(3));
    out
}

/// Returns `true` when `name` matches a top-level subcommand declared
/// on the clap tree. Used by [`rewrite_help_synonym`] to decide
/// whether `inspect help <name>` is a verb synonym or a help-topic /
/// unknown-token path.
fn is_known_top_level_verb(name: &str) -> bool {
    use clap::CommandFactory;
    let cmd = Cli::command();
    let hit = cmd
        .get_subcommands()
        .any(|s| s.get_name() == name || s.get_all_aliases().any(|a| a == name));
    hit
}

#[cfg(test)]
mod f3_tests {
    use super::*;

    fn os(s: &str) -> std::ffi::OsString {
        std::ffi::OsString::from(s)
    }

    fn rewrite(args: &[&str]) -> Vec<String> {
        rewrite_help_synonym(args.iter().map(|s| os(s)).collect())
            .into_iter()
            .map(|s| s.into_string().unwrap())
            .collect()
    }

    #[test]
    fn f3_rewrites_help_verb_to_verb_dash_dash_help() {
        assert_eq!(
            rewrite(&["inspect", "help", "logs"]),
            vec!["inspect", "logs", "--help"]
        );
        assert_eq!(
            rewrite(&["inspect", "help", "status"]),
            vec!["inspect", "status", "--help"]
        );
        assert_eq!(
            rewrite(&["inspect", "help", "restart"]),
            vec!["inspect", "restart", "--help"]
        );
    }

    #[test]
    fn f3_editorial_topic_takes_precedence_over_verb() {
        // `search` and `fleet` are BOTH a verb and a topic. The topic
        // wins so operators see the curated topic body, not clap's
        // flag list, by default.
        assert_eq!(
            rewrite(&["inspect", "help", "search"]),
            vec!["inspect", "help", "search"]
        );
        assert_eq!(
            rewrite(&["inspect", "help", "fleet"]),
            vec!["inspect", "help", "fleet"]
        );
    }

    #[test]
    fn f3_unknown_token_passes_through_unchanged() {
        // commands::help::run will then emit the F3 unknown-or-topic
        // error and exit 2.
        assert_eq!(
            rewrite(&["inspect", "help", "definitely-not-a-thing"]),
            vec!["inspect", "help", "definitely-not-a-thing"]
        );
    }

    #[test]
    fn f3_bare_help_unchanged() {
        assert_eq!(
            rewrite(&["inspect", "help"]),
            vec!["inspect", "help"]
        );
        assert_eq!(rewrite(&["inspect"]), vec!["inspect"]);
    }

    #[test]
    fn f3_pure_topic_unchanged() {
        // `quickstart`, `selectors`, etc. are topic-only (no verb
        // collision). They must NOT be rewritten.
        assert_eq!(
            rewrite(&["inspect", "help", "quickstart"]),
            vec!["inspect", "help", "quickstart"]
        );
        assert_eq!(
            rewrite(&["inspect", "help", "selectors"]),
            vec!["inspect", "help", "selectors"]
        );
    }

    #[test]
    fn f3_extra_args_after_verb_are_preserved() {
        // `inspect help logs --json` should rewrite to
        // `inspect logs --help --json`. clap will then reject the
        // unknown flag, surfacing a real error rather than silently
        // dropping the operator's intent.
        assert_eq!(
            rewrite(&["inspect", "help", "logs", "--json"]),
            vec!["inspect", "logs", "--help", "--json"]
        );
    }

    #[test]
    fn f3_does_not_rewrite_when_token_starts_with_dash() {
        // `inspect help --json` is the JSON help envelope, not a
        // verb synonym — never rewrite.
        assert_eq!(
            rewrite(&["inspect", "help", "--json"]),
            vec!["inspect", "help", "--json"]
        );
    }
}
