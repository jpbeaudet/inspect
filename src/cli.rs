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
    arg_required_else_help = true
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
    Status(SelectorArgs),
    /// Detailed health checks.
    Health(SelectorArgs),
    /// Tail or view container logs.
    Logs(SelectorArgs),
    /// Search content in logs or files.
    Grep(SelectorArgs),
    /// Read a file.
    Cat(SelectorArgs),
    /// List directory contents.
    Ls(SelectorArgs),
    /// Find files by pattern.
    Find(SelectorArgs),
    /// List running containers.
    Ps(SelectorArgs),
    /// List volumes.
    Volumes(SelectorArgs),
    /// List images.
    Images(SelectorArgs),
    /// List networks.
    Network(SelectorArgs),
    /// List listening ports.
    Ports(SelectorArgs),
    /// Diagnostic walk for a service.
    Why(SelectorArgs),
    /// Connectivity matrix.
    Connectivity(SelectorArgs),
    /// Run a multi-step diagnostic recipe.
    Recipe(SelectorArgs),

    // ---- Phase 6/7 search ----------------------------------------------------
    /// LogQL search across mediums and namespaces.
    Search(SelectorArgs),

    // ---- Phase 5 write verbs -------------------------------------------------
    /// Restart container(s).
    Restart(SelectorArgs),
    /// Stop container(s).
    Stop(SelectorArgs),
    /// Start container(s).
    Start(SelectorArgs),
    /// Reload service(s) (SIGHUP).
    Reload(SelectorArgs),
    /// Copy files between local and remote.
    Cp(SelectorArgs),
    /// Sed-style content edit.
    Edit(SelectorArgs),
    /// Delete file.
    Rm(SelectorArgs),
    /// Create directory.
    Mkdir(SelectorArgs),
    /// Create empty file.
    Touch(SelectorArgs),
    /// Change file mode.
    Chmod(SelectorArgs),
    /// Change file ownership.
    Chown(SelectorArgs),
    /// Run a command on a target.
    Exec(SelectorArgs),

    // ---- Phase 3 alias management --------------------------------------------
    /// Manage selector aliases.
    Alias(AliasArgs),

    /// Resolve a selector against discovered profiles and print the targets.
    /// Useful for testing selector grammar before the read/write verbs land.
    Resolve(ResolveArgs),

    // ---- Phase 5 audit + revert ----------------------------------------------
    /// Inspect or query the local audit log.
    Audit(SelectorArgs),
    /// Revert a previous mutation by audit id.
    Revert(SelectorArgs),

    // ---- Phase 11 fleet ------------------------------------------------------
    /// Run a verb across multiple namespaces.
    Fleet(SelectorArgs),
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
    /// Emit machine-readable JSON.
    #[arg(long)]
    pub json: bool,
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

    /// Emit machine-readable JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct ShowArgs {
    /// Namespace to show.
    pub namespace: String,

    /// Emit machine-readable JSON.
    #[arg(long)]
    pub json: bool,
}

/// Generic selector container used by all not-yet-implemented verbs so that
/// the CLI surface stays parseable and forward-compatible.
#[derive(Debug, Args)]
pub struct SelectorArgs {
    /// Free-form selector or arguments. Validated in later phases.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
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
    /// Emit machine-readable JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct DisconnectArgs {
    /// Namespace to disconnect.
    pub namespace: String,
    /// Emit machine-readable JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct ConnectionsArgs {
    /// Emit machine-readable JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct DisconnectAllArgs {
    /// Skip confirmation prompt.
    #[arg(long, short)]
    pub yes: bool,
    /// Emit machine-readable JSON.
    #[arg(long)]
    pub json: bool,
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
    /// Emit machine-readable JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct ProfileArgs {
    /// Namespace whose profile to display.
    pub namespace: String,
    /// Emit machine-readable JSON.
    #[arg(long)]
    pub json: bool,
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
    /// Emit machine-readable JSON.
    #[arg(long)]
    pub json: bool,
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
    /// Emit machine-readable JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct ResolveArgs {
    /// Selector text (e.g. `arte/pulse`, `prod-*/storage`, `@plogs`).
    pub selector: String,
    /// Emit machine-readable JSON.
    #[arg(long)]
    pub json: bool,
}
