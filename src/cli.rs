//! CLI command-tree definitions.
//!
//! In Phase 0, only the namespace lifecycle commands (`add`, `list`, `remove`,
//! `test`, `show`) carry real implementations. All other verbs from the bible
//! are scaffolded here so the surface is stable and future phases can fill
//! them in without breaking flag layouts.

use clap::{ArgGroup, Args, Parser, Subcommand};

const LONG_ABOUT: &str = "\
inspect — operational debugging CLI for cross-server search and safe hot-fix \
application.

COMMON VERBS
  $ inspect run arte 'docker ps -a'              one-shot remote command (read-only)
  $ inspect exec arte/atlas --apply -- systemctl restart atlas
                                                 audited mutation
  $ inspect logs arte/atlas-vault --since 5m --match 'panic'
                                                 tail + filter without 'inspect run -- docker logs'
  $ inspect status arte                          one-line health rollup
  $ inspect why arte/atlas-vault                 deep diagnostic walk

DIAGNOSTIC + READ VERBS
  $ inspect ps / health / cat / grep / find / ls / ports / volumes / network / images

WRITE + LIFECYCLE VERBS
  $ inspect restart / stop / start / reload / put / get / cp / edit / rm / chmod / chown

For the full list run `inspect --help` (each verb), or `inspect help <topic>` for editorial guides.
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
// F6 (v0.1.3): the compose verb cluster cross-links into the safety
// + write topics for the audited `compose restart`, the formats topic
// for the per-sub `--json` schemas, and the dedicated `compose`
// editorial topic for the deferred-verb policy.
const SEE_ALSO_COMPOSE: &str =
    "See also: inspect help compose, inspect help safety, inspect help formats";

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
  $ inspect why 'prod-*/storage' --json

REDACTION (L7, v0.1.3)
  The F4 deep-bundle log tail attached on unhealthy targets runs
  through the same four-masker pipeline as `inspect logs` (PEM
  blocks → marker, `Authorization` / `Cookie` headers → `<redacted>`,
  URL credentials → `user:****@host`, secret-shaped `KEY=VALUE` →
  `head4****tail2`). `inspect why` does not currently expose
  `--show-secrets` because the deep-bundle is a diagnostic
  affordance, not a privileged read; for a verbatim tail run
  `inspect logs <ns>/<svc> --tail N --show-secrets` directly. See
  `inspect help safety` for the redaction model.";

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
MODEL:    LogQL query across the profile-side index of logs / files / discovery sources; client-evaluated against indexed snapshots, not a live remote 'grep'.
EXAMPLE:  inspect search '{server=\"arte\", source=\"logs\"} |= \"error\"' --since 1h
NOTE:     for a live one-shot 'grep -r' against a single target path, use 'inspect grep' instead.

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

const LONG_SSH: &str = "\
SSH-related management subcommands.

SUBCOMMANDS
  add-key  Install a public key on the namespace's remote host \
(generating one if needed) and optionally flip the namespace from \
`auth = \"password\"` to `auth = \"key\"`. The audited migration \
path off password auth (L4, v0.1.3).

ADD-KEY USAGE
  inspect ssh add-key <ns>            # dry-run preview
  inspect ssh add-key <ns> --apply    # generate + install + offer config flip
  inspect ssh add-key <ns> --apply --key ~/.ssh/id_ed25519
  inspect ssh add-key <ns> --apply --no-rewrite-config

DEFAULTS
  Without `--key`, the helper generates a fresh ed25519 keypair at \
  `~/.ssh/inspect_<ns>_ed25519` (passphrase-less; protected by file mode \
  0600). With `--key <path>`, that key is used as-is and never \
  regenerated. Public-key install is idempotent — running twice does \
  not duplicate the line in the remote `~/.ssh/authorized_keys`.

CONFIG FLIP
  After installing the public key, the helper offers to rewrite the \
  namespace's entry in `~/.inspect/servers.toml` to `auth = \"key\"` \
  with `key_path = \"<path>\"`. `--no-rewrite-config` skips the \
  prompt. The flip drops `password_env` and `session_ttl` so the \
  namespace falls back to the default key-auth TTL (30m local / 4h \
  codespace) — re-set them explicitly if you need a longer key-auth \
  session.

AUDIT
  Every successful run writes one audit entry: \
  `verb=ssh.add-key, target=<ns>`, with bracketed args \
  `[key_path=...] [generated=true|false] [installed=true] \
  [config_rewritten=true|false]`. `revert.kind=command_pair` pointing \
  at a manual remove from `authorized_keys` (`inspect` does not \
  attempt to revoke the key automatically — that requires further \
  operator intent).

EXIT CODES
  0   ok
  1   no matching namespace / install verification failed
  2   argument/usage error (e.g. `--key` path does not exist)";

const LONG_KEYCHAIN: &str = "\
OS keychain management — opt-in, cross-session passphrase / password \
persistence (L2, v0.1.3).

WHAT THIS IS FOR
  Most operators want exactly the v0.1.2 behavior: ssh-agent (or \
inspect's own ControlMaster) holds the credential for the life of \
the shell session; logout / reboot clears it; the next session \
prompts once again. That is the secure default and remains the \
recommended path. The keychain is for the smaller group of \
operators who want passphrases (or passwords, after L4) to survive \
a reboot without leaving them in env vars, .envrc files, or shell \
history.

OPT-IN FLOW
  $ inspect connect <ns> --save-passphrase     # also: --save-password
        Prompts once, opens the master, saves the credential to the
        OS keychain under service 'inspect-cli', account '<ns>'.
        Subsequent 'inspect connect <ns>' in fresh shell sessions
        consult the keychain automatically — but only for namespaces
        that were previously saved. There is no implicit
        cross-namespace lookup.

SUBCOMMANDS
  list      Show stored namespaces (no values).
  remove    Delete the entry for one namespace.
  test      Round-trip probe (write/read/delete a known dummy entry).

CREDENTIAL RESOLUTION ORDER
  Key auth: socket → user mux → ssh-agent → key_passphrase_env → \
**OS keychain** → interactive prompt.
  Password auth (L4): socket → user mux → password_env → **OS \
keychain** → interactive prompt.
  The keychain is consulted only for namespaces previously saved \
with --save-passphrase. Missing entry ⇒ silent fall-through to \
the next step; never an error on the auto-retrieval path.

BACKENDS
  macOS    Keychain Services
  Windows  Credential Manager (also reachable from WSL2)
  Linux    Secret Service via DBus (GNOME Keyring / KDE Wallet)

  Pure-Rust crypto; no system OpenSSL dependency. libdbus is \
vendored (built from source) so the binary works on systems \
without dev headers.

HEADLESS / CI
  When the OS backend is unreachable (no keyring daemon, no \
session bus, container without a desktop):
  - --save-passphrase warns once and falls back to per-session
    prompt; the master still comes up.
  - Auto-retrieval during normal connects silently treats backend
    errors as 'not stored' (no stderr line per call).
  - 'inspect keychain test' is the explicit probe — exits non-zero
    with a chained hint when the backend is unreachable.

INDEX
  inspect keeps a small `~/.inspect/keychain-index` (mode 0600, \
one namespace per line). The index holds NO secret material — \
only namespace names — and exists because the keyring crate's \
enumeration support is platform-spotty. 'inspect keychain list' \
is self-healing: entries the backend no longer recognizes are \
pruned silently on the next call.

AUDIT
  inspect keychain remove   verb=keychain.remove, target=<ns>, \
revert.kind=unsupported (we don't store the secret; can't replay).
  Save is implicit in 'inspect connect --save-passphrase' and \
audited by the connect entry; the keychain module emits no \
separate audit row for the save itself.

EXIT CODES
  0   ok
  1   no matching namespace / backend unavailable on 'test'
  2   argument/usage error";

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
MODEL:    shells out to remote 'grep -r' against the resolved target path; no client-side indexing.
EXAMPLE:  inspect grep arte/onyx-vault:/var/log 'panic'
NOTE:     for indexed search across a fleet, use 'inspect search' (LogQL DSL, profile-side index).

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

const LONG_COMPOSE: &str = "\
First-class verbs over Docker Compose projects discovered on the namespace.

Replaces the v0.1.2-era `inspect run <ns> -- 'cd <project_dir> && sudo \
docker compose ...'` pattern: every sub-verb resolves the project's \
working directory from the cached profile, so operators never type the \
path. Compose project discovery runs at `inspect setup` time via \
`docker compose ls --format json` and is surfaced by both \
`inspect compose ls` and the new `compose_projects:` line in \
`inspect status`.

READ SUBCOMMANDS
  ls       List compose projects on the namespace.
  ps       Per-service status table for one project.
  config   Effective merged compose config (redacted).
  logs     Aggregated logs for a project, or one service inside it.

WRITE SUBCOMMANDS (audited; require --apply)
  up       Bring up a project. verb=compose.up.
  down     Tear down a project. verb=compose.down. --volumes is destructive.
  pull     Pull images for a project. verb=compose.pull. Streams progress.
  build    Build images for a project. verb=compose.build. Streams progress.
  restart  Restart a single service. verb=compose.restart.

EXEC (inspect-run-style; not audited)
  exec     Run a command inside a compose service container. Mirrors
           `inspect run`'s contract (no apply gate, no audit, output
           runs through the L7 redaction pipeline).

SELECTORS
  <ns>                       — for `compose ls`
  <ns>/<project>             — for `compose ps`, `compose config`,
                               aggregated `compose logs`, and
                               `compose restart --all`
  <ns>/<project>/<service>   — for narrowed `compose logs` and
                               for `compose restart` (the safe default)

  The existing `<ns>/<service>` form continues to work for the
  generic read/write verbs (`inspect logs`, `inspect restart`) because
  F5's resolver tries the compose service label first.

JSON SCHEMAS (--json)
  ls:      data.compose_projects = [{name, status, working_dir,
           compose_file, service_count, running_count}, ...]
  ps:      data.services = [{service, state, ports, image, uptime}, ...]
  restart: audit entry with verb=compose.restart, plus per-service rows
           in DATA. Each audit entry records project, service, and
           compose_file_hash so the post-mortem can verify the file
           didn't change between the audit and a re-run.

EXIT CODES
  0   ok
  1   no matching compose project / service
  2   usage error (missing service portion without --all, malformed selector,
      or one of the deferred sub-verbs)

EXAMPLES
  $ inspect compose ls arte
  $ inspect compose ps arte/luminary-onyx
  $ inspect compose config arte/luminary-onyx --json
  $ inspect compose logs arte/luminary-onyx --tail 200
  $ inspect compose logs arte/luminary-onyx/onyx-vault --follow
  $ inspect compose restart arte/luminary-onyx/onyx-vault --apply
  $ inspect compose up arte/luminary-onyx --apply
  $ inspect compose down arte/luminary-onyx --apply --yes
  $ inspect compose pull arte/luminary-onyx --apply
  $ inspect compose build arte/luminary-onyx --no-cache --apply
  $ inspect compose exec arte/luminary-onyx/onyx-vault -- ps -ef";

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
  suffixes), set `--stdin-max 0` to disable, or use `inspect put`
  (canonical) / `inspect cp` for bulk transfer (uncapped,
  audit-tracked, F11-revertible).

  Pass `--no-stdin` to refuse to forward; if you pass `--no-stdin`
  while local stdin has data waiting, `inspect run` exits 2 BEFORE
  dispatching the remote command (never silently discards input).

STREAMING (F16, v0.1.3)
  `--stream` (alias `--follow`) line-streams remote stdout/stderr
  to local stdout instead of buffering until the remote command
  exits. Required for long-running commands that produce output
  indefinitely until SIGINT (`docker logs -f`, `tail -f`,
  `journalctl -fu vault`, `python -m monitor`). Without `--stream`,
  these commands either buffer until exit (silent until you Ctrl-C
  the local `inspect`, which often orphans the remote process) or
  work only by accident if the remote happens to flush eagerly.

  Forces `ssh -tt` (PTY allocation): the PTY makes the remote
  process line-buffer its output (so lines arrive in real time
  instead of in 4 KB bursts) and propagates local Ctrl-C through
  the PTY layer to the remote process (so the command actually
  dies instead of being orphaned).

  Default timeout is bumped to 8 hours under `--stream` (matches
  `inspect logs --follow`); override either default with
  `--timeout-secs <N>`.

  Every `--stream` invocation writes a one-line audit entry with
  `streamed: true` (and the usual `failure_class`, `rendered_cmd`,
  `duration_ms`); non-streaming runs omit the `streamed` field.

  Mutex with `--stdin-script` (clap-rejected; the half-duplex case
  of streaming both directions on the same SSH stdin is deferred
  to v0.1.5). `--stream --file <script>` is fine — the script body
  is delivered in one shot, then output streams back.

  See `inspect logs --follow` for the dedicated log-tailing verb;
  F16 is for non-logs streaming commands.

MULTI-STEP (F17, v0.1.3)
  `--steps <PATH>` reads a JSON manifest (file path, or `-` for
  stdin) describing an ordered list of steps to dispatch
  sequentially against a single resolved target. Promotes the
  defensive `set +e; ... || echo MARKER` heredoc pattern to a
  first-class verb mode with structured per-step output that an
  LLM-driven wrapper can reason about.

  Manifest shape:
    {\"steps\": [
      {\"name\": \"snap\",  \"cmd\": \"docker compose stop app\",
       \"on_failure\": \"stop\", \"revert_cmd\": \"docker compose start app\"},
      {\"name\": \"migrate\", \"cmd_file\": \"./migrate.sh\",
       \"on_failure\": \"stop\", \"timeout_s\": 600},
      {\"name\": \"verify\",  \"cmd\": \"curl -fsS http://localhost/health\",
       \"on_failure\": \"continue\"}
    ]}

  Per-step fields: `name` (req, unique), `cmd` (req unless
  `cmd_file`), `cmd_file` (path to a local script body shipped via
  `bash -s`; F14 composition), `on_failure` (`\"stop\"` default
  | `\"continue\"`), `timeout_s` (per-step wall-clock cap, default
  8h), `revert_cmd` (declared inverse for F11 composite revert;
  absent ⇒ `revert.kind = \"unsupported\"` for that step).

  Output (human): one `STEP <name> ▶` / `STEP <name> ◀ exit=N
  duration=Ms` block per step, then a `STEPS: N total, K ok,
  M failed, S skipped` table with ✓/✗/· markers. Output (--json):
  one structured object with `steps[]` (each with `name`, `cmd`,
  `exit`, `duration_ms`, `stdout`, `stderr`, `status:
  \"ok\"|\"failed\"|\"skipped\"|\"timeout\"`) plus `summary`
  (counts + `stopped_at`) plus `manifest_sha256` and
  `steps_run_id`.

  Audit shape: every step writes its own `run.step` audit entry,
  all linked via `steps_run_id` (a fresh UUID-shaped id for the
  invocation). The parent invocation also writes a `run.steps`
  entry with `revert.kind = \"composite\"`, `manifest_sha256`,
  and `manifest_steps` (the ordered name list). Post-hoc:
  `inspect audit show <steps_run_id>` for the parent record;
  `inspect revert <steps_run_id>` walks the per-step inverses
  in reverse manifest order.

  `--revert-on-failure` (requires `--steps`): when a step fails
  with `on_failure: \"stop\"`, walk the inverses of the prior
  steps in reverse order and dispatch each as its own
  audit-logged auto-revert entry (linked via `auto_revert_of`).
  Steps with no declared `revert_cmd` are skipped with a
  warning rather than aborting the unwind.

  Mutex with `--file` / `--stdin-script` / `--stream` (clap
  rejected). Single-target only — fanout selectors exit 2 with a
  chained hint. YAML input (`--steps-yaml`) and per-step live
  streaming under `--steps --stream` are deferred to v0.1.5.

EXAMPLES
  $ inspect run arte/atlas -- env
  $ inspect run arte/atlas -- 'docker ps --format json'
  $ inspect run 'prod-*' -- 'df -h /var'
  $ inspect run arte 'docker exec -i atlas-pg sh' < ./init.sql
  $ cat big.tar.gz | inspect run arte --stdin-max 100m -- 'tar -xz -C /opt'
  $ inspect run arte --stream -- 'docker logs -f atlas-vault'
  $ inspect run arte --follow -- 'tail -f /var/log/syslog'
  $ inspect run arte --steps migration.json
  $ inspect run arte --steps migration.json --revert-on-failure
  $ cat migration.json | inspect run arte --steps -";

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
unified diff before `--apply`. Bidirectional convenience over the \
canonical F15 verbs `inspect put` (upload) and `inspect get` \
(download); arg shape decides the direction. See `inspect help write`.

EXAMPLES
  $ inspect cp ./fix.conf arte/pulse:/etc/pulse.conf --diff
  $ inspect cp ./fix.conf arte/pulse:/etc/pulse.conf --apply
  $ inspect cp arte/atlas:/var/log/atlas.log ./atlas.log";

const LONG_PUT: &str = "\
Upload a local file to a remote path on a namespace target (F15, \
v0.1.3). The remote endpoint can be a host-level path \
(`<ns>/_:/path` or the `<ns>:/path` shorthand from F7.2) or a \
container-level path (`<ns>/<svc>:/path`, dispatched via \
`docker exec -i <ctr> sh -c 'cat > /path'`). The transfer rides on \
the same persistent ControlPath master used by every other \
namespace verb, so it inherits F12 env overlay, F13 stale-session \
auto-reauth, F11 revert capture, and the standard audit trail.

Dry-run by default; `--apply` executes. Captures the prior remote \
file content as `revert.kind = state_snapshot` (or a delete-the-file \
inverse when the target did not exist), so `inspect revert <id>` \
restores either way.

EXAMPLES
  $ inspect put ./fix.conf arte:/etc/atlas/fix.conf --apply
  $ inspect put ./atlas.yml arte/_:/etc/compose/atlas.yml --apply
  $ inspect put ./vault.hcl arte/atlas-vault:/etc/vault/config.hcl --apply
  $ inspect put ./helper arte:/usr/local/bin/helper --mode 0755 --apply
  $ inspect put ./cfg arte:/etc/svc/cfg --mkdir-p --apply

NOTE
  `inspect cp` is a bidirectional convenience that dispatches to
  `put` (local → remote) or `get` (remote → local) based on arg
  shape. The canonical names are `put` and `get`.";

const LONG_GET: &str = "\
Download a remote file from a namespace target to a local path \
(F15, v0.1.3). The remote endpoint can be a host-level path \
(`<ns>/_:/path` or the `<ns>:/path` shorthand from F7.2) or a \
container-level path (`<ns>/<svc>:/path`). Like every other \
namespace-bound verb, the transfer rides the persistent ControlPath \
master and inherits F12 env overlay + F13 auto-reauth.

`inspect get` is read-only on the remote, so `revert.kind` is \
`unsupported` (revert by deleting the local file). The audit \
entry still records `bytes` + `sha256` so a later `inspect put` \
of the same content is verifiable byte-for-byte.

EXAMPLES
  $ inspect get arte:/etc/compose/atlas.yml ./atlas.yml
  $ inspect get arte/_:/var/log/syslog ./syslog
  $ inspect get arte/atlas-vault:/etc/vault/config.hcl ./vault.hcl
  $ inspect get arte:/etc/issue -                            # `-` writes to stdout

NOTE
  `inspect cp` dispatches here when the source carries the
  selector. Canonical name is `get`.";

const LONG_EDIT: &str = "\
In-place sed-style content edit (atomic). Dry-run by default — shows a \
unified diff. `--apply` writes.

EXAMPLES
  $ inspect edit arte/atlas:/etc/atlas.conf 's/timeout=30/timeout=60/'
  $ inspect edit arte/atlas:/etc/atlas.conf 's/timeout=30/timeout=60/' --apply
  $ inspect edit '*/atlas:/etc/atlas.conf' 's|debug=on|debug=off|' --apply --yes-all";

const LONG_AUDIT: &str = "\
Inspect or query the local audit log. Subcommands: ls, show, grep, \
verify, gc.

GC + RETENTION (L5, v0.1.3)
  `inspect audit gc --keep <X>` deletes audit entries older than the
  retention threshold and sweeps orphan snapshot files under
  `~/.inspect/audit/snapshots/`. `<X>` accepts:
    90d / 4w / 12h / 15m  duration suffix (days, weeks, hours, minutes)
    100                   integer = newest N entries kept per namespace
  `--dry-run` previews counts + freed bytes without modifying anything.
  `--json` emits the deterministic { deleted_entries, deleted_snapshots,
  freed_bytes, ... } envelope. A snapshot referenced by any retained
  audit entry is **never** deleted (F11 revert contract).

  Set `[audit] retention = \"<X>\"` in `~/.inspect/config.toml` to make
  GC run lazily on every audit append (cheap-path: scans only when the
  oldest file's mtime crosses the threshold, and at most once per
  minute via the `~/.inspect/audit/.gc-checked` marker).

EXAMPLES
  $ inspect audit ls
  $ inspect audit show <id>
  $ inspect audit grep \"atlas\"
  $ inspect audit gc --keep 90d --dry-run
  $ inspect audit gc --keep 100 --json";

const LONG_HISTORY: &str = "\
F18 (v0.1.3): per-namespace, per-day human-readable transcript of \
every verb invocation and its full output, written automatically by \
default to ~/.inspect/history/<ns>-<YYYY-MM-DD>.log (mode 0600).

The transcript is a complement to the structured audit log
(`~/.inspect/audit/`). Audit log answers \"what verbs ran with what
arguments + what changed\"; transcript answers \"what did I see on
my terminal during the 4-hour migration?\". Each verb produces one
fenced block so `awk '/^── /,/^── exit=/'` extracts individual
invocations; `audit_id=<id>` in the footer cross-links back to the
structured audit entry for forensic round-trip.

SUBCOMMANDS
  show     Render fenced blocks. Defaults to today's transcript for
           the most-recently-used namespace. Filter with --date,
           --grep, or --audit-id. Transparently decompresses gz.
  list     List transcript files with sizes and date ranges.
  clear    Delete files older than --before YYYY-MM-DD for one
           namespace. Confirmation gate via --yes.
  rotate   Apply the [history] retention policy now: delete files
           older than retain_days, gzip files older than
           compress_after_days, evict oldest-first when total bytes
           exceed max_total_mb. Lazy version fires once a day from
           transcript::finalize.

CONFIG (`~/.inspect/config.toml`)
  [history]
  retain_days = 90              # default 90; older files deleted on rotate
  max_total_mb = 500            # default 500; cap across all namespaces
  compress_after_days = 7       # default 7; older files gzipped on rotate

PER-NAMESPACE OVERRIDES (`~/.inspect/servers.toml`)
  [namespaces.<ns>.history]
  disabled = true               # skip transcript writes for this ns
  redact = \"off\" | \"normal\" | \"strict\"

REDACTION
  Every line tee'd into the transcript runs through the L7
  four-masker pipeline (PEM / Authorization / URL credentials /
  KEY=VALUE) using the per-namespace mode. `redact = \"off\"`
  writes raw lines (file mode 0600 already restricts exposure).
  `--show-secrets` on the originating verb bypasses redaction in
  both stdout and transcript.

EXAMPLES
  $ inspect history show arte                        # today's transcript
  $ inspect history show arte --date 2026-04-28      # one specific day
  $ inspect history show arte --grep 'docker volume rm'
  $ inspect history show --audit-id 01HXR9Q5YQK2     # cross-ref from audit
  $ inspect history list arte
  $ inspect history rotate --json
  $ inspect history clear arte --before 2026-01-01 --yes";

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
  plan    Validate the bundle, interpolate {{ vars.* }} / {{ matrix.* }},
          and print the rendered step list. Never touches a remote.
  apply   Run preflight, then steps in order. On failure, route via the
          step's `on_failure:` (abort | continue | rollback | rollback_to:<id>).
          Postflight runs on success and is reported but does NOT trigger
          rollback.
  status  L6 (v0.1.3): show per-step + per-branch outcomes for a past
          `apply` invocation. Reads the local audit log; no remote work.
          Accepts a `bundle_id` prefix. `--json` emits the structured
          per-branch outcomes for agent consumption.

Audit:
  Every exec step (and bundle.rollback / bundle.watch action) writes one
  audit entry tagged with bundle_id (a fresh ULID-shaped id per apply
  invocation) and bundle_step (the step's `id:`). L6 (v0.1.3) adds
  `bundle_branch` (`<matrix-key>=<value>`) and `bundle_branch_status`
  (`ok` | `failed` | `skipped`) on per-branch entries from `parallel:
  true` + `matrix:` steps. `inspect audit grep <bundle_id>` matches
  every entry tagged with this bundle.

Per-branch rollback (L6, v0.1.3):
  When a `parallel: true` + `matrix:` step fails partway, on_failure:
  rollback inverts ONLY the succeeded branches. Failed branches log a
  `bundle.rollback.skip` audit entry explaining why no inverse fired.
  The `{{ matrix.<key> }}` reference inside a `rollback:` block now
  resolves to each succeeded branch's value (the v0.1.2 empty-matrix
  bug is fixed). `inspect bundle status <id>` renders the per-branch
  table with ✓ (ok) / ✗ (failed) / · (skipped) / ↶ (rollback ok)
  markers.

EXAMPLES
  $ inspect bundle plan deploy.yaml
  $ inspect bundle apply deploy.yaml --apply --reason 'INC-1234'
  $ inspect bundle apply deploy.yaml --apply --no-prompt    # CI-safe
  $ inspect bundle status <bundle-id>
  $ inspect bundle status <bundle-id> --json";

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
    /// L4 (v0.1.3): SSH-related management subcommands (`add-key`).
    #[command(long_about = LONG_SSH)]
    Ssh(SshArgs),
    /// L2 (v0.1.3): OS keychain management (opt-in, cross-session).
    #[command(long_about = LONG_KEYCHAIN)]
    Keychain(KeychainArgs),

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
    /// Upload a local file to a namespace target (F15, v0.1.3).
    #[command(long_about = LONG_PUT)]
    Put(PutArgs),
    /// Download a remote file from a namespace target (F15, v0.1.3).
    #[command(long_about = LONG_GET)]
    Get(GetArgs),
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

    // ---- v0.1.3 F18 session transcript ---------------------------------------
    /// Browse and rotate the per-namespace per-day transcript files.
    #[command(long_about = LONG_HISTORY)]
    History(HistoryArgs),

    // ---- Phase 11 fleet ------------------------------------------------------
    /// Run a verb across multiple namespaces.
    #[command(long_about = LONG_FLEET)]
    Fleet(FleetArgs),

    // ---- v0.1.2 B9 bundle ----------------------------------------------------
    /// YAML-driven multi-step orchestration with rollback.
    #[command(long_about = LONG_BUNDLE)]
    Bundle(BundleArgs),

    // ---- v0.1.3 F6 compose ---------------------------------------------------
    /// First-class verbs over Docker Compose projects (F6, v0.1.3).
    #[command(long_about = LONG_COMPOSE)]
    Compose(ComposeArgs),

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
    /// L7 (v0.1.3): print secret-shaped values verbatim. By default
    /// `inspect search` runs every emitted log line through the
    /// redaction pipeline (`pem` / `header` / `url` / `env` maskers).
    /// Use this only when the captured output is provably safe (test
    /// fixture, public dataset).
    #[arg(long)]
    pub show_secrets: bool,
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
    /// F12 (v0.1.3): print the namespace's configured env overlay
    /// (the `[namespaces.<ns>.env]` block in `~/.inspect/servers.toml`)
    /// and exit without opening a session. Mutually exclusive with the
    /// other env-mutation flags.
    #[arg(
        long,
        conflicts_with_all = ["set_env", "unset_env", "set_path", "detect_path"],
    )]
    pub show: bool,
    /// F12 (v0.1.3): set `PATH` for this namespace (shorthand for
    /// `--set-env PATH=<value>`). Persists immediately.
    #[arg(long, value_name = "PATH")]
    pub set_path: Option<String>,
    /// F12 (v0.1.3): set an env-overlay entry for this namespace
    /// (repeatable, `--set-env KEY=VALUE`). Persists immediately.
    #[arg(long, value_name = "KEY=VALUE")]
    pub set_env: Vec<String>,
    /// F12 (v0.1.3): remove an env-overlay entry for this namespace
    /// (repeatable, `--unset-env KEY`). Persists immediately.
    #[arg(long, value_name = "KEY")]
    pub unset_env: Vec<String>,
    /// F12 (v0.1.3): probe the remote login PATH and, if it differs
    /// from the non-login PATH, prompt to pin the diff into the env
    /// overlay. Non-tty invocation auto-declines (never writes config
    /// without confirmation).
    #[arg(long)]
    pub detect_path: bool,
    /// L2 (v0.1.3): save the prompted credential to the OS keychain
    /// after a successful master start so subsequent connects in
    /// fresh shell sessions don't re-prompt. Idempotent re-saves
    /// are silent. The flag works for both auth modes — for
    /// `auth = "key"` it saves the key passphrase, for
    /// `auth = "password"` it saves the password. Backend
    /// unavailable → warns once and continues without saving;
    /// the master still comes up. Pair with `inspect keychain
    /// remove <ns>` to undo.
    #[arg(long, alias = "save-password")]
    pub save_passphrase: bool,
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

// L4 (v0.1.3): inspect ssh ... — SSH management subcommands.
#[derive(Debug, Args)]
#[command(after_help = SEE_ALSO_SSH)]
pub struct SshArgs {
    #[command(subcommand)]
    pub command: SshSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum SshSubcommand {
    /// Install a public key on the namespace's remote host and
    /// optionally migrate it off password auth.
    #[command(long_about = LONG_SSH_ADD_KEY)]
    AddKey(SshAddKeyArgs),
}

#[derive(Debug, Args)]
#[command(after_help = SEE_ALSO_SSH)]
pub struct SshAddKeyArgs {
    /// Namespace to install the key on.
    pub namespace: String,
    /// Use an existing key instead of generating one. The path
    /// must point at a private key (`<path>.pub` is read for the
    /// public half).
    #[arg(long, value_name = "PATH")]
    pub key: Option<std::path::PathBuf>,
    /// Skip the interactive offer to flip
    /// `auth = "password"` → `auth = "key"` in `servers.toml`.
    /// Only the public-key install runs.
    #[arg(long)]
    pub no_rewrite_config: bool,
    /// Required to perform the install + audit-log entry. Without
    /// it, the verb prints a dry-run preview and exits 0.
    #[arg(long)]
    pub apply: bool,
    /// Free-form note attached to the audit entry. Limited to 240
    /// characters.
    #[arg(long, value_name = "TEXT")]
    pub reason: Option<String>,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

const LONG_SSH_ADD_KEY: &str = "\
Install a public key on a namespace's remote host and offer to flip \
its config from password auth to key auth. The audited migration path \
off password-only legacy boxes (L4, v0.1.3).

USAGE
  inspect ssh add-key <ns>             # dry-run preview
  inspect ssh add-key <ns> --apply     # generate (or reuse) + install + offer flip
  inspect ssh add-key <ns> --apply --key <path>
  inspect ssh add-key <ns> --apply --no-rewrite-config

REQUIREMENTS
  A live ssh session for `<ns>` must be open (`inspect connect <ns>`). \
  The verb installs the public key over the existing master rather \
  than re-authenticating, so the operator's password is entered \
  exactly once per migration.

KEY MATERIAL
  Without `--key`, an ed25519 keypair is generated at \
  `~/.ssh/inspect_<ns>_ed25519` with a passphrase-less private half \
  (file mode 0600). The matching public key is written to \
  `<path>.pub` (mode 0644). With `--key <path>`, that key is used \
  as-is; the verb refuses if the corresponding `.pub` file is \
  missing.

INSTALL
  The public key is appended to the remote `~/.ssh/authorized_keys` \
  if and only if the exact key line is not already present \
  (idempotent). Remote permissions are normalized: `~/.ssh` to 0700, \
  `~/.ssh/authorized_keys` to 0600. The verb verifies the install by \
  re-reading the file and exits 1 if the line is not present after \
  the write.

CONFIG FLIP
  By default, after a successful install, the verb prompts the \
  operator to rewrite `~/.inspect/servers.toml`: \
  `auth = \"key\"`, `key_path = \"<path>\"`, with `password_env` and \
  `session_ttl` dropped. `--no-rewrite-config` skips the prompt. On \
  non-tty stdin, the flip auto-declines (no config changes without \
  explicit confirmation).

AUDIT
  One audit entry per `--apply` run: \
  `verb=ssh.add-key, target=<ns>` with bracketed args \
  `[key_path=...] [generated=true|false] [installed=true] \
  [config_rewritten=true|false]`. `revert.kind=command_pair` points \
  at a manual remove from `authorized_keys` (the verb does not \
  attempt to revoke automatically — that requires further operator \
  intent).

EXAMPLES
  inspect ssh add-key legacy-box
  inspect ssh add-key legacy-box --apply
  inspect ssh add-key legacy-box --apply --key ~/.ssh/site_id_ed25519
  inspect ssh add-key legacy-box --apply --no-rewrite-config

EXIT CODES
  0   ok
  1   no matching namespace / install verification failed
  2   argument/usage error (e.g. `--key` path does not exist)";

// L2 (v0.1.3): inspect keychain ... — OS keychain management.
#[derive(Debug, Args)]
#[command(after_help = SEE_ALSO_SSH)]
pub struct KeychainArgs {
    #[command(subcommand)]
    pub command: KeychainSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum KeychainSubcommand {
    /// Show namespaces with stored keychain entries (no values).
    #[command(long_about = LONG_KEYCHAIN_LIST)]
    List(KeychainListArgs),
    /// Delete the keychain entry for one namespace.
    #[command(long_about = LONG_KEYCHAIN_REMOVE)]
    Remove(KeychainRemoveArgs),
    /// Probe the OS keychain backend with a write/read/delete round-trip.
    #[command(long_about = LONG_KEYCHAIN_TEST)]
    Test(KeychainTestArgs),
}

#[derive(Debug, Args)]
#[command(after_help = SEE_ALSO_SSH)]
pub struct KeychainListArgs {
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
#[command(after_help = SEE_ALSO_SSH)]
pub struct KeychainRemoveArgs {
    /// Namespace whose keychain entry should be deleted.
    pub namespace: String,
    /// Free-form note attached to the audit entry. Limited to 240
    /// characters.
    #[arg(long, value_name = "TEXT")]
    pub reason: Option<String>,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
#[command(after_help = SEE_ALSO_SSH)]
pub struct KeychainTestArgs {
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

const LONG_KEYCHAIN_LIST: &str = "\
List namespaces with stored OS keychain entries (L2, v0.1.3).

OUTPUT
  Each row is one namespace with a saved entry. The entries are \
sorted alphabetically. No secret material is shown — only the \
namespace names.

  Self-healing: any index entry the OS backend no longer recognizes \
(e.g., the operator deleted it externally via Keychain Access.app \
or `secret-tool clear ...`) is pruned on this call. Subsequent \
calls reflect reality.

JSON
  $ inspect keychain list --json
  {\"schema_version\":1,\"namespaces\":[\"arte\",\"legacy-box\"],\"backend_status\":\"available\"}

  When the OS backend is unreachable the schema still parses; \
the names array reflects the on-disk index without backend probing \
and `backend_status` is \"unavailable\" with a `reason` field.

EXIT CODES
  0   ok (including the empty case — no namespaces stored)
  1   backend unavailable AND the on-disk index is also empty
      (operator never opted into keychain on this machine)";

const LONG_KEYCHAIN_REMOVE: &str = "\
Delete the OS keychain entry for one namespace (L2, v0.1.3).

USAGE
  $ inspect keychain remove arte
  $ inspect keychain remove legacy-box --reason 'rotated key'
  $ inspect keychain remove arte --json

WHAT IT DOES
  Removes the `(service=inspect-cli, account=<ns>)` entry from \
the OS keychain and prunes the namespace from \
`~/.inspect/keychain-index`. Idempotent — removing an absent \
entry exits 0 with a `was_present: false` note.

AUDIT
  Writes one entry per invocation:
    verb=keychain.remove, target=<ns>,
    args=\"[was_present=true|false]\",
    revert.kind=unsupported  (the secret was not stored on the
    inspect side, so the only re-save path is `inspect connect
    --save-passphrase`, which prompts for the secret again).

EXIT CODES
  0   ok (whether or not an entry was actually present)
  1   keychain backend unavailable
  2   argument/usage error (e.g., invalid namespace name)";

const LONG_KEYCHAIN_TEST: &str = "\
Probe the OS keychain backend (L2, v0.1.3).

WHAT IT DOES
  Writes a known dummy entry under \
`(service=inspect-cli, account=__inspect_keychain_test__)`, reads \
it back, and deletes it. If every step succeeds, the backend is \
reachable and `--save-passphrase` will work on this host. If any \
step fails, the failure mode is reported with a chained hint at \
which dependency is missing (no keyring daemon, no session bus, \
DBus not running, etc.).

USAGE
  $ inspect keychain test
  $ inspect keychain test --json

OUTPUT
  Human form:
    SUMMARY: keychain backend reachable
    DATA:
      backend: keychain (Linux Secret Service / GNOME Keyring)
      probe:   write + read + delete OK

  JSON form:
    {\"schema_version\":1,\"status\":\"available\",\"backend\":\"...\"}
    or
    {\"schema_version\":1,\"status\":\"unavailable\",\"reason\":\"...\",\"hint\":\"...\"}

EXIT CODES
  0   backend reachable
  1   backend unreachable (no keyring daemon, no session bus, etc.)
      → see the `hint:` line for the next operator action";

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
    /// L7 (v0.1.3): print secret-shaped values verbatim. By default
    /// `inspect logs` runs every line through the redaction pipeline
    /// (`pem` / `header` / `url` / `env` maskers).
    #[arg(long)]
    pub show_secrets: bool,
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

    /// L7 (v0.1.3): print secret-shaped values verbatim. By default
    /// `inspect grep` runs every emitted line through the redaction
    /// pipeline (`pem` / `header` / `url` / `env` maskers).
    #[arg(long)]
    pub show_secrets: bool,

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
    /// F10.2 (v0.1.3): inclusive 1-based line range to print, e.g.
    /// `--lines 5-10`. Mutually exclusive with `--start`/`--end`.
    /// Alias `--range`.
    #[arg(
        long = "lines",
        alias = "range",
        value_name = "L-R",
        conflicts_with_all = ["start", "end"],
    )]
    pub lines: Option<String>,
    /// F10.2 (v0.1.3): inclusive 1-based start line. Pair with
    /// `--end` for a range, or omit `--end` to print from `--start`
    /// to EOF. Mutually exclusive with `--lines`.
    #[arg(long = "start", value_name = "N")]
    pub start: Option<usize>,
    /// F10.2 (v0.1.3): inclusive 1-based end line. Pair with
    /// `--start`. Mutually exclusive with `--lines`.
    #[arg(long = "end", value_name = "N")]
    pub end: Option<usize>,
    /// L7 (v0.1.3): print secret-shaped values verbatim. By default
    /// `inspect cat` runs the file content through the redaction
    /// pipeline (`pem` / `header` / `url` / `env` maskers) — most
    /// notably collapsing PEM private-key blocks to a single
    /// `[REDACTED PEM KEY]` marker. Use this only for files you've
    /// already vetted as non-sensitive.
    #[arg(long)]
    pub show_secrets: bool,
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
    /// L7 (v0.1.3): print emitted paths verbatim. `find` emits file
    /// paths only — secret patterns rarely fire — but the flag is
    /// exposed for symmetry with the other read verbs.
    #[arg(long)]
    pub show_secrets: bool,
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
    /// L7 (v0.1.3): print secret-shaped values verbatim. Off by
    /// default so log captures and screenshots are safe — every
    /// emitted line otherwise runs through the four-masker pipeline
    /// (`pem` / `header` / `url` / `env` maskers): PEM private-key
    /// blocks collapse to `[REDACTED PEM KEY]`, `Authorization` /
    /// `Cookie` / `X-API-Key` / `Set-Cookie` header values become
    /// `<redacted>`, password portions of `scheme://user:pass@host`
    /// URLs are masked to `user:****@host`, and `KEY=VALUE` env
    /// pairs with secret-shaped keys (P4 suffix list) become
    /// `head4****tail2`. On `exec`, `--show-secrets` stamps
    /// `[secrets_exposed=true]` into the audit args.
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
    /// F12 (v0.1.3): per-invocation env-overlay entry, repeatable.
    /// Merges on top of the namespace overlay (operator wins on
    /// collision).
    #[arg(long, value_name = "KEY=VALUE")]
    pub env: Vec<String>,
    /// F12 (v0.1.3): drop the namespace's env overlay for this
    /// invocation only.
    #[arg(long)]
    pub env_clear: bool,
    /// F12 (v0.1.3): print the rendered remote command line (including
    /// the `env KEY="VAL" -- ` overlay prefix and any `docker exec`
    /// wrapping) to stderr before dispatch.
    #[arg(long)]
    pub debug: bool,
    /// F13 (v0.1.3): disable stale-session auto-reauth for this
    /// invocation. When set, a transport-stale dispatch failure
    /// surfaces as exit 12 with the chained `ssh_error: stale
    /// connection` SUMMARY hint instead of being transparently
    /// retried. Per-namespace `auto_reauth = false` in
    /// `servers.toml` has the same effect persistently.
    #[arg(long)]
    pub no_reauth: bool,
}

#[derive(Debug, Args)]
#[command(
    long_about = LONG_RUN,
    after_help = SEE_ALSO_READ,
    // F17 (v0.1.3): both --steps and --steps-yaml are valid
    // manifest sources for the multi-step runner; --revert-on-failure
    // requires either one.
    group(ArgGroup::new("manifest_source").args(["steps", "steps_yaml"])),
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
    /// L7 (v0.1.3): print secret-shaped values verbatim. Off by
    /// default so log captures and screenshots are safe — every
    /// emitted line otherwise runs through the four-masker pipeline
    /// (`pem` / `header` / `url` / `env` maskers): PEM private-key
    /// blocks collapse to `[REDACTED PEM KEY]`, `Authorization` /
    /// `Cookie` / `X-API-Key` / `Set-Cookie` header values become
    /// `<redacted>`, password portions of `scheme://user:pass@host`
    /// URLs are masked to `user:****@host`, and `KEY=VALUE` env
    /// pairs with secret-shaped keys (P4 suffix list) become
    /// `head4****tail2`. On `exec`, `--show-secrets` stamps
    /// `[secrets_exposed=true]` into the audit args.
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
    /// F10.7 (v0.1.3): strip ANSI escape sequences from captured
    /// output and prepend `TERM=dumb` to the remote command's env
    /// so progress bars / colorizers downgrade to plain text. Use
    /// for log captures and snapshots that must remain pipe-clean.
    /// Alias `--no-tty`. Mutually exclusive with `--tty`.
    #[arg(long = "clean-output", alias = "no-tty", conflicts_with = "tty")]
    pub clean_output: bool,
    /// F10.7 (v0.1.3): force tty allocation on the remote side.
    /// Mutually exclusive with `--clean-output`. Reserved for the
    /// (future) interactive-run flag-set; currently a no-op marker
    /// so `--clean-output --tty` is a clap-level rejection.
    #[arg(long = "tty")]
    pub tty: bool,
    /// F12 (v0.1.3): per-invocation env-overlay entry, repeatable.
    /// Merges on top of the namespace overlay (operator wins on
    /// collision). With `--env-clear`, replaces the namespace overlay
    /// entirely.
    #[arg(long, value_name = "KEY=VALUE")]
    pub env: Vec<String>,
    /// F12 (v0.1.3): drop the namespace's env overlay for this
    /// invocation only. Composes with `--env`: pass `--env-clear --env
    /// LANG=C` to dispatch with only `LANG=C`. The audit entry still
    /// records what the namespace overlay would have been.
    #[arg(long)]
    pub env_clear: bool,
    /// F12 (v0.1.3): print the rendered remote command line (including
    /// the `env KEY="VAL" -- ` overlay prefix and any container
    /// wrapping) to stderr before dispatch. Use to confirm what
    /// actually crosses the SSH channel.
    #[arg(long)]
    pub debug: bool,
    /// F13 (v0.1.3): disable stale-session auto-reauth for this
    /// invocation. When set, a transport-stale dispatch failure
    /// surfaces as exit 12 with the chained `ssh_error: stale
    /// connection` SUMMARY hint instead of being transparently
    /// retried. Per-namespace `auto_reauth = false` in
    /// `servers.toml` has the same effect persistently.
    #[arg(long)]
    pub no_reauth: bool,
    /// F14 (v0.1.3): script mode — read the entire bash payload from
    /// `<PATH>` on the local filesystem and ship it as the remote
    /// command body via `bash -s` (or the interpreter declared in the
    /// script's shebang). The script is **never parsed by any local
    /// shell** beyond the one that invoked `inspect`, so embedded
    /// `psql -c "..."` / `python -c '...'` / `cypher-shell` heredocs
    /// reach the remote interpreter byte-for-byte. Arguments after
    /// `--` become the script's positional `$1` / `$2` / ... .
    /// Mutually exclusive with `--no-stdin` and `--stdin-script`.
    #[arg(long, value_name = "PATH", conflicts_with_all = ["no_stdin", "stdin_script"])]
    pub file: Option<String>,
    /// F14 (v0.1.3): script mode — read the script body from local
    /// stdin and ship it as the remote command body via `bash -s`.
    /// Stdin must NOT be a tty; the heredoc form
    /// `inspect run arte --stdin-script <<'BASH' ... BASH` is the
    /// canonical use. Mutually exclusive with `--no-stdin`,
    /// `--file`, and `--stream` (streaming + script-on-stdin is a
    /// half-duplex protocol headache deferred to v0.1.5).
    #[arg(long, conflicts_with_all = ["no_stdin", "stream"])]
    pub stdin_script: bool,
    /// F16 (v0.1.3): line-stream remote stdout/stderr to local
    /// stdout instead of buffering until the remote command exits.
    /// Required for long-running commands like `docker logs -f`,
    /// `tail -f /var/log/...`, `journalctl -fu vault`, or any other
    /// process that produces output indefinitely until SIGINT. Forces
    /// `ssh -tt` (PTY allocation) so (1) the remote process flips
    /// from block-buffered to line-buffered output and lines arrive
    /// in real time instead of in 4 KB bursts, and (2) local Ctrl-C
    /// propagates through the PTY layer to the remote process so the
    /// command actually dies instead of being orphaned. Default
    /// timeout is bumped to 8 hours (override with
    /// `--timeout-secs <N>`). Mutually exclusive with
    /// `--stdin-script`. `--follow` is an alias.
    #[arg(long, alias = "follow")]
    pub stream: bool,
    /// F14 (v0.1.3): record the full script body inline in the audit
    /// entry. Off by default to keep the JSONL small; the body is
    /// otherwise dedup-stored under `~/.inspect/scripts/<sha256>.sh`
    /// (mode 0600) and the audit entry references it by hash.
    #[arg(long)]
    pub audit_script_body: bool,
    /// F17 (v0.1.3): multi-step runner mode — read a JSON manifest
    /// (file path, or `-` for stdin) describing an ordered list of
    /// steps to dispatch sequentially against every target the
    /// selector resolves to. Each step has `name`, `cmd` (or
    /// `cmd_file` for an F14 script reference), `on_failure`
    /// (`"stop"` default | `"continue"`), optional `timeout_s`
    /// (per-step wall-clock cap, seconds), optional `revert_cmd`
    /// (declared inverse for F11 composite revert; absent ⇒
    /// `revert.kind = "unsupported"` for that step). Output is
    /// per-step structured (STEP markers + table summary, or a
    /// single JSON object under `--json`); every (step, target)
    /// pair writes its own audit entry, all linked via
    /// `steps_run_id`, and the parent invocation's audit entry has
    /// `revert.kind = "composite"` so `inspect revert <parent-id>`
    /// walks the inverses in reverse manifest order. Composes with
    /// `--stream` (forces PTY on every per-step dispatch for
    /// line-buffered live output), `--env` (per-step env overlay),
    /// `--reason` (recorded on the parent audit entry), F13
    /// auto-reauth (a stale socket mid-pipeline triggers transparent
    /// reauth + retry on the failing step). Multi-target dispatch is
    /// sequential within each step; on_failure="stop" applies
    /// globally (any target's failure aborts the next manifest step
    /// on all targets). Mutually exclusive with `--file`,
    /// `--stdin-script`, and `--steps-yaml`.
    #[arg(
        long,
        value_name = "PATH",
        conflicts_with_all = ["file", "stdin_script", "steps_yaml"],
    )]
    pub steps: Option<String>,
    /// F17 (v0.1.3): YAML manifest variant of `--steps`. Same
    /// schema, just parsed as YAML instead of JSON for operators
    /// who maintain their migration manifests as YAML alongside
    /// CI/CD pipelines. Mutually exclusive with `--steps`.
    #[arg(
        long = "steps-yaml",
        value_name = "PATH",
        conflicts_with_all = ["file", "stdin_script", "steps"],
    )]
    pub steps_yaml: Option<String>,
    /// F17 (v0.1.3): when a step fails under `--steps` /
    /// `--steps-yaml` with `on_failure = "stop"`, walk the inverses
    /// of the steps that already ran (in reverse manifest order)
    /// before exiting. Multi-target: each prior step's inverse fans
    /// out across every original target before moving to the next
    /// step's inverse. Each auto-revert dispatches as its own
    /// audit-logged entry stamped with `auto_revert_of:
    /// <original-step-id>` so the audit log reconstructs the full
    /// unwind chain. Steps whose `revert.kind = "unsupported"` (no
    /// declared `revert_cmd` for a free-form `bash -c` body) are
    /// skipped with a one-line warning rather than aborting the
    /// unwind.
    #[arg(long, requires = "manifest_source")]
    pub revert_on_failure: bool,
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
    /// L6 (v0.1.3): show per-step + per-branch outcomes for a past
    /// bundle invocation by `bundle_id`. Reads the local audit log;
    /// no remote work. `--json` returns the structured per-branch
    /// outcomes for agent consumption.
    Status(BundleStatusArgs),
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
pub struct BundleStatusArgs {
    /// `bundle_id` from a past `inspect bundle apply` invocation.
    /// Accepts a prefix (matched against the leading characters of
    /// every audit entry's `bundle_id`); ambiguous prefixes are
    /// reported and exit non-zero.
    pub bundle_id: String,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
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
    /// F15 (v0.1.3): on a push, set the remote file's mode (octal,
    /// e.g. `0755` or `755`) after the transfer. Applied via
    /// `chmod` on the remote; overrides the mode-mirror that the
    /// atomic-write helper would otherwise inherit from the prior
    /// file at the same path.
    #[arg(long, value_name = "OCTAL")]
    pub mode: Option<String>,
    /// F15 (v0.1.3): on a push, set the remote file's owner
    /// (`user` or `user:group`) after the transfer. Requires the
    /// SSH user have permission to chown — typically root via
    /// the namespace's existing privilege model.
    #[arg(long, value_name = "USER[:GROUP]")]
    pub owner: Option<String>,
    /// F15 (v0.1.3): on a push, create missing parent directories
    /// on the remote (`mkdir -p`) before writing. Without this,
    /// a missing parent dir surfaces as `error: remote parent
    /// directory does not exist` and the transfer is aborted.
    #[arg(long)]
    pub mkdir_p: bool,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
#[command(long_about = LONG_PUT, after_help = SEE_ALSO_WRITE)]
pub struct PutArgs {
    /// Local source path.
    pub local: String,
    /// Remote destination as `<selector>:<path>` (e.g.
    /// `arte:/etc/foo`, `arte/_:/etc/foo`,
    /// `arte/atlas:/etc/vault/config.hcl`). Selector must carry a
    /// `:<path>` — F7.2 shorthand `<ns>:/path` resolves to the
    /// host-level `_` service.
    pub remote: String,
    /// Apply the transfer. Without this, prints a dry-run preview.
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
    /// F15 (v0.1.3): set the remote file's mode (octal, e.g. `0755`
    /// or `755`) after the transfer. Applied via `chmod` on the
    /// remote; overrides the mode-mirror inherited from any prior
    /// file at the same path.
    #[arg(long, value_name = "OCTAL")]
    pub mode: Option<String>,
    /// F15 (v0.1.3): set the remote file's owner (`user` or
    /// `user:group`) after the transfer. Requires the SSH user
    /// have permission to chown.
    #[arg(long, value_name = "USER[:GROUP]")]
    pub owner: Option<String>,
    /// F15 (v0.1.3): create missing parent directories on the
    /// remote (`mkdir -p`) before writing. Without this, a missing
    /// parent surfaces as `error: remote parent directory does not
    /// exist` and the transfer aborts.
    #[arg(long)]
    pub mkdir_p: bool,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
#[command(long_about = LONG_GET, after_help = SEE_ALSO_WRITE)]
pub struct GetArgs {
    /// Remote source as `<selector>:<path>` (e.g.
    /// `arte:/etc/foo`, `arte/_:/etc/foo`,
    /// `arte/atlas:/etc/vault/config.hcl`). Selector must carry a
    /// `:<path>` — F7.2 shorthand `<ns>:/path` resolves to the
    /// host-level `_` service.
    pub remote: String,
    /// Local destination path, or `-` to write to stdout.
    pub local: String,
    /// Free-form note recorded in the audit entry. Limited to 240 characters.
    #[arg(long, value_name = "TEXT")]
    pub reason: Option<String>,
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
    long_about = LONG_AUDIT,
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
    /// L5 (v0.1.3): delete audit entries older than the retention
    /// threshold and sweep orphan snapshot files. See
    /// `inspect audit --help` for the GC + RETENTION section that
    /// documents `--keep` syntax, the `[audit] retention` config
    /// hook, and the once-per-minute cheap-path marker.
    Gc(AuditGcArgs),
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

/// L5 (v0.1.3): retention GC over `~/.inspect/audit/`.
#[derive(Debug, Args)]
pub struct AuditGcArgs {
    /// Retention threshold. Either a duration suffix (`90d`, `4w`,
    /// `12h`, `15m`) or a bare integer (entries-per-namespace, newest
    /// first). Pass `0` is rejected — refusing to silently delete
    /// every entry is the only safe default.
    #[arg(long, value_name = "DURATION-OR-COUNT")]
    pub keep: String,
    /// Preview the deletion without modifying anything; counts and
    /// freed bytes are computed identically to a real run.
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

// ---- F18 history (v0.1.3) ---------------------------------------------------

#[derive(Debug, Args)]
#[command(
    long_about = LONG_HISTORY,
    after_help = SEE_ALSO_SAFETY,
)]
pub struct HistoryArgs {
    #[command(subcommand)]
    pub command: HistoryCommand,
}

#[derive(Debug, Subcommand)]
pub enum HistoryCommand {
    /// Render fenced transcript blocks. Filter by --date, --grep, or
    /// --audit-id. Transparently decompresses .log.gz files. Default
    /// scope is today's transcript for the most-recently-used
    /// namespace.
    Show(HistoryShowArgs),
    /// List transcript files with sizes and date ranges. Optional
    /// namespace filter.
    List(HistoryListArgs),
    /// Delete transcript files older than --before YYYY-MM-DD for
    /// one namespace. Audit log is untouched.
    Clear(HistoryClearArgs),
    /// Apply the `[history]` retention policy now: delete +
    /// compress + evict per `~/.inspect/config.toml`. Lazy version
    /// fires once per day from `transcript::finalize`.
    Rotate(HistoryRotateArgs),
}

#[derive(Debug, Args)]
pub struct HistoryShowArgs {
    /// Restrict to one namespace. When omitted with no other filter,
    /// every namespace's today's-transcript is rendered.
    pub namespace: Option<String>,
    /// One specific day, YYYY-MM-DD (UTC). When omitted, today's
    /// transcript for the resolved namespace is rendered.
    #[arg(long, value_name = "YYYY-MM-DD")]
    pub date: Option<String>,
    /// Substring filter applied across every fenced block (header,
    /// argv, body, footer). Case-sensitive.
    #[arg(long, value_name = "PATTERN")]
    pub grep: Option<String>,
    /// Cross-reference filter: render only the block(s) whose
    /// `audit_id=` footer matches (substring match against the
    /// trailing audit id).
    #[arg(long, value_name = "ID")]
    pub audit_id: Option<String>,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct HistoryListArgs {
    /// Optional namespace filter.
    pub namespace: Option<String>,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct HistoryClearArgs {
    /// Namespace whose transcripts to delete.
    pub namespace: String,
    /// Delete every file dated strictly before this YYYY-MM-DD.
    #[arg(long, value_name = "YYYY-MM-DD")]
    pub before: String,
    /// Confirm the deletion. Without this flag, `clear` prints what
    /// it would do and exits non-zero.
    #[arg(long)]
    pub yes: bool,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct HistoryRotateArgs {
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

// ---- F6 compose (v0.1.3) ----------------------------------------------------

#[derive(Debug, Args)]
#[command(
    long_about = LONG_COMPOSE,
    after_help = SEE_ALSO_COMPOSE,
)]
pub struct ComposeArgs {
    #[command(subcommand)]
    pub command: ComposeCommand,
}

#[derive(Debug, Subcommand)]
pub enum ComposeCommand {
    /// List compose projects discovered on the namespace. Reads
    /// from the cached profile; pass `--refresh` to re-probe live.
    Ls(ComposeLsArgs),
    /// Per-service status table for one project. Wraps `docker
    /// compose -p <project> ps --all --format json` over the
    /// persistent ssh socket.
    Ps(ComposePsArgs),
    /// Effective merged compose config for one project. Wraps
    /// `docker compose -p <project> config` over the persistent
    /// socket; output streams through the redaction pipeline so
    /// secret-shaped values in `environment:` blocks and URL
    /// auth portions are masked unless `--show-secrets` is passed.
    Config(ComposeConfigArgs),
    /// Aggregated logs for a project, or one service inside it.
    /// Wraps `docker compose -p <project> logs` with the same
    /// `--tail` / `--follow` / `--since` flags as `inspect logs`.
    Logs(ComposeLogsArgs),
    /// Restart a single service inside a compose project. Audited
    /// (`verb=compose.restart`); requires `--apply` to actually
    /// execute. Without a service portion in the selector, refuses
    /// to fan out unless `--all` is passed (defensive default —
    /// "the user typed `--all`, they meant it").
    Restart(ComposeRestartArgs),

    /// Bring up a compose project (`docker compose -p <p> up [-d]`).
    /// Audited (`verb=compose.up`); requires `--apply`.
    Up(ComposeUpArgs),
    /// Tear down a compose project (`docker compose -p <p> down`).
    /// Audited (`verb=compose.down`); requires `--apply`. Pass
    /// `--volumes` to also remove named volumes (DESTRUCTIVE).
    Down(ComposeDownArgs),
    /// Pull images for a project (`docker compose -p <p> pull`).
    /// Audited (`verb=compose.pull`); requires `--apply`. Streams
    /// docker pull progress lines so operators see what's happening
    /// during multi-minute pulls.
    Pull(ComposePullArgs),
    /// Build images for a project (`docker compose -p <p> build`).
    /// Audited (`verb=compose.build`); requires `--apply`. Streams
    /// build output for visibility on long builds.
    Build(ComposeBuildArgs),
    /// Run a command inside a compose service (`docker compose -p
    /// <p> exec <svc> ...`). Mirrors `inspect run` — no audit, no
    /// apply gate, output redacted unless `--show-secrets`.
    Exec(ComposeExecArgs),
}

#[derive(Debug, Args)]
pub struct ComposeLsArgs {
    /// Namespace selector (no service portion). Multi-namespace
    /// selectors (`prod-*`, `arte~staging`) are supported and the
    /// projects are tagged with their owning namespace in the
    /// JSON envelope.
    pub selector: String,
    /// Bypass the cached project list and re-probe live via
    /// `docker compose ls --all --format json`. Use after a `compose
    /// up` (run out-of-band) to see a freshly-deployed project
    /// without waiting for the next `inspect setup`. `--live` is
    /// an alias for symmetry with the other read verbs.
    #[arg(long, alias = "live")]
    pub refresh: bool,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct ComposePsArgs {
    /// Project selector: `<ns>/<project>`. Globs are not supported
    /// here — `compose ps` is a single-project verb.
    pub selector: String,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct ComposeConfigArgs {
    /// Project selector: `<ns>/<project>`.
    pub selector: String,
    /// L7 (v0.1.3): print secret-shaped values verbatim. Off by
    /// default — every line otherwise runs through the redaction
    /// pipeline (env / header / URL / PEM maskers).
    #[arg(long)]
    pub show_secrets: bool,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct ComposeLogsArgs {
    /// Project or service selector: `<ns>/<project>[/<service>]`.
    /// Without the service portion, logs from every service in the
    /// project are aggregated.
    pub selector: String,
    /// Show logs since duration (e.g. `30s`, `5m`, `1h`, `2d`).
    /// Forwarded as `docker compose logs --since <duration>`.
    #[arg(long)]
    pub since: Option<String>,
    /// Number of lines from the tail. Forwarded as `--tail N`.
    #[arg(long)]
    pub tail: Option<u64>,
    /// Stream logs (`docker compose logs --follow`).
    #[arg(short = 'f', long)]
    pub follow: bool,
    /// L7 (v0.1.3): print secret-shaped values verbatim.
    #[arg(long)]
    pub show_secrets: bool,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct ComposeRestartArgs {
    /// Service selector: `<ns>/<project>/<service>`. Without the
    /// service portion, `--all` is required (defensive default —
    /// "you didn't tell me which service, prove you really mean
    /// every service").
    pub selector: String,
    /// Restart every service in the project. Required when no
    /// service portion is given on the selector; harmless (a no-op)
    /// when a single service is named.
    #[arg(long)]
    pub all: bool,
    /// Actually perform the restart. Without this flag the verb
    /// is a dry-run that lists every service that *would* restart.
    #[arg(long)]
    pub apply: bool,
    /// Skip the per-verb confirmation prompt.
    #[arg(short = 'y', long)]
    pub yes: bool,
    /// Skip the large-fanout interlock as well.
    #[arg(long)]
    pub yes_all: bool,
    /// Free-form note recorded in the audit entry. Useful for
    /// change-management tickets, incident IDs, or simply "why did I
    /// run this?". Limited to 240 characters.
    #[arg(long, value_name = "TEXT")]
    pub reason: Option<String>,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

// F6 (v0.1.3): per-sub-verb args for the audited compose-state
// mutations (`up`/`down`/`pull`/`build`) and the `inspect run`-style
// `exec`. Each carries the standard write-verb safety knobs (`--apply`
// / `--yes` / `--yes-all` / `--reason`) plus a small set of compose
// passthrough flags. `exec` deliberately omits `--apply` — it mirrors
// `inspect run`, which is unaudited because the operator's intent is
// inspection, not state mutation.

#[derive(Debug, Args)]
pub struct ComposeUpArgs {
    /// Project selector: `<ns>/<project>`.
    pub selector: String,
    /// Run the project in the foreground (drops the default `-d`).
    /// Useful only when piping to a TUI; rare in inspect's audited
    /// workflow because output goes through the audit-capture path.
    #[arg(long)]
    pub no_detach: bool,
    /// Force-recreate every container even if config / image
    /// haven't changed (`--force-recreate` passthrough).
    #[arg(long)]
    pub force_recreate: bool,
    /// Actually perform the up. Without this flag, the verb is a
    /// dry-run that lists every service that *would* be brought up.
    #[arg(long)]
    pub apply: bool,
    #[arg(short = 'y', long)]
    pub yes: bool,
    #[arg(long)]
    pub yes_all: bool,
    /// Free-form note recorded in the audit entry. Limited to 240
    /// characters.
    #[arg(long, value_name = "TEXT")]
    pub reason: Option<String>,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct ComposeDownArgs {
    /// Project selector: `<ns>/<project>`.
    pub selector: String,
    /// Also remove named volumes declared in the compose file.
    /// **DESTRUCTIVE.** Confirms via the standard apply gate; pair
    /// with `--apply --yes-all` only after you've manually verified
    /// the volume contents are recoverable.
    #[arg(long)]
    pub volumes: bool,
    /// Also remove all images used by the project (`--rmi local`).
    #[arg(long)]
    pub rmi: bool,
    /// Actually perform the down. Without this flag, the verb is a
    /// dry-run.
    #[arg(long)]
    pub apply: bool,
    #[arg(short = 'y', long)]
    pub yes: bool,
    #[arg(long)]
    pub yes_all: bool,
    #[arg(long, value_name = "TEXT")]
    pub reason: Option<String>,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct ComposePullArgs {
    /// Project or service selector: `<ns>/<project>[/<service>]`.
    /// With a service portion, only that one image is pulled.
    pub selector: String,
    /// Continue pulling other services if one fails
    /// (`--ignore-pull-failures`).
    #[arg(long)]
    pub ignore_pull_failures: bool,
    /// Actually perform the pull. Without this flag, the verb lists
    /// what would be pulled.
    #[arg(long)]
    pub apply: bool,
    #[arg(short = 'y', long)]
    pub yes: bool,
    #[arg(long)]
    pub yes_all: bool,
    #[arg(long, value_name = "TEXT")]
    pub reason: Option<String>,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct ComposeBuildArgs {
    /// Project or service selector: `<ns>/<project>[/<service>]`.
    pub selector: String,
    /// Skip the build cache (`--no-cache`).
    #[arg(long)]
    pub no_cache: bool,
    /// Always pull base images during build (`--pull`).
    #[arg(long)]
    pub pull: bool,
    /// Actually perform the build.
    #[arg(long)]
    pub apply: bool,
    #[arg(short = 'y', long)]
    pub yes: bool,
    #[arg(long)]
    pub yes_all: bool,
    #[arg(long, value_name = "TEXT")]
    pub reason: Option<String>,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
}

#[derive(Debug, Args)]
pub struct ComposeExecArgs {
    /// Service selector: `<ns>/<project>/<service>`. The service
    /// portion is mandatory (compose exec without a target service
    /// is meaningless).
    pub selector: String,
    /// Command and arguments after `--`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub cmd: Vec<String>,
    /// Run as user inside the container (`-u` passthrough).
    #[arg(short = 'u', long, value_name = "USER")]
    pub user: Option<String>,
    /// Working directory inside the container (`-w` passthrough).
    #[arg(short = 'w', long, value_name = "DIR")]
    pub workdir: Option<String>,
    /// L7 (v0.1.3): print secret-shaped values verbatim.
    #[arg(long)]
    pub show_secrets: bool,
    /// L7 (v0.1.3): mask every `KEY=VALUE` line regardless of key
    /// name. Useful when the remote command emits config blobs you
    /// have not vetted.
    #[arg(long)]
    pub redact_all: bool,
    /// Free-form note. `compose exec` is not audited (mirrors
    /// `inspect run`), so this is purely informational and is
    /// echoed once to stderr at the start of the run.
    #[arg(long, value_name = "TEXT")]
    pub reason: Option<String>,
    #[command(flatten)]
    pub format: crate::format::FormatArgs,
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
        assert_match("compose", super::SEE_ALSO_COMPOSE);
        assert_match("help", super::SEE_ALSO_HELP);
    }
}
