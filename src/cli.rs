//! CLI command-tree definitions.
//!
//! In Phase 0, only the namespace lifecycle commands (`add`, `list`, `remove`,
//! `test`, `show`) carry real implementations. All other verbs from the bible
//! are scaffolded here so the surface is stable and future phases can fill
//! them in without breaking flag layouts.

use clap::{Args, Parser, Subcommand};

const LONG_ABOUT: &str = "\
inspect — operational debugging CLI for cross-server search and safe hot-fix \
application.

Phase 0 implements namespace credential management. Other commands are \
scaffolded and will be filled in by subsequent phases.
";

#[derive(Debug, Parser)]
#[command(
    name = "inspect",
    bin_name = "inspect",
    version,
    about = "Operational debugging CLI",
    long_about = LONG_ABOUT,
    propagate_version = true,
    arg_required_else_help = true,
    // We ship our own `help` subcommand (HP-0+). Disable clap's
    // auto-generated one to avoid a "command name `help` is duplicated"
    // panic at startup. The auto-generated `--help` flag is unaffected.
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Add or update a namespace interactively.
    Add(AddArgs),
    /// List configured namespaces.
    List(ListArgs),
    /// Remove a namespace.
    Remove(RemoveArgs),
    /// Validate a namespace's configuration and reachability.
    Test(TestArgs),
    /// Show a namespace's resolved configuration (secrets redacted).
    Show(ShowArgs),

    // ---- Phase 1 ssh lifecycle ----------------------------------------------
    /// Open a persistent SSH session for a namespace.
    Connect(ConnectArgs),
    /// Close the persistent SSH session for a namespace.
    Disconnect(DisconnectArgs),
    /// List active persistent connections.
    Connections(ConnectionsArgs),
    /// Close all persistent connections.
    DisconnectAll(DisconnectAllArgs),

    // ---- Phase 2 discovery ---------------------------------------------------
    /// Run discovery against a namespace and persist its profile.
    Setup(SetupArgs),
    /// Alias of `setup`.
    Discover(SetupArgs),
    /// Show the cached profile for a namespace.
    Profile(ProfileArgs),

    // ---- Phase 4 read verbs --------------------------------------------------
    /// Show service inventory and health rollup.
    Status(StatusArgs),
    /// Detailed health checks.
    Health(HealthArgs),
    /// Tail or view container logs.
    Logs(LogsArgs),
    /// Search content in logs or files.
    Grep(GrepArgs),
    /// Read a file.
    Cat(CatArgs),
    /// List directory contents.
    Ls(LsArgs),
    /// Find files by pattern.
    Find(FindArgs),
    /// List running containers.
    Ps(PsArgs),
    /// List volumes.
    Volumes(SimpleSelectorArgs),
    /// List images.
    Images(SimpleSelectorArgs),
    /// List networks.
    Network(SimpleSelectorArgs),
    /// List listening ports.
    Ports(SimpleSelectorArgs),
    /// Diagnostic walk for a service.
    Why(WhyArgs),
    /// Connectivity matrix.
    Connectivity(ConnectivityArgs),
    /// Run a multi-step diagnostic recipe.
    Recipe(RecipeArgs),

    // ---- Phase 6/7 search ----------------------------------------------------
    /// LogQL search across mediums and namespaces.
    Search(SearchArgs),

    // ---- Phase 5 write verbs -------------------------------------------------
    /// Restart container(s).
    Restart(LifecycleArgs),
    /// Stop container(s).
    Stop(LifecycleArgs),
    /// Start container(s).
    Start(LifecycleArgs),
    /// Reload service(s) (SIGHUP).
    Reload(LifecycleArgs),
    /// Copy files between local and remote.
    Cp(CpArgs),
    /// Sed-style content edit.
    Edit(EditArgs),
    /// Delete file.
    Rm(PathArgArgs),
    /// Create directory.
    Mkdir(PathArgArgs),
    /// Create empty file.
    Touch(PathArgArgs),
    /// Change file mode.
    Chmod(ChmodArgs),
    /// Change file ownership.
    Chown(ChownArgs),
    /// Run a command on a target.
    Exec(ExecArgs),

    // ---- Phase 3 alias management --------------------------------------------
    /// Manage selector aliases.
    Alias(AliasArgs),

    /// Resolve a selector against discovered profiles and print the targets.
    /// Useful for testing selector grammar before the read/write verbs land.
    Resolve(ResolveArgs),

    // ---- Phase 5 audit + revert ----------------------------------------------
    /// Inspect or query the local audit log.
    Audit(AuditArgs),
    /// Revert a previous mutation by audit id.
    Revert(RevertArgs),

    // ---- Phase 11 fleet ------------------------------------------------------
    /// Run a verb across multiple namespaces.
    Fleet(FleetArgs),

    // ---- Help system (HP-0) -------------------------------------------------
    /// Show help on a topic, search help, or list all topics.
    ///
    /// Run `inspect help` for the topic + command index, or
    /// `inspect help <topic>` for in-depth documentation.
    Help(HelpArgs),
}

#[derive(Debug, Args)]
pub struct HelpArgs {
    /// Topic name (e.g. `quickstart`, `selectors`, `search`). Omit to
    /// print the topic + command index.
    pub topic: Option<String>,

    /// Search every help topic, verb, and example for a keyword.
    /// (Scheduled for HP-3; the flag is accepted today.)
    #[arg(long, value_name = "KEYWORD")]
    pub search: Option<String>,

    /// Emit the full help registry as a stable, versioned JSON
    /// document (the LLM/agent contract). Scheduled for HP-4.
    #[arg(long)]
    pub json: bool,

    /// Append the optional `verbose/<topic>.md` sidecar with edge
    /// cases and implementation notes. (Sidecars ship in HP-6.)
    #[arg(long)]
    pub verbose: bool,
}

#[derive(Debug, Args)]
pub struct AddArgs {
    /// Namespace short name (e.g. `arte`, `prod`, `staging`).
    pub namespace: String,

    /// Hostname or IP of the target server.
    #[arg(long)]
    pub host: Option<String>,

    /// SSH user.
    #[arg(long)]
    pub user: Option<String>,

    /// Path to the SSH private key.
    #[arg(long)]
    pub key_path: Option<String>,

    /// Name of an environment variable holding the key passphrase.
    #[arg(long)]
    pub key_passphrase_env: Option<String>,

    /// SSH port (default 22).
    #[arg(long)]
    pub port: Option<u16>,

    /// Overwrite an existing entry without prompting.
    #[arg(long)]
    pub force: bool,

    /// Run without prompting for any missing values.
    #[arg(long)]
    pub non_interactive: bool,
}

#[derive(Debug, Args)]
pub struct ListArgs {
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct RemoveArgs {
    /// Namespace to remove.
    pub namespace: String,

    /// Skip confirmation prompt.
    #[arg(long, short)]
    pub yes: bool,
}

#[derive(Debug, Args)]
pub struct TestArgs {
    /// Namespace to test.
    pub namespace: String,

    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct ShowArgs {
    /// Namespace to show.
    pub namespace: String,

    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

/// Generic selector container used by all not-yet-implemented verbs so that
/// the CLI surface stays parseable and forward-compatible.
#[derive(Debug, Args)]
pub struct SelectorArgs {
    /// Free-form selector or arguments. Validated in later phases.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

/// Phase 11 fleet orchestrator. Runs an inner verb across a set of
/// namespaces selected via `--ns` (glob, comma-list, or `@group`).
///
/// Layout: `inspect fleet [FLEET-FLAGS] <verb> [VERB-ARGS...]`
///
/// Fleet flags must come before the verb name. Everything after the verb
/// is forwarded verbatim to the child invocation.
#[derive(Debug, Args)]
pub struct FleetArgs {
    /// Namespace pattern: a glob (`prod-*`), a comma-separated list
    /// (`prod-1,prod-2`), or a group reference (`@prod`).
    #[arg(long)]
    pub ns: String,
    /// Override `INSPECT_FLEET_CONCURRENCY` (default 8).
    #[arg(long)]
    pub concurrency: Option<usize>,
    /// Skip the large-fanout interlock that would otherwise prompt when
    /// the matched namespace count exceeds the safety threshold.
    #[arg(long)]
    pub yes_all: bool,
    /// Emit a single aggregate JSON document with per-namespace results.
    #[arg(long)]
    pub json: bool,
    /// Stop after the first failing namespace instead of continuing.
    #[arg(long)]
    pub abort_on_error: bool,
    /// Field pitfall §4.3: run the first N namespaces as a canary
    /// before fanning out to the rest. Any failure during the canary
    /// phase aborts the run with a clear error and does not touch the
    /// remaining namespaces.
    #[arg(long, value_name = "N")]
    pub canary: Option<usize>,
    /// Inner verb to run (e.g. `status`, `restart`, `setup`).
    pub verb: String,
    /// Remaining args forwarded to the inner verb.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

// ---- Phase 9 diagnostics + recipes -----------------------------------------

#[derive(Debug, Args)]
pub struct WhyArgs {
    /// Selector resolving to one or more services to diagnose.
    pub selector: String,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct ConnectivityArgs {
    /// Selector resolving to one or more services.
    pub selector: String,
    /// Live-probe each declared edge with `bash -c '</dev/tcp/host/port'`.
    #[arg(long)]
    pub probe: bool,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct RecipeArgs {
    /// Recipe name (built-in) or absolute/relative path to a recipe YAML.
    pub name: String,
    /// Optional selector forwarded as `$SEL` to recipe steps that use it.
    #[arg(long)]
    pub sel: Option<String>,
    /// Apply mutating steps (default is dry-run).
    #[arg(long)]
    pub apply: bool,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct SearchArgs {
    /// LogQL query string. Always pass a single quoted argument.
    pub query: String,
    /// Restrict to records newer than this duration (e.g. `5m`, `1h`).
    #[arg(long)]
    pub since: Option<String>,
    /// Restrict to records older than this duration.
    #[arg(long)]
    pub until: Option<String>,
    /// Tail the last N records before applying further filters.
    #[arg(long)]
    pub tail: Option<usize>,
    /// Stream new records as they arrive (log queries only).
    #[arg(long, short = 'f')]
    pub follow: bool,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct ConnectArgs {
    /// Namespace to connect.
    pub namespace: String,
    /// Override the ControlPersist TTL (e.g. `4h`, `30m`, `2d`). Defaults
    /// to 4h inside Codespaces and 30m elsewhere.
    #[arg(long)]
    pub ttl: Option<String>,
    /// Skip the probe for an existing user-managed mux.
    #[arg(long)]
    pub no_existing_mux: bool,
    /// Force interactive prompt even when an env passphrase var is set.
    #[arg(long)]
    pub interactive: bool,
    /// Disable interactive prompts entirely (CI mode).
    #[arg(long)]
    pub non_interactive: bool,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct DisconnectArgs {
    /// Namespace to disconnect.
    pub namespace: String,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct ConnectionsArgs {
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct DisconnectAllArgs {
    /// Skip confirmation prompt.
    #[arg(long, short)]
    pub yes: bool,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct SetupArgs {
    /// Namespace to discover.
    pub namespace: String,
    /// Force a full re-discovery, ignoring cache TTL.
    #[arg(long)]
    pub force: bool,
    /// Skip the systemd probe (useful when the user has no journal access).
    #[arg(long)]
    pub skip_systemd: bool,
    /// Skip host-port listener probes.
    #[arg(long)]
    pub skip_host_listeners: bool,
    /// Run a synchronous drift check against the cached profile and exit
    /// without re-discovering.
    #[arg(long, conflicts_with_all = ["force", "skip_systemd", "skip_host_listeners"])]
    pub check_drift: bool,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct ProfileArgs {
    /// Namespace whose profile to display.
    pub namespace: String,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

// ---- Phase 3 -----------------------------------------------------------------

#[derive(Debug, Args)]
pub struct AliasArgs {
    #[command(subcommand)]
    pub command: AliasCommand,
}

#[derive(Debug, Subcommand)]
pub enum AliasCommand {
    /// Save a selector under a short name.
    Add(AliasAddArgs),
    /// List configured aliases.
    List(AliasListArgs),
    /// Remove an alias.
    Remove(AliasRemoveArgs),
    /// Show one alias in detail.
    Show(AliasShowArgs),
}

#[derive(Debug, Args)]
pub struct AliasAddArgs {
    /// Alias name (without the leading '@').
    pub name: String,
    /// Selector text to save (verb-style or LogQL `{...}` form).
    pub selector: String,
    /// Optional description shown by `alias list`.
    #[arg(long)]
    pub description: Option<String>,
    /// Overwrite an existing alias.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct AliasListArgs {
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct AliasRemoveArgs {
    /// Alias name (without the leading '@').
    pub name: String,
}

#[derive(Debug, Args)]
pub struct AliasShowArgs {
    /// Alias name (without the leading '@').
    pub name: String,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct ResolveArgs {
    /// Selector text (e.g. `arte/pulse`, `prod-*/storage`, `@plogs`).
    pub selector: String,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

// ---- Phase 4 read verbs ------------------------------------------------------

/// Reusable arg block for verbs that just need a selector + `--json`.
#[derive(Debug, Args)]
pub struct SimpleSelectorArgs {
    /// Selector (server, server/service, etc.).
    pub selector: String,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Selector (server, server/service, etc.).
    pub selector: String,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct HealthArgs {
    pub selector: String,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct PsArgs {
    pub selector: String,
    /// Show all containers (default shows just running).
    #[arg(short = 'a', long)]
    pub all: bool,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct LogsArgs {
    pub selector: String,
    /// Show logs since duration (e.g. 30s, 5m, 1h, 2d).
    #[arg(long)]
    pub since: Option<String>,
    /// Show logs until duration.
    #[arg(long)]
    pub until: Option<String>,
    /// Number of lines from the tail.
    #[arg(long)]
    pub tail: Option<u64>,
    /// Stream logs.
    #[arg(short = 'f', long)]
    pub follow: bool,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
    /// Hidden: ssh-side timeout for follow mode (seconds).
    #[arg(long, hide = true)]
    pub follow_timeout_secs: Option<u64>,
}

#[derive(Debug, Args)]
pub struct GrepArgs {
    /// Pattern to search for.
    pub pattern: String,
    /// Selector. May include `:path` to grep a file.
    pub selector: String,
    #[arg(long)]
    pub since: Option<String>,
    #[arg(long)]
    pub until: Option<String>,
    #[arg(long)]
    pub tail: Option<u64>,

    /// Case-insensitive (overrides smart-case).
    #[arg(short = 'i', long = "ignore-case")]
    pub ignore_case: bool,
    /// Force case-sensitive (overrides smart-case).
    #[arg(short = 's', long = "case-sensitive")]
    pub case_sensitive: bool,
    /// Match whole words.
    #[arg(short = 'w', long = "word")]
    pub word: bool,
    /// Treat pattern as fixed string.
    #[arg(short = 'F', long = "fixed-strings")]
    pub fixed: bool,
    /// Treat pattern as extended regex.
    #[arg(short = 'E', long = "extended-regexp")]
    pub extended: bool,
    /// Invert match.
    #[arg(short = 'v', long = "invert-match")]
    pub invert: bool,
    /// Stop after N matches per target.
    #[arg(short = 'm', long = "max-count")]
    pub max_count: Option<u64>,
    /// Print N lines after each match.
    #[arg(short = 'A', long = "after")]
    pub after: Option<u64>,
    /// Print N lines before each match.
    #[arg(short = 'B', long = "before")]
    pub before: Option<u64>,
    /// Print N lines around each match.
    #[arg(short = 'C', long = "context")]
    pub context: Option<u64>,
    /// Just count matches per target.
    #[arg(short = 'c', long = "count")]
    pub count: bool,

    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct CatArgs {
    /// Selector with `:path` (e.g. `arte/atlas:/etc/atlas.conf`).
    pub target: String,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct LsArgs {
    /// Selector with `:path`.
    pub target: String,
    /// Show hidden entries (`-A`).
    #[arg(short = 'A', long)]
    pub all: bool,
    /// Long listing (`-l`).
    #[arg(short = 'l', long)]
    pub long: bool,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct FindArgs {
    /// Selector with `:path`.
    pub target: String,
    /// Optional name pattern (find -name).
    pub pattern: Option<String>,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

// ---- Phase 5 write verbs -----------------------------------------------------

/// Shared safety flags for every write verb. Defined inline on each
/// arg-struct rather than via `#[command(flatten)]` so the help text
/// stays grouped per verb.
#[derive(Debug, Args)]
pub struct LifecycleArgs {
    /// Selector (server, server/service, ...).
    pub selector: String,
    /// Actually perform the mutation. Without this flag, the verb is a dry-run.
    #[arg(long)]
    pub apply: bool,
    /// Skip the per-verb confirmation prompt.
    #[arg(short = 'y', long)]
    pub yes: bool,
    /// Skip the large-fanout interlock as well.
    #[arg(long)]
    pub yes_all: bool,
}

#[derive(Debug, Args)]
pub struct ExecArgs {
    /// Selector.
    pub selector: String,
    /// Command and arguments after `--`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub cmd: Vec<String>,
    #[arg(long)]
    pub apply: bool,
    /// Field pitfall §3.2: `exec` runs free-form code on the remote and
    /// is not symmetric with the predictable write verbs. Require this
    /// extra flag in addition to `--apply` so a misclick or muscle-
    /// memory `--apply` cannot silently shell out across the fleet.
    #[arg(long)]
    pub allow_exec: bool,
    #[arg(short = 'y', long)]
    pub yes: bool,
    #[arg(long)]
    pub yes_all: bool,
    /// Override the per-target timeout (seconds).
    #[arg(long)]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Args)]
pub struct PathArgArgs {
    /// Selector with `:path`.
    pub target: String,
    #[arg(long)]
    pub apply: bool,
    #[arg(short = 'y', long)]
    pub yes: bool,
    #[arg(long)]
    pub yes_all: bool,
}

#[derive(Debug, Args)]
pub struct ChmodArgs {
    /// Selector with `:path`.
    pub target: String,
    /// Octal (e.g. `0644`) or symbolic (`u+x`).
    pub mode: String,
    #[arg(long)]
    pub apply: bool,
    #[arg(short = 'y', long)]
    pub yes: bool,
    #[arg(long)]
    pub yes_all: bool,
}

#[derive(Debug, Args)]
pub struct ChownArgs {
    /// Selector with `:path`.
    pub target: String,
    /// `user[:group]`.
    pub owner: String,
    #[arg(long)]
    pub apply: bool,
    #[arg(short = 'y', long)]
    pub yes: bool,
    #[arg(long)]
    pub yes_all: bool,
}

#[derive(Debug, Args)]
pub struct CpArgs {
    /// Source: local path or `<sel>:<path>`.
    pub source: String,
    /// Destination: local path or `<sel>:<path>`.
    pub dest: String,
    #[arg(long)]
    pub apply: bool,
    /// Show a unified diff in dry-run mode.
    #[arg(long)]
    pub diff: bool,
    #[arg(short = 'y', long)]
    pub yes: bool,
    #[arg(long)]
    pub yes_all: bool,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct EditArgs {
    /// Selector with `:path`.
    pub target: String,
    /// Sed substitution expression (e.g. `s/old/new/g`).
    pub expr: String,
    #[arg(long)]
    pub apply: bool,
    #[arg(short = 'y', long)]
    pub yes: bool,
    #[arg(long)]
    pub yes_all: bool,
}

#[derive(Debug, Args)]
pub struct AuditArgs {
    #[command(subcommand)]
    pub command: AuditCommand,
}

#[derive(Debug, Subcommand)]
pub enum AuditCommand {
    /// List recent audit entries (newest first).
    Ls(AuditLsArgs),
    /// Show one audit entry in detail.
    Show(AuditShowArgs),
    /// Filter audit entries by substring (id/verb/selector/args).
    Grep(AuditGrepArgs),
    /// Field pitfall §3.4: best-effort integrity check of the local
    /// audit log. Verifies every JSONL line parses, every referenced
    /// snapshot file exists, and every snapshot's on-disk sha256
    /// matches the `previous_hash` recorded in the entry. This is
    /// **tamper detection, not tamper prevention** — a privileged
    /// local user can still rewrite the log; for stronger guarantees
    /// forward audit entries to an append-only log sink (syslog,
    /// journald, or a remote collector).
    Verify(AuditVerifyArgs),
}

#[derive(Debug, Args)]
pub struct AuditLsArgs {
    /// Maximum entries to show.
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct AuditShowArgs {
    pub id: String,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct AuditGrepArgs {
    pub pattern: String,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct AuditVerifyArgs {
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct RevertArgs {
    /// Audit id (or unique prefix).
    pub audit_id: String,
    #[arg(long)]
    pub apply: bool,
    /// Override the drift check (current remote != recorded new_hash).
    #[arg(long)]
    pub force: bool,
    #[arg(short = 'y', long)]
    pub yes: bool,
    #[arg(long)]
    pub yes_all: bool,
}
