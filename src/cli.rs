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

// ---------------------------------------------------------------------------
// HP-2: per-verb cross-link footers.
//
// These constants are attached to each command's `#[command(after_help = …)]`
// attribute. The exact text is the contract — the test suite in
// `tests/help_contract.rs` pins the format and verifies every constant is
// in lock-step with `crate::help::topics::see_also_line(verb)`, which is the
// runtime source of truth used by `inspect help --json` (HP-4).
//
// Format (frozen): `See also: inspect help <t1>, inspect help <t2>, ...`
// ---------------------------------------------------------------------------

const SEE_ALSO_READ: &str =
    "See also: inspect help selectors, inspect help formats, inspect help examples";
const SEE_ALSO_RESOLVE: &str =
    "See also: inspect help selectors, inspect help aliases, inspect help examples";
const SEE_ALSO_SEARCH: &str = "See also: inspect help search, inspect help selectors, \
                               inspect help aliases, inspect help formats";
const SEE_ALSO_WRITE: &str =
    "See also: inspect help write, inspect help safety, inspect help fleet";
const SEE_ALSO_SAFETY: &str = "See also: inspect help safety, inspect help write";
const SEE_ALSO_FLEET: &str =
    "See also: inspect help fleet, inspect help write, inspect help selectors";
const SEE_ALSO_RECIPES: &str = "See also: inspect help recipes, inspect help examples";
const SEE_ALSO_DISCOVER: &str =
    "See also: inspect help discovery, inspect help ssh, inspect help quickstart";
const SEE_ALSO_CONNECT: &str =
    "See also: inspect help ssh, inspect help discovery, inspect help quickstart";
const SEE_ALSO_SSH: &str = "See also: inspect help ssh, inspect help discovery";
const SEE_ALSO_ALIAS: &str =
    "See also: inspect help aliases, inspect help selectors, inspect help search";
const SEE_ALSO_HELP: &str = "See also: inspect help quickstart, inspect help examples";

// ---------------------------------------------------------------------------
// HP-2: per-verb `long_about` blocks.
//
// These are attached to each `Command` variant via `#[command(long_about = …)]`
// (the inner `Args` struct's long_about is shadowed by the variant doc-comment,
// so the attribute must live on the variant itself). One constant per Args
// struct family; verbs that share an Args type also share the long_about, with
// examples chosen to demonstrate the cluster's idiom.
//
// Each block opens with a one-paragraph DESCRIPTION and ends with an EXAMPLES
// stanza of three `$ inspect …` lines so `inspect <verb> --help` is
// self-sufficient (HP-2 DoD).
// ---------------------------------------------------------------------------

const LONG_ADD: &str = "\
Register or update a namespace's SSH credentials. Idempotent: running \
`add` again with new flags rewrites the entry.

EXAMPLES
  $ inspect add arte
  $ inspect add prod-eu --host prod-eu.example.com --user ops --key-path ~/.ssh/prod
  $ inspect add staging --non-interactive --force";

const LONG_LIST: &str = "\
Print every configured namespace with its host, user, and last-known \
reachability.

EXAMPLES
  $ inspect list
  $ inspect list --json
  $ inspect list --csv";

const LONG_REMOVE: &str = "\
Delete a namespace's stored credentials and cached profile. The SSH key \
file itself is never touched.

EXAMPLES
  $ inspect remove staging
  $ inspect remove prod-eu --yes";

const LONG_TEST: &str = "\
Validate a namespace's configuration: env vars resolve, key file is \
readable, host is reachable. Does not run discovery.

EXAMPLES
  $ inspect test arte
  $ inspect test prod-eu --json";

const LONG_SHOW: &str = "\
Print a namespace's resolved configuration with secrets redacted. Use \
`--profile` to see the cached discovery profile.

EXAMPLES
  $ inspect show arte
  $ inspect show arte --json
  $ inspect show arte --profile";

const LONG_FLEET: &str = "\
Run an inner verb across multiple namespaces selected by `--ns` (glob, \
comma-list, or `@group`). Results stream as they arrive; failed \
namespaces appear with error rows but do not abort the run unless \
`--abort-on-error` is set.

EXAMPLES
  $ inspect fleet --ns 'prod-*' status
  $ inspect fleet --ns @prod restart pulse --apply
  $ inspect fleet --ns 'prod-*' --canary 1 restart pulse --apply";

const LONG_WHY: &str = "\
Diagnostic walk for a service: status, recent errors, health, and \
connectivity from one selector. The built-in `why` recipe.

EXAMPLES
  $ inspect why arte/atlas
  $ inspect why 'prod-*/storage' --json";

const LONG_CONNECTIVITY: &str = "\
Print the connectivity matrix for the selected services. With `--probe`, \
live-test each declared edge with bash /dev/tcp.

EXAMPLES
  $ inspect connectivity arte
  $ inspect connectivity arte/atlas --probe
  $ inspect connectivity 'prod-*' --json";

const LONG_RECIPE: &str = "\
Run a multi-step diagnostic or remediation recipe. Built-in recipes are \
listed in `inspect help recipes`. User recipes live under \
~/.inspect/recipes/<name>.yaml. Mutating recipes require `--apply`.

EXAMPLES
  $ inspect recipe deploy-check arte
  $ inspect recipe disk-audit 'prod-*'
  $ inspect recipe cycle-atlas --sel arte/atlas --apply";

const LONG_SEARCH: &str = "\
LogQL query across logs, files, and discovery sources. Queries are \
always single-quoted. The pipeline is the LogQL DSL, not the shell.

EXAMPLES
  $ inspect search '{server=\"arte\", source=\"logs\"} |= \"error\"' --since 1h
  $ inspect search '{server=~\"prod-.*\", service=\"storage\", source=\"logs\"} |= \"timeout\"'
  $ inspect search 'sum by (service) (count_over_time({server=\"arte\", source=\"logs\"} |= \"error\" [5m]))'";

const LONG_CONNECT: &str = "\
Open a persistent SSH session for a namespace. The first call prompts \
for the key passphrase (if encrypted); subsequent commands reuse the \
session via a control socket until the TTL expires.

EXAMPLES
  $ inspect connect arte
  $ inspect connect prod-eu --ttl 4h
  $ inspect connect arte --non-interactive";

const LONG_DISCONNECT: &str = "\
Close the persistent SSH session for one namespace. Does not affect \
user-managed ControlMaster sockets in ~/.ssh/config.

EXAMPLES
  $ inspect disconnect arte
  $ inspect disconnect prod-eu --json";

const LONG_CONNECTIONS: &str = "\
List active inspect-managed SSH sessions, with TTL remaining and the \
path to each control socket.

EXAMPLES
  $ inspect connections
  $ inspect connections --json
  $ inspect connections --csv";

const LONG_DISCONNECT_ALL: &str = "\
Close every active inspect-managed SSH session. Prompts for confirmation \
unless `--yes` is passed.

EXAMPLES
  $ inspect disconnect-all
  $ inspect disconnect-all --yes";

const LONG_SETUP: &str = "\
Run discovery against a namespace and persist its profile (containers, \
volumes, networks, listeners, remote tooling). Cached profiles live \
under ~/.inspect/profiles/.

EXAMPLES
  $ inspect setup arte
  $ inspect setup prod-eu --force
  $ inspect setup arte --check-drift";

const LONG_PROFILE: &str = "\
Print the cached discovery profile for a namespace.

EXAMPLES
  $ inspect profile arte
  $ inspect profile arte --json
  $ inspect profile prod-eu --yaml";

const LONG_ALIAS: &str = "\
Manage saved selector aliases (`@name`). Subcommands: add, list, \
remove, show.

EXAMPLES
  $ inspect alias add plogs '{server=\"arte\", service=\"pulse\", source=\"logs\"}'
  $ inspect alias add storage-prod 'prod-*/storage'
  $ inspect alias list";

const LONG_RESOLVE: &str = "\
Resolve a selector against discovered profiles and print the target \
list. Useful to test selector grammar before a real verb call.

EXAMPLES
  $ inspect resolve arte/storage
  $ inspect resolve 'prod-*/atlas'
  $ inspect resolve @plogs";

const LONG_SIMPLE_SELECTOR: &str = "\
Generic listing for ports / volumes / images / network. The verb name \
selects which medium to list.

EXAMPLES
  $ inspect volumes arte
  $ inspect images arte/atlas
  $ inspect ports 'prod-*'";

const LONG_STATUS: &str = "\
Service inventory and health rollup for the selected targets.

EXAMPLES
  $ inspect status arte
  $ inspect status arte/atlas
  $ inspect status 'prod-*' --json";

const LONG_HEALTH: &str = "\
Detailed health checks for the selected services. Calls each service's \
declared health endpoint and reports per-check status.

EXAMPLES
  $ inspect health arte
  $ inspect health arte/atlas --json
  $ inspect health 'prod-*/storage'";

const LONG_PS: &str = "\
List containers on the selected targets.

EXAMPLES
  $ inspect ps arte
  $ inspect ps arte --all
  $ inspect ps 'prod-*' --json";

const LONG_LOGS: &str = "\
Tail or view container logs. With `--follow`, streams new records until \
interrupted; the inner SSH session auto-resumes on transient failures.

EXAMPLES
  $ inspect logs arte/pulse --since 30m
  $ inspect logs arte/atlas --tail 200 --follow
  $ inspect logs 'prod-*/storage' --since 1h --json";

const LONG_GREP: &str = "\
Search content in logs or files on the selected targets. Selector may \
include `:path` to grep a specific file. Smart-case by default.

EXAMPLES
  $ inspect grep \"error\" arte/pulse --since 1h
  $ inspect grep -i \"timeout\" 'prod-*/storage' --since 30m
  $ inspect grep \"milvus\" arte/atlas:/var/log/atlas.log";

const LONG_CAT: &str = "\
Print the contents of a file inside a container or on the host.

EXAMPLES
  $ inspect cat arte/atlas:/etc/atlas.conf
  $ inspect cat arte/_:/var/log/syslog
  $ inspect cat arte/pulse:/etc/pulse.conf --raw";

const LONG_LS: &str = "\
List directory contents on a target.

EXAMPLES
  $ inspect ls arte/atlas:/etc
  $ inspect ls arte/_:/var/log -A
  $ inspect ls arte/pulse:/var/lib/pulse -l";

const LONG_FIND: &str = "\
Find files by name pattern on a target. Wraps remote `find` and \
respects discovery's tooling probe.

EXAMPLES
  $ inspect find arte/atlas:/etc \"*.conf\"
  $ inspect find arte/_:/var/log \"*.log\"
  $ inspect find 'prod-*/storage:/data'";

const LONG_LIFECYCLE: &str = "\
Container lifecycle (the verb form chooses the action: restart / stop / \
start / reload). Dry-run by default; `--apply` executes.

EXAMPLES
  $ inspect restart arte/pulse
  $ inspect restart arte/pulse --apply
  $ inspect stop 'prod-*/atlas' --apply --yes-all";

const LONG_EXEC: &str = "\
Run a state-changing command on the selected targets. Audited; \
`--apply` required to actually execute. For read-only inspection use \
`inspect run` instead -- it skips the audit log and the apply gate.

EXAMPLES
  $ inspect exec arte/atlas -- systemctl restart atlas --apply
  $ inspect exec arte/atlas --apply -- 'touch /var/lib/atlas/.maint'
  $ inspect exec 'prod-*' --apply --yes-all -- 'rm -f /tmp/lockfile'";

const LONG_RUN: &str = "\
Run a read-only command on the selected targets. Output is streamed; \
secrets in `KEY=VALUE` form are masked unless `--show-secrets` is \
passed. No audit entry, no apply gate -- this is the verb to reach \
for when you want a quick \"what is the state?\" check.

STDIN HANDLING (F9, v0.1.3)
  When `inspect run`'s own stdin is non-tty (piped or redirected from
  a file), it is forwarded byte-for-byte to the remote command's
  stdin and closed on EOF, so commands that read until EOF (`sh`,
  `psql`, `cat`, `tee`) terminate normally. When local stdin is a
  tty, no forwarding happens (same as `ssh -T host cmd <terminal>`).

  Forwarding writes a one-line audit entry with `stdin_bytes: <N>`;
  pass `--audit-stdin-hash` to also record `stdin_sha256` of the
  forwarded payload (off by default for perf).

  Default size cap is 10 MiB; raise with `--stdin-max <SIZE>` (k/m/g
  suffixes), set `--stdin-max 0` to disable, or use `inspect cp` for
  bulk transfer (faster, resumable, audit-tracked separately).

  Pass `--no-stdin` to refuse to forward; if you pass `--no-stdin`
  while local stdin has data waiting, `inspect run` exits 2 BEFORE
  dispatching the remote command (never silently discards input).

EXAMPLES
  $ inspect run arte/atlas -- env
  $ inspect run arte/atlas -- 'docker ps --format json'
  $ inspect run 'prod-*' -- 'df -h /var'
  $ inspect run arte 'docker exec -i atlas-pg sh' < ./init.sql
  $ cat big.tar.gz | inspect run arte --stdin-max 100m -- 'tar -xz -C /opt'";

const LONG_WATCH: &str = "\
Block until a predicate over the target becomes true (B10, v0.1.2). \
Exactly one `--until-*` flag is required:

  --until-cmd <CMD>      run CMD on the target each interval and apply
                         a comparator (--equals/--matches/--gt/--lt/
                         --changes/--stable-for); without a comparator,
                         the predicate is `exit code == 0`.
  --until-log <PATTERN>  poll `docker logs` since watch start and match
                         PATTERN literally (default) or as a regex
                         (`--regex`).
  --until-sql <SQL>      run SQL via `docker exec <ctr> psql -tAc ...`;
                         match if result trims to t/true/1/yes.
  --until-http <URL>     curl URL on the target; match on HTTP 200 by
                         default, or apply `--match <EXPR>` with the
                         tiny DSL `<lhs> <op> <rhs>` where lhs is
                         `body`/`status`/`$.json.path`, op is
                         ==/!=/</>/contains.

Exit codes: 0 on match, 124 on timeout (matches timeout(1)), 2 on \
error. Default --interval is 2s and default --timeout cap is 10m. \
Each watch records one audit entry with verb=`watch`, the predicate, \
elapsed time, and the value that triggered the match.

EXAMPLES
  $ inspect watch arte/atlas --until-cmd 'systemctl is-active atlas' --equals active
  $ inspect watch arte/db    --until-sql 'SELECT pg_is_in_recovery()=false' --psql-opts '-U postgres'
  $ inspect watch arte/api   --until-http http://localhost:8080/health --match 'status == 200'
  $ inspect watch arte/atlas --until-log 'ready to accept connections' --timeout 5m";

const LONG_PATH_ARG: &str = "\
File operation on a target path (the verb form chooses: rm / mkdir / \
touch). Dry-run by default; `--apply` executes.

EXAMPLES
  $ inspect rm arte/atlas:/tmp/stale.log --apply
  $ inspect mkdir arte/_:/var/log/inspect --apply
  $ inspect touch arte/atlas:/tmp/marker --apply";

const LONG_CHMOD: &str = "\
Change file mode (octal or symbolic). Dry-run by default; `--apply` \
executes.

EXAMPLES
  $ inspect chmod arte/atlas:/etc/atlas.conf 0644
  $ inspect chmod arte/atlas:/etc/atlas.conf 0644 --apply
  $ inspect chmod arte/atlas:/usr/local/bin/atlas u+x --apply";

const LONG_CHOWN: &str = "\
Change file ownership (`user[:group]`). Dry-run by default; `--apply` \
executes.

EXAMPLES
  $ inspect chown arte/atlas:/etc/atlas.conf atlas
  $ inspect chown arte/atlas:/etc/atlas.conf atlas:atlas --apply
  $ inspect chown arte/_:/var/log/atlas root:adm --apply";

const LONG_CP: &str = "\
Copy a file between local and remote (push or pull, depending on which \
side carries `<sel>:<path>`). Dry-run by default; `--diff` shows a \
unified diff before `--apply`.

EXAMPLES
  $ inspect cp ./fix.conf arte/pulse:/etc/pulse.conf --diff
  $ inspect cp ./fix.conf arte/pulse:/etc/pulse.conf --apply
  $ inspect cp arte/atlas:/var/log/atlas.log ./atlas.log";

const LONG_EDIT: &str = "\
In-place sed-style content edit (atomic). Dry-run by default — shows a \
unified diff. `--apply` writes.

EXAMPLES
  $ inspect edit arte/atlas:/etc/atlas.conf 's/timeout=30/timeout=60/'
  $ inspect edit arte/atlas:/etc/atlas.conf 's/timeout=30/timeout=60/' --apply
  $ inspect edit '*/atlas:/etc/atlas.conf' 's|debug=on|debug=off|' --apply --yes-all";

const LONG_AUDIT: &str = "\
Inspect or query the local audit log. Subcommands: ls, show, grep, \
verify.

EXAMPLES
  $ inspect audit ls
  $ inspect audit show <id>
  $ inspect audit grep \"atlas\"";

const LONG_CACHE: &str = "\
Inspect or invalidate the local runtime cache.

inspect maintains a two-tier cache for read verbs (status, health, why):

  inventory tier  ~/.inspect/profiles/<ns>.yaml      (the discovered service
                                                      list, refreshed by
                                                      `inspect setup`)
  runtime tier    ~/.inspect/cache/<ns>/runtime.json (per-container
                                                      running/health/restart
                                                      counts; default TTL 10s)

The runtime tier is what makes `inspect status` fast. It's invalidated
automatically by every successful mutation verb (restart, stop, start,
reload). Lifetime is controlled by INSPECT_RUNTIME_TTL_SECS (default \
10s; '0' disables; 'never' = infinite).

Subcommands:
  show   Print one row per cached namespace with runtime/inventory
         age, staleness, and on-disk size.
  clear  Delete cached runtime snapshot(s). With no namespace,
         clears every cached namespace.

EXAMPLES
  $ inspect cache show
  $ inspect cache clear arte
  $ inspect cache clear --all";

const LONG_REVERT: &str = "\
Revert a previous mutation by audit id. Dry-run by default (shows the \
reverse diff); `--apply` restores the original content. Refuses if the \
file changed since the recorded mutation unless `--force`.

EXAMPLES
  $ inspect revert <audit-id>
  $ inspect revert <audit-id> --apply
  $ inspect revert <audit-id> --apply --force";

const LONG_BUNDLE: &str = "\
YAML-driven multi-step orchestration. A bundle declares preflight \
checks, an ordered list of steps (exec / run / watch), per-step \
rollback actions, an optional bundle-level rollback block, and \
postflight checks.

Subcommands:
  plan   Validate the bundle, interpolate {{ vars.* }} / {{ matrix.* }},
         and print the rendered step list. Never touches a remote.
  apply  Run preflight, then steps in order. On failure, route via the
         step's `on_failure:` (abort | continue | rollback | rollback_to:<id>).
         Postflight runs on success and is reported but does NOT trigger
         rollback.

Audit:
  Every exec step (and bundle.rollback / bundle.watch action) writes one
  audit entry tagged with bundle_id (a fresh ULID-shaped id per apply
  invocation) and bundle_step (the step's `id:`). `inspect audit ls
  --bundle <id>` filters to a single run.

EXAMPLES
  $ inspect bundle plan deploy.yaml
  $ inspect bundle apply deploy.yaml --apply --reason 'INC-1234'
  $ inspect bundle apply deploy.yaml --apply --no-prompt    # CI-safe";

const LONG_HELP: &str = "\
Show in-binary documentation. Run with no topic to see the topic + \
command index, or with a topic name for the full prose. Use `--search` \
(HP-3) to find help by keyword and `--json` (HP-4) to get the full \
machine-readable surface.

EXAMPLES
  $ inspect help
  $ inspect help quickstart
  $ inspect help all";

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
    #[command(long_about = LONG_ADD)]
    Add(AddArgs),
    /// List configured namespaces.
    #[command(long_about = LONG_LIST)]
    List(ListArgs),
    /// Remove a namespace.
    #[command(long_about = LONG_REMOVE)]
    Remove(RemoveArgs),
    /// Validate a namespace's configuration and reachability.
    #[command(long_about = LONG_TEST)]
    Test(TestArgs),
    /// Show a namespace's resolved configuration (secrets redacted).
    #[command(long_about = LONG_SHOW)]
    Show(ShowArgs),

    // ---- Phase 1 ssh lifecycle ----------------------------------------------
    /// Open a persistent SSH session for a namespace.
    #[command(long_about = LONG_CONNECT)]
    Connect(ConnectArgs),
    /// Close the persistent SSH session for a namespace.
    #[command(long_about = LONG_DISCONNECT)]
    Disconnect(DisconnectArgs),
    /// List active persistent connections.
    #[command(long_about = LONG_CONNECTIONS)]
    Connections(ConnectionsArgs),
    /// Close all persistent connections.
    #[command(long_about = LONG_DISCONNECT_ALL)]
    DisconnectAll(DisconnectAllArgs),

    // ---- Phase 2 discovery ---------------------------------------------------
    /// Run discovery against a namespace and persist its profile.
    #[command(long_about = LONG_SETUP)]
    Setup(SetupArgs),
    /// Alias of `setup`.
    #[command(long_about = LONG_SETUP)]
    Discover(SetupArgs),
    /// Show the cached profile for a namespace.
    #[command(long_about = LONG_PROFILE)]
    Profile(ProfileArgs),

    // ---- Phase 4 read verbs --------------------------------------------------
    /// Show service inventory and health rollup.
    #[command(long_about = LONG_STATUS)]
    Status(StatusArgs),
    /// Detailed health checks.
    #[command(long_about = LONG_HEALTH)]
    Health(HealthArgs),
    /// Tail or view container logs.
    #[command(long_about = LONG_LOGS)]
    Logs(LogsArgs),
    /// Search content in logs or files.
    #[command(long_about = LONG_GREP)]
    Grep(GrepArgs),
    /// Read a file.
    #[command(long_about = LONG_CAT)]
    Cat(CatArgs),
    /// List directory contents.
    #[command(long_about = LONG_LS)]
    Ls(LsArgs),
    /// Find files by pattern.
    #[command(long_about = LONG_FIND)]
    Find(FindArgs),
    /// List running containers.
    #[command(long_about = LONG_PS)]
    Ps(PsArgs),
    /// List volumes.
    #[command(long_about = LONG_SIMPLE_SELECTOR)]
    Volumes(SimpleSelectorArgs),
    /// List images.
    #[command(long_about = LONG_SIMPLE_SELECTOR)]
    Images(SimpleSelectorArgs),
    /// List networks.
    #[command(long_about = LONG_SIMPLE_SELECTOR)]
    Network(SimpleSelectorArgs),
    /// List listening ports.
    #[command(long_about = LONG_SIMPLE_SELECTOR)]
    Ports(PortsArgs),
    /// Diagnostic walk for a service.
    #[command(long_about = LONG_WHY)]
    Why(WhyArgs),
    /// Connectivity matrix.
    #[command(long_about = LONG_CONNECTIVITY)]
    Connectivity(ConnectivityArgs),
    /// Run a multi-step diagnostic recipe.
    #[command(long_about = LONG_RECIPE)]
    Recipe(RecipeArgs),

    // ---- Phase 6/7 search ----------------------------------------------------
    /// LogQL search across mediums and namespaces.
    #[command(long_about = LONG_SEARCH)]
    Search(SearchArgs),

    // ---- Phase 5 write verbs -------------------------------------------------
    /// Restart container(s).
    #[command(long_about = LONG_LIFECYCLE)]
    Restart(LifecycleArgs),
    /// Stop container(s).
    #[command(long_about = LONG_LIFECYCLE)]
    Stop(LifecycleArgs),
    /// Start container(s).
    #[command(long_about = LONG_LIFECYCLE)]
    Start(LifecycleArgs),
    /// Reload service(s) (SIGHUP).
    #[command(long_about = LONG_LIFECYCLE)]
    Reload(LifecycleArgs),
    /// Copy files between local and remote.
    #[command(long_about = LONG_CP)]
    Cp(CpArgs),
    /// Sed-style content edit.
    #[command(long_about = LONG_EDIT)]
    Edit(EditArgs),
    /// Delete file.
    #[command(long_about = LONG_PATH_ARG)]
    Rm(PathArgArgs),
    /// Create directory.
    #[command(long_about = LONG_PATH_ARG)]
    Mkdir(PathArgArgs),
    /// Create empty file.
    #[command(long_about = LONG_PATH_ARG)]
    Touch(PathArgArgs),
    /// Change file mode.
    #[command(long_about = LONG_CHMOD)]
    Chmod(ChmodArgs),
    /// Change file ownership.
    #[command(long_about = LONG_CHOWN)]
    Chown(ChownArgs),
    /// Run a state-changing command on a target. Audited.
    #[command(long_about = LONG_EXEC)]
    Exec(ExecArgs),
    /// Run a read-only command on a target. Not audited; secrets masked.
    #[command(long_about = LONG_RUN)]
    Run(RunArgs),

    /// Block until a predicate over the target becomes true (B10).
    #[command(long_about = LONG_WATCH)]
    Watch(WatchArgs),

    // ---- Phase 3 alias management --------------------------------------------
    /// Manage selector aliases.
    #[command(long_about = LONG_ALIAS)]
    Alias(AliasArgs),

    /// Resolve a selector against discovered profiles and print the targets.
    /// Useful for testing selector grammar before the read/write verbs land.
    #[command(long_about = LONG_RESOLVE)]
    Resolve(ResolveArgs),

    // ---- Phase 5 audit + revert ----------------------------------------------
    /// Inspect or query the local audit log.
    #[command(long_about = LONG_AUDIT)]
    Audit(AuditArgs),
    /// Revert a previous mutation by audit id.
    #[command(long_about = LONG_REVERT)]
    Revert(RevertArgs),

    // ---- v0.1.3 F8 cache management ------------------------------------------
    /// Inspect or invalidate the runtime cache.
    #[command(long_about = LONG_CACHE)]
    Cache(CacheArgs),

    // ---- Phase 11 fleet ------------------------------------------------------
    /// Run a verb across multiple namespaces.
    #[command(long_about = LONG_FLEET)]
    Fleet(FleetArgs),

    // ---- v0.1.2 B9 bundle ----------------------------------------------------
    /// YAML-driven multi-step orchestration with rollback.
    #[command(long_about = LONG_BUNDLE)]
    Bundle(BundleArgs),

    // ---- Help system (HP-0) -------------------------------------------------
    /// Show help on a topic, search help, or list all topics.
    #[command(long_about = LONG_HELP)]
    Help(HelpArgs),
}

#[derive(Debug, Args)]
#[command(
    long_about = "Show in-binary documentation. Run with no topic to see the \
topic + command index, or with a topic name for the full prose. Use \
`--search` (HP-3) to find help by keyword and `--json` (HP-4) to get the \
full machine-readable surface.\n\n\
EXAMPLES\n  \
  $ inspect help\n  \
  $ inspect help quickstart\n  \
  $ inspect help all",
    after_help = SEE_ALSO_HELP,
)]
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
#[command(
    long_about = "Register or update a namespace's SSH credentials. Idempotent: \
running `add` again with new flags rewrites the entry.\n\n\
EXAMPLES\n  \
  $ inspect add arte\n  \
  $ inspect add prod-eu --host prod-eu.example.com --user ops --key-path ~/.ssh/prod\n  \
  $ inspect add staging --non-interactive --force",
    after_help = SEE_ALSO_DISCOVER,
)]
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
#[command(
    long_about = "Print every configured namespace with its host, user, and \
last-known reachability.\n\n\
EXAMPLES\n  \
  $ inspect list\n  \
  $ inspect list --json\n  \
  $ inspect list --csv",
    after_help = SEE_ALSO_DISCOVER,
)]
pub struct ListArgs {
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
#[command(
    long_about = "Delete a namespace's stored credentials and cached profile. \
The SSH key file itself is never touched.\n\n\
EXAMPLES\n  \
  $ inspect remove staging\n  \
  $ inspect remove prod-eu --yes",
    after_help = SEE_ALSO_DISCOVER,
)]
pub struct RemoveArgs {
    /// Namespace to remove.
    pub namespace: String,

    /// Skip confirmation prompt.
    #[arg(long, short)]
    pub yes: bool,
}

#[derive(Debug, Args)]
#[command(
    long_about = "Validate a namespace's configuration: env vars resolve, key \
file is readable, host is reachable. Does not run discovery.\n\n\
EXAMPLES\n  \
  $ inspect test arte\n  \
  $ inspect test prod-eu --json",
    after_help = SEE_ALSO_DISCOVER,
)]
pub struct TestArgs {
    /// Namespace to test.
    pub namespace: String,

    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
#[command(
    long_about = "Print a namespace's resolved configuration with secrets \
redacted. Use `--profile` to see the cached discovery profile.\n\n\
EXAMPLES\n  \
  $ inspect show arte\n  \
  $ inspect show arte --json\n  \
  $ inspect show arte --profile",
    after_help = SEE_ALSO_DISCOVER,
)]
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
#[command(
    long_about = "Run an inner verb across multiple namespaces selected by \
`--ns` (glob, comma-list, or `@group`). Results stream as they arrive; \
failed namespaces appear with error rows but do not abort the run unless \
`--abort-on-error` is set.\n\n\
EXAMPLES\n  \
  $ inspect fleet --ns 'prod-*' status\n  \
  $ inspect fleet --ns @prod restart pulse --apply\n  \
  $ inspect fleet --ns 'prod-*' --canary 1 restart pulse --apply",
    after_help = SEE_ALSO_FLEET,
)]
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
#[command(
    long_about = "Diagnostic walk for a service: status, recent errors, \
health, and connectivity from one selector. The built-in `why` recipe.\n\n\
EXAMPLES\n  \
  $ inspect why arte/atlas\n  \
  $ inspect why 'prod-*/storage' --json",
    after_help = SEE_ALSO_RECIPES,
)]
pub struct WhyArgs {
    /// Selector resolving to one or more services to diagnose.
    pub selector: String,
    /// F8 (v0.1.3): bypass the runtime cache and re-fetch live state
    /// before walking the dependency graph. Removes the post-restart
    /// "reads as still unhealthy" symptom. `--live` is an alias.
    #[arg(long, alias = "live")]
    pub refresh: bool,
    /// F4 (v0.1.3): suppress the diagnostic bundle (recent logs +
    /// effective Cmd/Entrypoint + port reality) attached to
    /// unhealthy / down / restart-looping containers. Restores the
    /// v0.1.2 terse output for agents that already drive the deeper
    /// queries themselves.
    #[arg(long)]
    pub no_bundle: bool,
    /// F4 (v0.1.3): tail size for the recent-logs section of the
    /// diagnostic bundle. Default 20, hard-capped at 200 (anything
    /// above is clamped with a one-line stderr notice — protects
    /// the operator from accidentally pulling 50k lines through
    /// redaction).
    #[arg(long, default_value_t = 20)]
    pub log_tail: u32,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
#[command(
    long_about = "Print the connectivity matrix for the selected services. \
With `--probe`, live-test each declared edge with bash /dev/tcp.\n\n\
EXAMPLES\n  \
  $ inspect connectivity arte\n  \
  $ inspect connectivity arte/atlas --probe\n  \
  $ inspect connectivity 'prod-*' --json",
    after_help = SEE_ALSO_RECIPES,
)]
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
#[command(
    long_about = "Run a multi-step diagnostic or remediation recipe. Built-in \
recipes are listed in `inspect help recipes`. User recipes live under \
~/.inspect/recipes/<name>.yaml. Mutating recipes require `--apply`.\n\n\
EXAMPLES\n  \
  $ inspect recipe deploy-check arte\n  \
  $ inspect recipe disk-audit 'prod-*'\n  \
  $ inspect recipe cycle-atlas --sel arte/atlas --apply",
    after_help = SEE_ALSO_RECIPES,
)]
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
#[command(
    long_about = "LogQL query across logs, files, and discovery sources. \
Queries are always single-quoted. The pipeline is the LogQL DSL, not the \
shell.\n\n\
EXAMPLES\n  \
  $ inspect search '{server=\"arte\", source=\"logs\"} |= \"error\"' --since 1h\n  \
  $ inspect search '{server=~\"prod-.*\", service=\"storage\", source=\"logs\"} |= \"timeout\"'\n  \
  $ inspect search 'sum by (service) (count_over_time({server=\"arte\", source=\"logs\"} |= \"error\" [5m]))'",
    after_help = SEE_ALSO_SEARCH,
)]
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
#[command(
    long_about = "Open a persistent SSH session for a namespace. The first \
call prompts for the key passphrase (if encrypted); subsequent commands \
reuse the session via a control socket until the TTL expires.\n\n\
EXAMPLES\n  \
  $ inspect connect arte\n  \
  $ inspect connect prod-eu --ttl 4h\n  \
  $ inspect connect arte --non-interactive",
    after_help = SEE_ALSO_CONNECT,
)]
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
#[command(
    long_about = "Close the persistent SSH session for one namespace. Does \
not affect user-managed ControlMaster sockets in ~/.ssh/config.\n\n\
EXAMPLES\n  \
  $ inspect disconnect arte\n  \
  $ inspect disconnect prod-eu --json",
    after_help = SEE_ALSO_SSH,
)]
pub struct DisconnectArgs {
    /// Namespace to disconnect.
    pub namespace: String,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
#[command(
    long_about = "List active inspect-managed SSH sessions, with TTL \
remaining and the path to each control socket.\n\n\
EXAMPLES\n  \
  $ inspect connections\n  \
  $ inspect connections --json\n  \
  $ inspect connections --csv",
    after_help = SEE_ALSO_SSH,
)]
pub struct ConnectionsArgs {
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
#[command(
    long_about = "Close every active inspect-managed SSH session. Prompts \
for confirmation unless `--yes` is passed.\n\n\
EXAMPLES\n  \
  $ inspect disconnect-all\n  \
  $ inspect disconnect-all --yes",
    after_help = SEE_ALSO_SSH,
)]
pub struct DisconnectAllArgs {
    /// Skip confirmation prompt.
    #[arg(long, short)]
    pub yes: bool,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
#[command(
    long_about = "Run discovery against a namespace and persist its profile \
(containers, volumes, networks, listeners, remote tooling). Cached \
profiles live under ~/.inspect/profiles/.\n\n\
EXAMPLES\n  \
  $ inspect setup arte\n  \
  $ inspect setup prod-eu --force\n  \
  $ inspect setup arte --check-drift",
    after_help = SEE_ALSO_DISCOVER,
)]
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
    /// P13: re-probe only the services flagged `discovery_incomplete`
    /// in the cached profile (i.e. those whose `docker inspect` timed
    /// out on the previous run). Cheaper than `--force` when only one
    /// or two containers are wedged.
    #[arg(long, conflicts_with_all = ["force", "check_drift"])]
    pub retry_failed: bool,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
#[command(
    long_about = "Print the cached discovery profile for a namespace.\n\n\
EXAMPLES\n  \
  $ inspect profile arte\n  \
  $ inspect profile arte --json\n  \
  $ inspect profile prod-eu --yaml",
    after_help = SEE_ALSO_DISCOVER,
)]
pub struct ProfileArgs {
    /// Namespace whose profile to display.
    pub namespace: String,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

// ---- Phase 3 -----------------------------------------------------------------

#[derive(Debug, Args)]
#[command(
    long_about = "Manage saved selector aliases (`@name`). Subcommands: add, \
list, remove, show.\n\n\
EXAMPLES\n  \
  $ inspect alias add plogs '{server=\"arte\", service=\"pulse\", source=\"logs\"}'\n  \
  $ inspect alias add storage-prod 'prod-*/storage'\n  \
  $ inspect alias list",
    after_help = SEE_ALSO_ALIAS,
)]
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
#[command(
    long_about = "Resolve a selector against discovered profiles and print \
the target list. Useful to test selector grammar before a real verb call.\n\n\
EXAMPLES\n  \
  $ inspect resolve arte/storage\n  \
  $ inspect resolve 'prod-*/atlas'\n  \
  $ inspect resolve @plogs",
    after_help = SEE_ALSO_RESOLVE,
)]
pub struct ResolveArgs {
    /// Selector text (e.g. `arte/pulse`, `prod-*/storage`, `@plogs`).
    pub selector: String,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

// ---- Phase 4 read verbs ------------------------------------------------------

#[derive(Debug, Args)]
#[command(
    long_about = "Generic listing for ports / volumes / images / network. \
The verb name selects which medium to list.\n\n\
EXAMPLES\n  \
  $ inspect volumes arte\n  \
  $ inspect images arte/atlas\n  \
  $ inspect ports 'prod-*'",
    after_help = SEE_ALSO_READ,
)]
pub struct SimpleSelectorArgs {
    /// Selector (server, server/service, etc.).
    pub selector: String,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
#[command(
    long_about = "List listening ports. Filter to a single port with \
`--port <n>` or a range with `--port-range <lo-hi>`.\n\n\
EXAMPLES\n  \
  $ inspect ports arte\n  \
  $ inspect ports arte --port 8200\n  \
  $ inspect ports 'prod-*' --port-range 8000-9000",
    after_help = SEE_ALSO_READ,
)]
pub struct PortsArgs {
    /// Selector (server, server/service, etc.).
    pub selector: String,
    /// F7.3 (v0.1.3): server-side filter to a single port. The row's
    /// `:<n>` token in the host- or container-port axis must equal
    /// `<n>` exactly. Mutually exclusive with `--port-range`.
    #[arg(long, conflicts_with = "port_range")]
    pub port: Option<u16>,
    /// F7.3 (v0.1.3): server-side filter to an inclusive port range
    /// `<lo>-<hi>`. Mutually exclusive with `--port`.
    #[arg(long, value_name = "LO-HI")]
    pub port_range: Option<String>,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
#[command(
    long_about = "Service inventory and health rollup for the selected \
targets.\n\n\
EXAMPLES\n  \
  $ inspect status arte\n  \
  $ inspect status arte/atlas\n  \
  $ inspect status 'prod-*' --json",
    after_help = SEE_ALSO_READ,
)]
pub struct StatusArgs {
    /// Selector (server, server/service, etc.).
    pub selector: String,
    /// F8 (v0.1.3): bypass the runtime cache and re-fetch live state
    /// before answering. Use after a mutation to confirm the change
    /// took effect. `--live` is an alias.
    #[arg(long, alias = "live")]
    pub refresh: bool,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
#[command(
    long_about = "Detailed health checks for the selected services. Calls \
each service's declared health endpoint and reports per-check status.\n\n\
EXAMPLES\n  \
  $ inspect health arte\n  \
  $ inspect health arte/atlas --json\n  \
  $ inspect health 'prod-*/storage'",
    after_help = SEE_ALSO_READ,
)]
pub struct HealthArgs {
    pub selector: String,
    /// F8 (v0.1.3): bypass the runtime cache and re-fetch live state
    /// before probing. Use after a mutation to confirm health
    /// recovered. `--live` is an alias.
    #[arg(long, alias = "live")]
    pub refresh: bool,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
#[command(
    long_about = "List containers on the selected targets.\n\n\
EXAMPLES\n  \
  $ inspect ps arte\n  \
  $ inspect ps arte --all\n  \
  $ inspect ps 'prod-*' --json",
    after_help = SEE_ALSO_READ,
)]
pub struct PsArgs {
    pub selector: String,
    /// Show all containers (default shows just running).
    #[arg(short = 'a', long)]
    pub all: bool,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Clone, Args)]
#[command(
    long_about = "Tail or view container logs. With `--follow`, streams new \
records until interrupted; the inner SSH session auto-resumes on transient \
failures.\n\n\
EXAMPLES\n  \
  $ inspect logs arte/pulse --since 30m\n  \
  $ inspect logs arte/atlas --tail 200 --follow\n  \
  $ inspect logs 'prod-*/storage' --since 1h --json",
    after_help = SEE_ALSO_READ,
)]
pub struct LogsArgs {
    pub selector: String,
    /// Show logs since duration (e.g. 30s, 5m, 1h, 2d).
    #[arg(long, conflicts_with = "since_last")]
    pub since: Option<String>,
    /// Resume from the last `--since-last` cursor for this
    /// (namespace, service). On a cold start (no cursor yet) falls back
    /// to the duration in `INSPECT_SINCE_LAST_DEFAULT` (default `5m`).
    /// Cursors live in `~/.inspect/cursors/` (mode 0600).
    #[arg(long)]
    pub since_last: bool,
    /// Delete the `--since-last` cursor for this (namespace, service)
    /// and exit. Idempotent.
    #[arg(long)]
    pub reset_cursor: bool,
    /// Show logs until duration.
    #[arg(long)]
    pub until: Option<String>,
    /// Number of lines from the tail.
    #[arg(long)]
    pub tail: Option<u64>,
    /// Stream logs.
    #[arg(short = 'f', long)]
    pub follow: bool,
    /// Merge logs from multiple matched services into a single stream
    /// with `[svc]` prefixes. In batch mode (no `--follow`) lines are
    /// k-way-merged by timestamp; in follow mode they arrive in
    /// observed order (clock-skew caveat applies).
    #[arg(long)]
    pub merged: bool,
    /// Server-side regex filter: keep only lines matching this regex.
    /// Repeat the flag to OR multiple patterns. Pushed down to the
    /// remote host as a `grep -E` pipeline; in `--follow` mode uses
    /// `grep --line-buffered`.
    #[arg(long = "match", short = 'g', value_name = "REGEX")]
    pub match_re: Vec<String>,
    /// Server-side regex filter: drop lines matching this regex.
    /// Repeat to OR multiple patterns. Applied after `--match`.
    #[arg(long = "exclude", short = 'G', value_name = "REGEX")]
    pub exclude_re: Vec<String>,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
    /// Hidden: ssh-side timeout for follow mode (seconds).
    #[arg(long, hide = true)]
    pub follow_timeout_secs: Option<u64>,
}

#[derive(Debug, Args)]
#[command(
    long_about = "Search content in logs or files on the selected targets. \
Selector may include `:path` to grep a specific file. Smart-case by \
default.\n\n\
EXAMPLES\n  \
  $ inspect grep \"error\" arte/pulse --since 1h\n  \
  $ inspect grep -i \"timeout\" 'prod-*/storage' --since 30m\n  \
  $ inspect grep \"milvus\" arte/atlas:/var/log/atlas.log",
    after_help = SEE_ALSO_READ,
)]
pub struct GrepArgs {
    /// Pattern to search for.
    pub pattern: String,
    /// Selector. May include `:path` to grep a file.
    pub selector: String,
    #[arg(long, conflicts_with = "since_last")]
    pub since: Option<String>,
    /// Resume from the last `--since-last` cursor for this
    /// (namespace, service). See `inspect logs --since-last`.
    #[arg(long)]
    pub since_last: bool,
    /// Delete the `--since-last` cursor for this (namespace, service).
    #[arg(long)]
    pub reset_cursor: bool,
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

    /// Server-side regex filter applied AFTER the main grep stage:
    /// keep only lines matching this regex. Repeat to OR patterns.
    #[arg(long = "match", short = 'g', value_name = "REGEX")]
    pub match_re: Vec<String>,
    /// Server-side regex filter: drop lines matching this regex.
    /// Repeat to OR patterns. Applied after `--match`.
    #[arg(long = "exclude", short = 'G', value_name = "REGEX")]
    pub exclude_re: Vec<String>,

    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
#[command(
    long_about = "Print the contents of a file inside a container or on the \
host.\n\n\
EXAMPLES\n  \
  $ inspect cat arte/atlas:/etc/atlas.conf\n  \
  $ inspect cat arte/_:/var/log/syslog\n  \
  $ inspect cat arte/pulse:/etc/pulse.conf --raw",
    after_help = SEE_ALSO_READ,
)]
pub struct CatArgs {
    /// Selector with `:path` (e.g. `arte/atlas:/etc/atlas.conf`).
    pub target: String,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
#[command(
    long_about = "List directory contents on a target.\n\n\
EXAMPLES\n  \
  $ inspect ls arte/atlas:/etc\n  \
  $ inspect ls arte/_:/var/log -A\n  \
  $ inspect ls arte/pulse:/var/lib/pulse -l",
    after_help = SEE_ALSO_READ,
)]
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
#[command(
    long_about = "Find files by name pattern on a target. Wraps remote \
`find` and respects discovery's tooling probe.\n\n\
EXAMPLES\n  \
  $ inspect find arte/atlas:/etc \"*.conf\"\n  \
  $ inspect find arte/_:/var/log \"*.log\"\n  \
  $ inspect find 'prod-*/storage:/data'",
    after_help = SEE_ALSO_READ,
)]
pub struct FindArgs {
    /// Selector with `:path`.
    pub target: String,
    /// Optional name pattern (find -name).
    pub pattern: Option<String>,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

// ---- Phase 5 write verbs -----------------------------------------------------

#[derive(Debug, Args)]
#[command(
    long_about = "Container lifecycle (the verb form chooses the action: \
restart / stop / start / reload). Dry-run by default; `--apply` executes.\n\n\
EXAMPLES\n  \
  $ inspect restart arte/pulse\n  \
  $ inspect restart arte/pulse --apply\n  \
  $ inspect stop 'prod-*/atlas' --apply --yes-all",
    after_help = SEE_ALSO_WRITE,
)]
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
    /// Free-form note recorded in the audit entry. Useful for change
    /// management tickets, incident IDs, or just "why did I run this?".
    /// Limited to 240 characters.
    #[arg(long, value_name = "TEXT")]
    pub reason: Option<String>,
    /// F11 (v0.1.3): print the captured inverse before applying. Lets
    /// the operator (or a driving agent) see exactly what
    /// `inspect revert <new-id>` will undo, before the mutation runs.
    #[arg(long)]
    pub revert_preview: bool,
}

#[derive(Debug, Args)]
#[command(
    long_about = "Run a state-changing command on the selected targets. Audited; \
`--apply` required to actually execute. For read-only inspection use \
`inspect run` instead -- it skips the audit log and the apply gate.\n\n\
EXAMPLES\n  \
  $ inspect exec arte/atlas --apply -- systemctl restart atlas\n  \
  $ inspect exec arte/atlas --apply -- 'touch /var/lib/atlas/.maint'\n  \
  $ inspect exec 'prod-*' --apply --yes-all -- 'rm -f /tmp/lockfile'",
    after_help = SEE_ALSO_WRITE,
)]
pub struct ExecArgs {
    /// Selector.
    pub selector: String,
    /// Command and arguments after `--`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub cmd: Vec<String>,
    #[arg(long)]
    pub apply: bool,
    #[arg(short = 'y', long)]
    pub yes: bool,
    #[arg(long)]
    pub yes_all: bool,
    /// Override the per-target timeout (seconds).
    #[arg(long)]
    pub timeout_secs: Option<u64>,
    /// Print KEY=VALUE secret values verbatim instead of masking them.
    /// Off by default so log captures and screenshots are safe.
    #[arg(long)]
    pub show_secrets: bool,
    /// Mask every line that looks like KEY=VALUE, regardless of key name.
    #[arg(long)]
    pub redact_all: bool,
    /// Free-form note recorded in the audit entry. Limited to 240 characters.
    #[arg(long, value_name = "TEXT")]
    pub reason: Option<String>,
    /// Emit a `[inspect] still running on <ns> (Ns elapsed)` line to
    /// stderr after this many seconds of remote silence (B7, v0.1.2).
    /// Defaults to 30s. Use `--no-heartbeat` to disable.
    #[arg(long, value_name = "SECS", conflicts_with = "no_heartbeat")]
    pub heartbeat: Option<u64>,
    /// Disable the live heartbeat. Streaming output is unaffected.
    #[arg(long)]
    pub no_heartbeat: bool,
    /// F11 (v0.1.3): acknowledge that this exec has no captured
    /// inverse. Required with `--apply`; without it, `inspect exec`
    /// refuses free-form mutations.
    #[arg(long)]
    pub no_revert: bool,
    /// F11 (v0.1.3): print the captured inverse before applying.
    /// For exec, this is always `revert.kind = unsupported`.
    #[arg(long)]
    pub revert_preview: bool,
}

#[derive(Debug, Args)]
#[command(
    long_about = LONG_RUN,
    after_help = SEE_ALSO_READ,
)]
pub struct RunArgs {
    /// Selector.
    pub selector: String,
    /// Command and arguments after `--`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub cmd: Vec<String>,
    /// Override the per-target timeout (seconds).
    #[arg(long)]
    pub timeout_secs: Option<u64>,
    /// Print KEY=VALUE secret values verbatim instead of masking them.
    /// Off by default so log captures and screenshots are safe.
    #[arg(long)]
    pub show_secrets: bool,
    /// Mask every line that looks like KEY=VALUE, regardless of the
    /// key name. Useful when the remote command emits config blobs
    /// you have not vetted.
    #[arg(long)]
    pub redact_all: bool,
    /// Server-side line filter (extended regex). Equivalent to piping
    /// the remote command through `grep -E <pattern>`. Quote shell
    /// metacharacters.
    #[arg(long, value_name = "REGEX")]
    pub filter_line_pattern: Option<String>,
    /// Free-form note. `inspect run` is not audited, so this is purely
    /// informational -- it is echoed once to stderr at the start of
    /// the run so the operator's terminal/shell history captures the
    /// intent. Limited to 240 characters.
    #[arg(long, value_name = "TEXT")]
    pub reason: Option<String>,
    /// Pass through every output line verbatim — disables the per-line
    /// byte cap that protects terminals from runaway 100KB+ JSON blobs.
    /// Use when you need full fidelity (e.g. capturing full SQL query
    /// output for a snapshot). Lines are still sanitized for ANSI/C0.
    #[arg(long)]
    pub no_truncate: bool,
    /// F9 (v0.1.3): refuse to forward local stdin to the remote
    /// command. If local stdin has data waiting (non-tty + readable)
    /// and this flag is set, `inspect run` exits 2 BEFORE dispatching
    /// the remote command — never silently discards input.
    #[arg(long, conflicts_with_all = ["stdin_max", "audit_stdin_hash"])]
    pub no_stdin: bool,
    /// F9 (v0.1.3): cap on forwarded stdin per invocation. Accepts a
    /// raw byte count or a k/m/g suffix (case-insensitive). Default
    /// 10m. Set to `0` to disable the cap entirely.
    #[arg(long, value_name = "SIZE")]
    pub stdin_max: Option<String>,
    /// F9 (v0.1.3): record `stdin_sha256` (hex SHA-256 of the
    /// forwarded payload) in the audit entry. Off by default for
    /// perf; opt-in for security-sensitive runs.
    #[arg(long)]
    pub audit_stdin_hash: bool,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
#[command(
    long_about = LONG_WATCH,
    after_help = SEE_ALSO_READ,
)]
pub struct WatchArgs {
    /// Selector. For `--until-cmd`/`--until-log`/`--until-sql`/`--until-http`
    /// this is the target the predicate is evaluated against.
    pub selector: String,

    // ---- predicate kinds (mutually exclusive, exactly one required) ----
    /// Block until CMD's stdout / exit code satisfies the comparator.
    /// Without a comparator, exit code 0 is treated as match.
    #[arg(
        long,
        value_name = "CMD",
        group = "predicate",
        conflicts_with_all = ["until_log", "until_sql", "until_http"],
    )]
    pub until_cmd: Option<String>,

    /// Block until PATTERN appears in `docker logs` after the watch
    /// started. Literal substring by default; pass `--regex` for ERE.
    #[arg(
        long,
        value_name = "PATTERN",
        group = "predicate",
        conflicts_with_all = ["until_cmd", "until_sql", "until_http"],
    )]
    pub until_log: Option<String>,

    /// Block until `psql -tAc <SQL>` returns truthy (t/true/1/yes after
    /// trim). Run inside the target container via `docker exec`. Use
    /// `--psql-opts` to pass `-U/-d/...`.
    #[arg(
        long,
        value_name = "SQL",
        group = "predicate",
        conflicts_with_all = ["until_cmd", "until_log", "until_http"],
    )]
    pub until_sql: Option<String>,

    /// Block until `curl -fsS <URL>` (run on the target host) satisfies
    /// `--match`. Without `--match`, any HTTP success (curl exit 0) is
    /// a match.
    #[arg(
        long,
        value_name = "URL",
        group = "predicate",
        conflicts_with_all = ["until_cmd", "until_log", "until_sql"],
    )]
    pub until_http: Option<String>,

    // ---- comparators (only valid with --until-cmd) ----
    /// Match if the trimmed cmd stdout equals VALUE.
    #[arg(long, value_name = "VALUE", requires = "until_cmd")]
    pub equals: Option<String>,
    /// Match if the cmd stdout matches REGEX (ERE).
    #[arg(long, value_name = "REGEX", requires = "until_cmd")]
    pub matches: Option<String>,
    /// Match if the trimmed cmd stdout (parsed as f64) is greater than N.
    #[arg(long, value_name = "N", requires = "until_cmd")]
    pub gt: Option<f64>,
    /// Match if the trimmed cmd stdout (parsed as f64) is less than N.
    #[arg(long, value_name = "N", requires = "until_cmd")]
    pub lt: Option<f64>,
    /// Match the first time the cmd stdout differs from the previous
    /// poll. Skips the first poll.
    #[arg(long, requires = "until_cmd")]
    pub changes: bool,
    /// Match when the cmd stdout has been the same for at least DUR
    /// (e.g. `30s`, `5m`).
    #[arg(long, value_name = "DUR", requires = "until_cmd")]
    pub stable_for: Option<String>,

    // ---- per-kind options ----
    /// Treat `--until-log <PATTERN>` as an extended regex instead of a
    /// literal substring.
    #[arg(long, requires = "until_log")]
    pub regex: bool,
    /// Extra args inserted between `psql` and the SQL flags (e.g.
    /// `-U postgres -d app`). Required when the container does not
    /// default to a usable PGUSER/PGDATABASE.
    #[arg(long, value_name = "OPTS", requires = "until_sql")]
    pub psql_opts: Option<String>,
    /// Predicate over the HTTP response. DSL: `<lhs> <op> <rhs>` where
    /// lhs ∈ {body, status, $.json.path}, op ∈ {==, !=, <, >, contains}.
    #[arg(long, value_name = "EXPR", requires = "until_http")]
    pub r#match: Option<String>,
    /// Disable TLS certificate verification for `--until-http`. Use
    /// only for self-signed staging endpoints; never against
    /// production. Maps to `curl --insecure`.
    #[arg(long, requires = "until_http")]
    pub insecure: bool,

    // ---- shared loop knobs ----
    /// Polling interval (default `2s`). Accepts `Ns/Nm/Nh/Nd`.
    #[arg(long, value_name = "DUR")]
    pub interval: Option<String>,
    /// Hard deadline (default `10m`). Accepts `Ns/Nm/Nh/Nd`. Use `0s`
    /// to disable. On timeout, `inspect watch` exits 124.
    #[arg(long, value_name = "DUR")]
    pub timeout: Option<String>,
    /// Free-form note recorded in the audit entry. Limited to 240
    /// characters.
    #[arg(long, value_name = "TEXT")]
    pub reason: Option<String>,
    /// One status line per poll (instead of in-place TTY rewrite).
    #[arg(long)]
    pub verbose: bool,
}

// ---------------------------------------------------------------------------
// B9 (v0.1.2) — `inspect bundle`
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
#[command(
    long_about = LONG_BUNDLE,
    after_help = SEE_ALSO_WRITE,
)]
pub struct BundleArgs {
    #[command(subcommand)]
    pub mode: BundleMode,
}

#[derive(Debug, Subcommand)]
pub enum BundleMode {
    /// Validate, interpolate vars/matrix, and print the rendered step
    /// list. No remote work.
    Plan(BundlePlanArgs),
    /// Run preflight + steps + postflight. Destructive steps require
    /// `--apply` unless they opt out (`apply: false`).
    Apply(BundleApplyArgs),
}

#[derive(Debug, Args)]
pub struct BundlePlanArgs {
    /// Path to the bundle YAML file.
    pub file: std::path::PathBuf,
}

#[derive(Debug, Args)]
pub struct BundleApplyArgs {
    /// Path to the bundle YAML file.
    pub file: std::path::PathBuf,

    /// Required for any bundle that contains a destructive `exec:`
    /// step. Without it, `apply` refuses up front.
    #[arg(long)]
    pub apply: bool,

    /// Skip the interactive "rollback completed steps?" prompt on
    /// failure / Ctrl-C. CI mode: rollback runs unconditionally.
    #[arg(long)]
    pub no_prompt: bool,

    /// Free-form note attached to every audit entry the bundle
    /// produces. Limited to 240 characters.
    #[arg(long, value_name = "TEXT")]
    pub reason: Option<String>,
}

#[derive(Debug, Args)]
#[command(
    long_about = "File operation on a target path (the verb form chooses: \
rm / mkdir / touch). Dry-run by default; `--apply` executes.\n\n\
EXAMPLES\n  \
  $ inspect rm arte/atlas:/tmp/stale.log --apply\n  \
  $ inspect mkdir arte/_:/var/log/inspect --apply\n  \
  $ inspect touch arte/atlas:/tmp/marker --apply",
    after_help = SEE_ALSO_WRITE,
)]
pub struct PathArgArgs {
    /// Selector with `:path`.
    pub target: String,
    #[arg(long)]
    pub apply: bool,
    #[arg(short = 'y', long)]
    pub yes: bool,
    #[arg(long)]
    pub yes_all: bool,
    /// Free-form note recorded in the audit entry. Limited to 240 characters.
    #[arg(long, value_name = "TEXT")]
    pub reason: Option<String>,
    /// F11 (v0.1.3): print the captured inverse before applying.
    #[arg(long)]
    pub revert_preview: bool,
}

#[derive(Debug, Args)]
#[command(
    long_about = "Change file mode (octal or symbolic). Dry-run by default; \
`--apply` executes.\n\n\
EXAMPLES\n  \
  $ inspect chmod arte/atlas:/etc/atlas.conf 0644\n  \
  $ inspect chmod arte/atlas:/etc/atlas.conf 0644 --apply\n  \
  $ inspect chmod arte/atlas:/usr/local/bin/atlas u+x --apply",
    after_help = SEE_ALSO_WRITE,
)]
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
    /// Free-form note recorded in the audit entry. Limited to 240 characters.
    #[arg(long, value_name = "TEXT")]
    pub reason: Option<String>,
    /// F11 (v0.1.3): print the captured inverse before applying.
    #[arg(long)]
    pub revert_preview: bool,
}

#[derive(Debug, Args)]
#[command(
    long_about = "Change file ownership (`user[:group]`). Dry-run by default; \
`--apply` executes.\n\n\
EXAMPLES\n  \
  $ inspect chown arte/atlas:/etc/atlas.conf atlas\n  \
  $ inspect chown arte/atlas:/etc/atlas.conf atlas:atlas --apply\n  \
  $ inspect chown arte/_:/var/log/atlas root:adm --apply",
    after_help = SEE_ALSO_WRITE,
)]
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
    /// Free-form note recorded in the audit entry. Limited to 240 characters.
    #[arg(long, value_name = "TEXT")]
    pub reason: Option<String>,
    /// F11 (v0.1.3): print the captured inverse before applying.
    #[arg(long)]
    pub revert_preview: bool,
}

#[derive(Debug, Args)]
#[command(
    long_about = "Copy a file between local and remote (push or pull, \
depending on which side carries `<sel>:<path>`). Dry-run by default; \
`--diff` shows a unified diff before `--apply`.\n\n\
EXAMPLES\n  \
  $ inspect cp ./fix.conf arte/pulse:/etc/pulse.conf --diff\n  \
  $ inspect cp ./fix.conf arte/pulse:/etc/pulse.conf --apply\n  \
  $ inspect cp arte/atlas:/var/log/atlas.log ./atlas.log",
    after_help = SEE_ALSO_WRITE,
)]
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
    /// Free-form note recorded in the audit entry. Limited to 240 characters.
    #[arg(long, value_name = "TEXT")]
    pub reason: Option<String>,
    /// F11 (v0.1.3): print the captured inverse before applying.
    #[arg(long)]
    pub revert_preview: bool,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
#[command(
    long_about = "In-place sed-style content edit (atomic). Dry-run by \
default — shows a unified diff. `--apply` writes.\n\n\
EXAMPLES\n  \
  $ inspect edit arte/atlas:/etc/atlas.conf 's/timeout=30/timeout=60/'\n  \
  $ inspect edit arte/atlas:/etc/atlas.conf 's/timeout=30/timeout=60/' --apply\n  \
  $ inspect edit '*/atlas:/etc/atlas.conf' 's|debug=on|debug=off|' --apply --yes-all",
    after_help = SEE_ALSO_WRITE,
)]
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
    /// Free-form note recorded in the audit entry. Limited to 240 characters.
    #[arg(long, value_name = "TEXT")]
    pub reason: Option<String>,
    /// F11 (v0.1.3): print the captured inverse before applying.
    #[arg(long)]
    pub revert_preview: bool,
}

#[derive(Debug, Args)]
#[command(
    long_about = "Inspect or query the local audit log. Subcommands: ls, \
show, grep, verify.\n\n\
EXAMPLES\n  \
  $ inspect audit ls\n  \
  $ inspect audit show <id>\n  \
  $ inspect audit grep \"atlas\"",
    after_help = SEE_ALSO_SAFETY,
)]
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
    /// Filter to entries whose `reason` field contains this substring
    /// (case-insensitive).
    #[arg(long, value_name = "PATTERN")]
    pub reason: Option<String>,
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

// ---- F8 cache management ----------------------------------------------------

#[derive(Debug, Args)]
#[command(
    long_about = LONG_CACHE,
)]
pub struct CacheArgs {
    #[command(subcommand)]
    pub command: CacheCommand,
}

#[derive(Debug, Subcommand)]
pub enum CacheCommand {
    /// List cached namespaces with their runtime/inventory ages.
    Show(CacheShowArgs),
    /// Delete cached runtime snapshot(s).
    Clear(CacheClearArgs),
}

#[derive(Debug, Args)]
pub struct CacheShowArgs {
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct CacheClearArgs {
    /// Namespace whose runtime snapshot to delete. Mutually exclusive
    /// with `--all`.
    pub namespace: Option<String>,
    /// Clear every cached namespace.
    #[arg(long)]
    pub all: bool,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
#[command(
    long_about = "Revert a previous mutation by audit id. Dry-run by default \
(shows the reverse diff); `--apply` restores the original content. \
Refuses if the file changed since the recorded mutation unless `--force`.\n\n\
EXAMPLES\n  \
  $ inspect revert <audit-id>\n  \
  $ inspect revert <audit-id> --apply\n  \
  $ inspect revert <audit-id> --apply --force",
    after_help = SEE_ALSO_SAFETY,
)]
pub struct RevertArgs {
    /// Audit id (or unique prefix). Optional when `--last` is given.
    pub audit_id: Option<String>,
    #[arg(long)]
    pub apply: bool,
    /// Override the drift check (current remote != recorded new_hash).
    #[arg(long)]
    pub force: bool,
    #[arg(short = 'y', long)]
    pub yes: bool,
    #[arg(long)]
    pub yes_all: bool,
    /// F11 (v0.1.3): revert the N most recent applied write entries
    /// (in reverse chronological order). Stops on the first
    /// `revert.kind = unsupported` entry with a loud explanation.
    /// Mutually exclusive with `<audit-id>`.
    #[arg(long, value_name = "N", num_args = 0..=1, default_missing_value = "1", conflicts_with = "audit_id")]
    pub last: Option<usize>,
}

#[cfg(test)]
mod hp2_cross_check {
    //! HP-2 cross-check: the literal `SEE_ALSO_*` constants in this
    //! module must stay in lock-step with `help::topics::see_also_line`,
    //! the runtime helper consumed by `inspect help --json` (HP-4).
    //!
    //! If a verb's topic mapping changes in `help::topics::VERB_TOPICS`
    //! and the matching `SEE_ALSO_*` literal here is not updated, this
    //! test fires and names the offending verb.

    use crate::help::topics::see_also_line;

    #[track_caller]
    fn assert_match(verb: &str, literal: &str) {
        let expected = see_also_line(verb);
        assert_eq!(
            literal, expected,
            "SEE_ALSO_* literal for {verb:?} drifted from VERB_TOPICS"
        );
    }

    #[test]
    fn read_cluster_matches_registry() {
        // Every read verb shares SEE_ALSO_READ; one representative
        // is enough — VERB_TOPICS guarantees siblings agree.
        assert_match("status", super::SEE_ALSO_READ);
        assert_match("grep", super::SEE_ALSO_READ);
        assert_match("logs", super::SEE_ALSO_READ);
    }

    #[test]
    fn write_cluster_matches_registry() {
        assert_match("restart", super::SEE_ALSO_WRITE);
        assert_match("edit", super::SEE_ALSO_WRITE);
        assert_match("cp", super::SEE_ALSO_WRITE);
        assert_match("exec", super::SEE_ALSO_WRITE);
    }

    #[test]
    fn other_clusters_match_registry() {
        assert_match("resolve", super::SEE_ALSO_RESOLVE);
        assert_match("search", super::SEE_ALSO_SEARCH);
        assert_match("audit", super::SEE_ALSO_SAFETY);
        assert_match("revert", super::SEE_ALSO_SAFETY);
        assert_match("fleet", super::SEE_ALSO_FLEET);
        assert_match("why", super::SEE_ALSO_RECIPES);
        assert_match("recipe", super::SEE_ALSO_RECIPES);
        assert_match("connectivity", super::SEE_ALSO_RECIPES);
        assert_match("setup", super::SEE_ALSO_DISCOVER);
        assert_match("add", super::SEE_ALSO_DISCOVER);
        assert_match("connect", super::SEE_ALSO_CONNECT);
        assert_match("disconnect", super::SEE_ALSO_SSH);
        assert_match("connections", super::SEE_ALSO_SSH);
        assert_match("disconnect-all", super::SEE_ALSO_SSH);
        assert_match("alias", super::SEE_ALSO_ALIAS);
        assert_match("help", super::SEE_ALSO_HELP);
    }
}
