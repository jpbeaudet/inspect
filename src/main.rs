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
mod transcript;
mod verbs;

use cli::{Cli, Command};
use error::ExitKind;

fn main() -> ExitCode {
    // Install SIGINT/SIGTERM handlers as the very first thing — every
    // long-running loop in the engine and the SSH poller cooperates on
    // the global cancel flag (see `exec::cancel`).
    exec::cancel::install_handlers();

    // F18 (v0.1.3): install the per-process transcript context so
    // every subsequent user-visible emit can tee into the in-memory
    // buffer. `init` records the operator's argv (post-redaction);
    // the namespace + audit_id link are filled in by the verbs as
    // they resolve. `finalize` is called at every return below.
    let argv: Vec<String> = std::env::args().collect();
    transcript::init(&argv);

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
    // F10.1 (v0.1.3): namespace-flag-as-typo pre-parser. Catches
    // operators with `kubectl -n <ns>` muscle memory writing
    // `inspect why atlas-neo4j --on arte` and emits a chained hint
    // pointing at the canonical `arte/atlas-neo4j` form. Pure error-
    // message change: must run BEFORE clap so the user sees our
    // hint instead of clap's generic "unknown flag" rejection.
    if let Some(msg) = detect_namespace_flag_typo(&raw) {
        eprintln!("{msg}");
        transcript::tee_stderr(&msg);
        transcript::finalize(2);
        return ExitCode::from(2);
    }
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
                transcript::tee_stderr("inspect: cancelled by signal");
            }
        } else {
            // Success path: the verb finished its work but the user
            // still pressed Ctrl+C. Honor their intent and return 130
            // anyway — that's what `git`, `kubectl`, and `ssh` do.
            // No extra output: the verb already wrote its envelope.
        }
        transcript::finalize(130);
        return ExitCode::from(130);
    }
    match result {
        Ok(kind) => {
            let code = kind.code();
            transcript::finalize(code as i32);
            ExitCode::from(code)
        }
        Err(err) => {
            // HP-5: route the top-level error through the central
            // helper so the `see: inspect help <topic>` line is
            // attached automatically whenever the message matches a
            // catalog fragment. Cause chain still rendered verbatim.
            error::emit(err.to_string());
            let mut source = err.source();
            while let Some(cause) = source {
                let line = format!("  caused by: {cause}");
                eprintln!("{line}");
                transcript::tee_stderr(&line);
                source = cause.source();
            }
            let code = ExitKind::Error.code();
            transcript::finalize(code as i32);
            ExitCode::from(code)
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
        Command::Ssh(args) => commands::ssh::run(args),
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
        Command::Cp(args) => verbs::transfer::run_cp(args),
        Command::Put(args) => verbs::transfer::run_put(args),
        Command::Get(args) => verbs::transfer::run_get(args),
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
        Command::History(args) => commands::history::run(args),
        Command::Why(args) => commands::why::run(args),
        Command::Connectivity(args) => commands::connectivity::run(args),
        Command::Recipe(args) => commands::recipe::run(args),
        Command::Fleet(args) => commands::fleet::run(args),
        Command::Bundle(args) => commands::bundle::run(args),
        Command::Compose(args) => verbs::compose::dispatch(args),
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

/// F10.1 (v0.1.3): selector-taking verbs whose first positional
/// argument is the selector. Used by [`detect_namespace_flag_typo`]
/// to scope the kubectl-muscle-memory hint to verbs where the
/// suggested `<ns>/<service>` rewrite makes sense.
const F10_SELECTOR_VERBS: &[&str] = &[
    "why", "logs", "run", "health", "status", "ports", "cat", "grep", "ps", "ls", "find", "exec",
    "volumes", "images", "network", "resolve",
];

/// F10.1 (v0.1.3): operators with `kubectl -n <ns>` muscle memory
/// commonly type `inspect <verb> <service> --<ns-flag> <ns>` before
/// learning the canonical `<ns>/<service>` selector. Today's clap
/// rejection ("unknown flag --on") is unhelpful — we detect the
/// shape pre-clap and emit a chained hint pointing at the correct
/// rewrite.
///
/// Returns `Some(msg)` when the shape matches and the caller should
/// emit `msg` to stderr + exit 2; `None` otherwise.
///
/// Recognized flags: `--on`, `--in`, `--at`, `--host`, `--ns`,
/// `--namespace`. Detection is conservative: only fires when (a) the
/// verb is in [`F10_SELECTOR_VERBS`], (b) the first non-verb
/// positional is a bare token (no `:`, no `/`, no `@`), and (c) the
/// flag carries a single non-empty value. Otherwise we fall through
/// to clap's regular parsing path so legitimate verb-specific flags
/// (e.g. `--no-bundle` on `why`) are never shadowed.
fn detect_namespace_flag_typo(raw: &[std::ffi::OsString]) -> Option<String> {
    if raw.len() < 5 {
        return None;
    }
    let verb = raw.get(1)?.to_str()?;
    if !F10_SELECTOR_VERBS.contains(&verb) {
        return None;
    }
    // Walk the argv looking for a pair (--<ns-flag> <value>) AND a
    // bare positional token that doesn't already look like a selector
    // (no `/`, no `:`, no leading `@`). Order is flexible.
    const NS_FLAGS: &[&str] = &["--on", "--in", "--at", "--host", "--ns", "--namespace"];
    let mut ns_flag: Option<&str> = None;
    let mut ns_value: Option<String> = None;
    let mut bare: Option<String> = None;
    let mut i = 2;
    while i < raw.len() {
        let cur = raw[i].to_str()?;
        if let Some(f) = NS_FLAGS.iter().find(|f| **f == cur) {
            // `--ns-flag <value>` consumes the next argv slot.
            let v = raw.get(i + 1)?.to_str()?;
            if v.is_empty() || v.starts_with('-') {
                return None;
            }
            ns_flag = Some(*f);
            ns_value = Some(v.to_string());
            i += 2;
            continue;
        }
        // `--ns-flag=value` form.
        if let Some(f) = NS_FLAGS
            .iter()
            .find(|f| cur.starts_with(&format!("{}=", f)))
        {
            let v = &cur[f.len() + 1..];
            if v.is_empty() {
                return None;
            }
            ns_flag = Some(*f);
            ns_value = Some(v.to_string());
            i += 1;
            continue;
        }
        if cur.starts_with('-') {
            // Some other flag — skip its value if it takes one. We
            // can't tell without clap, so just skip the flag itself
            // and let the next iteration continue.
            i += 1;
            continue;
        }
        // Plain positional. Treat the first non-selector-shaped one
        // as the candidate bare-service token.
        if bare.is_none() && !cur.contains('/') && !cur.contains(':') && !cur.starts_with('@') {
            bare = Some(cur.to_string());
        }
        i += 1;
    }
    let (flag, ns) = (ns_flag?, ns_value?);
    let svc = bare?;
    Some(format!(
        "error: {flag} is not a flag — selectors are <ns>/<service>. \
         Did you mean 'inspect {verb} {ns}/{svc}'?"
    ))
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
        assert_eq!(rewrite(&["inspect", "help"]), vec!["inspect", "help"]);
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
