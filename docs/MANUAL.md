# `inspect` — User Manual

> A hands-on manual for everyday use. The same material is also
> available offline inside the binary via `inspect help <topic>`.
> If anything in this file disagrees with `inspect help`, the in-binary
> copy is authoritative — please open an issue.

This manual is organized so you can read it top-to-bottom on day one and
then come back to a single section when you need it.

- [1. Install](#1-install)
- [2. Concepts in 90 seconds](#2-concepts-in-90-seconds)
- [3. First-time setup](#3-first-time-setup)
- [4. Selectors — addressing servers and services](#4-selectors--addressing-servers-and-services)
- [5. Read verbs — looking at things](#5-read-verbs--looking-at-things)
- [6. Search — the LogQL DSL](#6-search--the-logql-dsl)
- [7. Write verbs — changing things safely](#7-write-verbs--changing-things-safely)
- [8. Audit and revert](#8-audit-and-revert)
- [9. Output formats and scripting](#9-output-formats-and-scripting)
- [10. Recipes](#10-recipes)
- [11. Aliases and groups](#11-aliases-and-groups)
- [12. Fleet operations](#12-fleet-operations)
- [13. SSH lifecycle, ControlMaster, passphrases](#13-ssh-lifecycle-controlmaster-passphrases)
- [14. Configuration reference](#14-configuration-reference)
- [15. Troubleshooting](#15-troubleshooting)
- [16. Translation guide (grep / stern / ssh / sed)](#16-translation-guide-grep--stern--ssh--sed)

---

## 1. Install

The recommended path is the one-line installer. Every step is local,
verifies a SHA-256 sum (and a cosign signature if `cosign` is on
`$PATH`), and installs atomically.

```sh
curl -fsSL https://raw.githubusercontent.com/jpbeaudet/inspect/main/scripts/install.sh | sh
```

You can pin a version, change the destination, or skip signature
verification:

```sh
curl -fsSL .../install.sh | sh -s -- --version v0.1.0 --prefix /usr/local
curl -fsSL .../install.sh | sh -s -- --no-verify
```

After installation, confirm the binary works:

```sh
inspect --version
inspect help
inspect help logs        # synonym for `inspect logs --help` (F3, v0.1.3)
```

If `~/.local/bin` is not on `$PATH` (the default install root), the
installer prints a reminder. Add it to your shell profile.

Other install paths (Homebrew tap, `cargo install inspect-cli`, or a
direct download from GitHub Releases) are documented in the
[README](../README.md#install).

---

## 2. Concepts in 90 seconds

There are five things to know.

1. **Namespace.** A short name (e.g. `arte`, `prod-eu-1`) that maps to
   one host you reach over SSH. Configured once via `inspect add`,
   stored in `~/.inspect/`, and used as the first part of every
   selector.
2. **Profile.** A cached snapshot of what is on a host (containers,
   volumes, networks, listening ports). Built by `inspect setup
   <namespace>` and stored at `~/.inspect/profiles/<ns>.yaml`. Profiles
   make selectors fast and offline-friendly.
3. **Selector.** A short string that addresses one or more targets:
   `arte/atlas`, `arte/_:/etc/foo`, `prod-*/storage`. See §4.
4. **Verb.** What you want to do — `ps`, `logs`, `grep`, `edit`,
   `restart`, `search`. Read verbs run immediately. Write verbs are
   dry-run by default and require `--apply` to take effect.
5. **Audit / revert.** Every `--apply` is recorded under
   `~/.inspect/audit/` along with a snapshot of the original. Anything
   you change you can roll back with `inspect revert <audit-id>`.

That is the entire mental model. The rest of the manual is detail.

---

## 3. First-time setup

### 3.1 Make sure SSH already works

`inspect` shells out to your system `ssh`, so it inherits your
`~/.ssh/config`. Before adding a namespace, verify that the host is
reachable:

```sh
ssh arte hostname
```

If that fails, fix it the usual way (key, host entry, jump host, etc.)
before going further. `inspect` will not paper over a broken SSH setup.

### 3.2 Register the namespace

```sh
inspect add arte
```

This is interactive. It walks you through host, user, key path, and
optional Docker socket location, and writes the result to your local
config. You can also pre-set everything via environment variables —
useful for headless setups:

```sh
INSPECT_ARTE_HOST=arte.example.com \
INSPECT_ARTE_USER=ops \
INSPECT_ARTE_KEY_PATH=~/.ssh/id_ed25519 \
inspect add arte --non-interactive
```

### 3.3 Open a persistent session

```sh
inspect connect arte
```

This unlocks the SSH agent / key once and keeps a `ControlMaster`
socket open for the rest of the shell, so subsequent `inspect`
invocations do not prompt for a passphrase. See §15 for the full
SSH lifecycle.

### 3.4 Discover the topology

```sh
inspect setup arte
```

This walks the remote host with low-impact probes (no `apt`, no
`systemctl restart`) and writes `~/.inspect/profiles/arte.yaml`. The
profile is mode `0600` and contains no secrets. Re-run with `--force`
when the host changes (new container, new mount, etc.) — `inspect`
will also tell you when it detects drift.

If a single container is wedged (slow daemon socket, hung healthcheck)
the batched `docker inspect` will time out and the affected services
are flagged `discovery_incomplete: true` in the profile, with a
warning summary at the end of `inspect setup`. Re-probe just the
flagged services with:

```sh
inspect setup arte --retry-failed
```

This is cheaper than `--force` because it keeps the rest of the
profile cached and only re-runs `docker inspect` per-container with
a 5-second budget each.

### 3.5 Verify

```sh
inspect ps arte           # what containers are running
inspect status arte       # health summary
inspect list              # all namespaces you've registered
inspect show arte         # one namespace's details
```

If `ps` is empty but the host has containers, the SSH user probably
cannot talk to the Docker socket. Re-run `inspect add arte` and adjust
the `docker_socket` (or add the user to the `docker` group on the
remote).

---

## 4. Selectors — addressing servers and services

Every read or write verb takes a selector as its primary argument. The
grammar is small and consistent.

```
<selector> ::= <server>[/<service>][:<path>]   |   @<alias>
```

| Form | Meaning |
|---|---|
| `arte` | every service on `arte` |
| `arte/atlas` | one service |
| `arte/atlas,pulse` | two services |
| `arte/storage` | a profile group (see §11) |
| `'prod-*/storage'` | glob across servers (quote it!) |
| `'arte/^pulse-.*$'` | regex (slashes optional, must quote) |
| `arte/_` | host scope — for ports, host files, systemd units |
| `arte/atlas:/etc/atlas.conf` | one file inside a container |
| `arte/_:/var/log/syslog` | a host-level file |
| `arte:/etc/hostname` | host-level file shorthand (sugar for `arte/_:/etc/hostname`) |
| `@plogs` | a saved alias (see §11) |

### Resolution order

1. Container short name (`pulse`, `atlas`).
2. Aliases declared in the profile.
3. Groups declared in the profile.
4. Docker container name when distinct from the compose service name —
   e.g. both `arte/onyx-vault` (compose service) and
   `arte/luminary-onyx-onyx-vault-1` (the docker name from `docker ps`)
   resolve to the same target. When you typed the docker form,
   `inspect` prints a one-line breadcrumb on stderr pointing at the
   canonical compose form so the next invocation uses it. Suppress the
   hint with `INSPECT_NO_CANONICAL_HINT=1` for strict-stderr JSON
   pipelines. Aliases also surface in `inspect status --json` under
   `services[].aliases` so agents can enumerate equivalences.
5. Globs (`*`) and regex (`/.../` or quoted `^...$`) — match against
   either the compose name or the docker container name.
6. Subtractive (`~name`) — exclude after match.

If a name matches both a service and a group, the service wins and a
warning is emitted.

### Empty resolution is not silent

A selector that matches nothing prints the available servers, services,
groups, and aliases for the addressed namespace. There is no silent
no-op — if you don't see what you expect, the diagnostic is right
there.

### Quoting

Globs and regex must be single-quoted to keep your shell from
expanding them: `'prod-*'`, `'arte/^pulse-.*$'`. The colon path
separator does not need quoting unless the path itself has shell-
special characters.

For a deeper reference: `inspect help selectors`.

---

## 5. Read verbs — looking at things

| Verb | What it does |
|---|---|
| `ps` | running containers / services |
| `status` | one-line health per service |
| `health` | detailed health probe results |
| `logs` | container or host logs |
| `cat` | a file (container or host) |
| `grep <pattern>` | grep across logs and/or files |
| `find` | find files by name/glob |
| `ls` | list a directory |
| `network` | container networks |
| `images` | container images |
| `volumes` | mounted volumes |
| `ports` | host listening ports |
| `resolve <selector>` | print what a selector matches without running anything |

Common flags that work on most read verbs:

- `--since 1h --until 5m` — time window
- `--tail 100` — last N records
- `--follow` / `-f` — stream as new records arrive
- `--json` / `--jsonl` / `--csv` / `--md` / `--table` / `--format <go>` — output (see §9)
- `--timeout 30s` — give up if a host doesn't answer
- `--no-color` — for log capture

Examples:

```sh
inspect logs arte/atlas --since 30m --tail 200 --follow
inspect grep -i 'oom' arte/_ --since 1h
inspect cat arte/atlas:/etc/atlas.conf
inspect resolve 'prod-*/storage'
```

### Filtering `inspect ports`

`inspect ports <ns>` accepts two server-side filters so you don't
have to pipe through `grep` (and lose the SUMMARY/NEXT envelope):

- `--port <n>` — keep only rows mentioning a specific port number.
- `--port-range <lo-hi>` — keep only rows in `[lo, hi]` (inclusive).

The filters are mutually exclusive. The token-aware matcher handles
both the `0.0.0.0:8200` and `8200/tcp` shapes, so it doesn't fire
on incidental digits inside an interface name or a netns label. The
SUMMARY's "N listener(s)" count reflects the filtered total, not
the raw row count.

```sh
inspect ports arte --port 8200
inspect ports arte --port-range 8000-8999
```

### 5.1 Logs and grep — line filters and cursors (v0.1.1)

`inspect logs` and `inspect grep` accept two repeatable line-filter
flags that are pushed down to the remote host as a `grep -E` /
`grep -vE` pipeline suffix (server-side, so live `--follow` streams
stay snappy):

- `--match <regex>` / `-g <regex>` — keep lines matching the regex.
  Multiple flags OR together as `(?:p1)|(?:p2)`.
- `--exclude <regex>` / `-G <regex>` — drop matching lines.

A resumable cursor is also available:

- `--since-last` — resume from the previous run's start time, kept
  under `~/.inspect/cursors/<ns>/<svc>.kv` (mode 0600). Cold-start
  fallback comes from `INSPECT_SINCE_LAST_DEFAULT` (default `5m`).
  Mutually exclusive with `--since`.
- `--reset-cursor` — delete the saved cursor for the matched
  selector(s) and exit.

```sh
# Tail only error-shaped lines, ignore healthchecks
inspect logs arte/atlas --follow -g 'ERROR|FATAL' -G '/health'

# Pick up where you left off
inspect logs arte/atlas --since-last
```

### 5.2 `--merged` multi-container view (v0.1.1)

When a selector matches more than one service, `inspect logs <sel>
--merged` interleaves output from every selected service into a
single `[svc] <line>`-prefixed stream. We inject `--timestamps` into
the underlying `docker logs` invocation; batch mode k-way merges by
RFC3339 timestamp, follow mode prints in arrival order.

```sh
inspect logs 'arte/*' --merged --since 5m
inspect logs 'arte/*' --merged --follow
```

Lines whose driver isn't readable via `docker logs` (e.g. `awslogs`,
`gcplogs`) are skipped with a warning rather than failing the merged
view.

### 5.3 Progress spinner on slow fetches (v0.1.1)

`inspect logs` and `inspect grep` draw a small spinner to stderr
when a per-target fetch takes longer than 700ms. The spinner is
suppressed automatically in JSON mode, when stderr is not a TTY,
and when `INSPECT_NO_PROGRESS=1` is set in the env.

### 5.4 `inspect why` deep-diagnostic bundle (v0.1.3)

`inspect why <selector>` walks the dependency graph and labels each
node with a status — for healthy services that's the whole story.
For services in `unhealthy` or `down` state, three diagnostic
artifacts are now attached inline under `DATA:`:

- **`logs:`** — the recent log tail (default 20 lines, configurable
  via `--log-tail <N>`, hard-capped at 200 with a one-line stderr
  notice when exceeded).
- **`effective_command:`** — the container's effective `Entrypoint`
  + `Cmd` from `docker inspect`. When the container's
  `/docker-entrypoint.sh` (or `/entrypoint.sh`) contains a
  flag-injection pattern such as `-dev-listen-address=`,
  `-listen-address=`, `-bind-address=`, `-api-addr=`, or
  `--listen-address=`, the matched flag is surfaced as
  `wrapper injects: <flag>=<value>`.
- **`port_reality:`** — per-port table cross-referencing
  `PortBindings` and `ExposedPorts` from `docker inspect` against
  entrypoint-injected listeners and host listener state from
  `ss -ltn` (or `netstat -ltn` fallback). A port declared by *both*
  config and an entrypoint wrapper flag is marked
  `container: bound (twice!)` — the headline reproducer pattern
  for "service unhealthy with `bind: address already in use` in
  the logs".

The bundle costs **at most 4 extra remote commands per service per
invocation**: one `docker logs --tail`, one combined `docker inspect`,
one entrypoint-script `cat`, and one `ss -ltn`. Each fails
independently, so partial bundles still surface what worked.

Flags:
- `--no-bundle` — suppress the bundle and restore the v0.1.2 terse
  output (for agents that already drive `logs`, `inspect`, and
  `ports` themselves).
- `--log-tail <N>` — set the recent-logs tail size (default 20,
  capped at 200).

Smart `NEXT:` hints are pushed *before* the generic suggestions:
a bound-twice port emits an entrypoint-inspection hint
(`inspect run <ns>/<svc> -- 'cat /docker-entrypoint.sh'`), and
`address already in use` in the logs emits a port-reality hint
(`inspect ports <ns>`).

JSON mode adds three fields to each per-service object —
`recent_logs[]`, `effective_command{entrypoint, cmd, wrapper_injects}`,
`port_reality[{port, host, container, declared_by}]`. The fields
are always present; on healthy services they default to empty
arrays and `null` so agents don't need optional-chaining
gymnastics.

---

## 6. Search — the LogQL DSL

`inspect search` is the cross-medium query engine. It uses the same
syntax as Grafana Loki's LogQL and runs over logs, files, and host
state.

```sh
inspect search '{server="arte", source="logs"} |= "error"' --since 1h
```

### 6.1 Reserved labels

| Label | Meaning |
|---|---|
| `server` | namespace (`arte`, `prod-eu`) |
| `service` | container/service tag, or `_` for host-scoped |
| `source` | medium: `logs`, `file:/path`, `dir:/path`, `discovery`, `state`, `volume:name`, `image`, `network`, `host:/path` |

### 6.2 Selector matchers

Inside `{ ... }` use `=`, `!=`, `=~` (regex), `!~` (not regex). Combine
multiple sources with `or`:

```
{server="arte", service="pulse", source="logs"} or {server="arte", service="atlas", source="file:/etc/atlas.conf"}
```

### 6.3 Line filters

| Operator | Meaning |
|---|---|
| `\|= "x"` | line contains `x` |
| `!= "x"` | line does not contain `x` |
| `\|~ "rx"` | regex match |
| `!~ "rx"` | regex not match |

### 6.4 Pipeline stages (log queries)

Streaming, applied record-by-record:

```
| json
| logfmt
| pattern "<pattern>"
| regexp "<regex>"
| line_format "{{.field}}"
| label_format new=expr
| <field> <op> <value>     # ==, !=, >, >=, <, <=, =~, !~
| drop label1, label2
| keep label1, label2
| map { <sub-query> }      # cross-medium chain ($field$ interpolation)
```

### 6.5 Metric queries

Whole-window aggregates (a query is **either** a log query **or** a
metric query, never both):

```
count_over_time({...} |= "..." [5m])
rate({...} [5m])
sum by (service) (count_over_time({...} |= "error" [5m]))
topk(5, sum by (service) (rate({...} [1h])))
```

Plus `avg`, `min`, `max`, `bottomk`, `quantile_over_time`,
`bytes_over_time`, `bytes_rate`, `absent_over_time`, with
`by`/`without` grouping.

### 6.6 Reference

For the full reference: `inspect help search` and Loki's docs at
<https://grafana.com/docs/loki/latest/query/>. Behavior parity is the
goal — please file mismatches.

---

## 7. Write verbs — changing things safely

Write verbs are **dry-run by default**. The first invocation always
shows what would happen; you have to add `--apply` to enact it.

| Verb | What it changes |
|---|---|
| `restart` / `stop` / `start` / `reload` | container/service lifecycle |
| `cp <src> <dst>` | push or pull a file (dry-run shows the diff) |
| `edit <sel>:<path> '<sed-expr>'` | in-place atomic edit |
| `rm` / `mkdir` / `touch` | filesystem operations |
| `chmod` / `chown` | permission changes |
| `exec <sel> -- <cmd>` | arbitrary command (audited, see below) |

For read-only ad-hoc commands use `inspect run` instead — same shape
as `exec`, no audit log, no apply gate, masks secrets in stdout. See
§7.3.

### Safety contract

1. **Dry-run by default.** No mutation happens without `--apply`.
2. **Diff first.** `edit` and `cp` print a unified diff before any
   change.
3. **Audit log.** Every `--apply` is appended to
   `~/.inspect/audit/<YYYY-MM>-<user>.jsonl`.
4. **Snapshots.** The original content is saved under
   `~/.inspect/audit/snapshots/<sha>` before mutation.
5. **Confirmation prompts.** `rm`, `chmod`, and `chown` prompt
   interactively even with `--apply`. Skip with `--yes`.
6. **Atomic writes.** `edit` writes a tempfile, then renames.
7. **Large fan-out guard.** More than 10 targets prompts even with
   `--apply`. Skip with `--yes-all`.

### 7.1 `--reason <text>` (v0.1.1)

Every write verb (`restart`/`stop`/`start`/`reload`, `exec`, `cp`,
`edit`, `rm`, `mkdir`, `touch`, `chmod`, `chown`) accepts
`--reason <text>` to record *why* the change happened. The reason
is appended to the audit log alongside the diff and shows up as a
trailing column in `inspect audit ls` and on its own line in
`inspect audit show`. 240-character cap; oversize values are
rejected up-front. Filter by reason with
`inspect audit ls --reason <substr>` (case-insensitive).

### 7.2 A typical hot-fix flow

```sh
# 1. Preview
inspect edit arte/atlas:/etc/atlas.conf 's/timeout=30/timeout=60/'

# 2. Apply
inspect edit arte/atlas:/etc/atlas.conf 's/timeout=30/timeout=60/' \
  --apply --reason "INC-4421 raise atlas timeout"

# 3. Restart the affected service
inspect restart arte/atlas --apply --reason "INC-4421"

# 4. Verify
inspect logs arte/atlas --since 30s --follow
```

### 7.3 `run` vs `exec` (v0.1.1)

`inspect run <sel> -- <cmd>` runs an arbitrary read-only command on
every selected target and streams stdout line-by-line. It is **not
audited**, has **no apply gate**, and **no fan-out threshold** — use
it for ad-hoc inspection (`ps auxww`, `cat /proc/...`, `redis-cli
info`, etc.). `inspect exec` keeps the same shape but is audited
and requires `--apply` for state-mutating commands.

Both verbs propagate the remote command's **inner exit code** to
your shell: `inspect run arte/pulse -- 'exit 7'` returns 7. Mixed
exits across multiple targets fall back to exit 2.

### 7.4 Secret masking on `run` / `exec` (v0.1.1)

By default, `run` and `exec` scan stdout for `KEY=VALUE` lines and
mask the value when the key looks like a secret. The mask form is
`head4****tail2` (values shorter than 8 chars become `****`). The
`export ` prefix and matching quote pairs are preserved.

Recognized key shapes:

- Suffixes: `_KEY`, `_SECRET`, `_TOKEN`, `_PASSWORD`, `_PASS`,
  `_CREDENTIAL[S]`, `_APIKEY`, `_AUTH`, `_PRIVATE`, `_ACCESS_KEY`,
  `_DSN`, `_CONNECTION_STRING`.
- Exact: `DATABASE_URL`, `REDIS_URL`, `MONGO_URL`, `POSTGRES_URL`,
  `POSTGRESQL_URL`.

Opt-out flags:

- `--show-secrets` — verbatim output. On `exec`, this stamps
  `[secrets_exposed=true]` into the audit args so reviewers can
  tell apart verbatim from masked runs.
- `--redact-all` — mask **every** `KEY=VALUE` pair, not just
  recognized keys.

When masking actually fired during an `exec`, the audit args
captures `[secrets_masked=true]`.

For the deep dive: `inspect help write`.

### 7.5 `inspect run` stdin handling (v0.1.3)

When `inspect run`'s own stdin is non-tty (piped or redirected from
a file), it is forwarded byte-for-byte to the remote command's
stdin and closed on EOF, so commands that read until EOF (`sh`,
`psql`, `cat`, `tee`, `gpg`) terminate normally:

```sh
# Apply a local SQL script through a remote container's psql:
inspect run arte 'docker exec -i atlas-pg psql' < ./01-roles.sql

# Untar a local archive into a remote container:
cat fixtures.tgz | inspect run arte 'tar -xz -C /opt'
```

When local stdin **is** a tty (interactive terminal), forwarding is
skipped — `inspect run arte 'cat'` does not hang waiting for input
that was never piped. This matches `ssh -T host cmd <terminal>`
semantics.

**Audit.** Every `inspect run` invocation that forwards stdin
writes a one-line audit entry with `verb=run`, `stdin_bytes=<N>`,
and (with `--audit-stdin-hash`) `stdin_sha256=<hex>` of the
forwarded payload. Without forwarded stdin, `inspect run` remains
un-audited (matches v0.1.2 read-verb behavior).

**Size cap.** Default 10 MiB per invocation. Above that, exit 2
with a hint pointing at `inspect cp`. Override with `--stdin-max
<SIZE>` (k/m/g suffixes) or set `--stdin-max 0` to disable the cap
entirely. For bulk transfer, prefer `inspect cp` (faster,
resumable, audit-tracked separately).

**Loud failure.** `--no-stdin` refuses to forward. If you pass
`--no-stdin` while local stdin has data waiting, `inspect run`
exits 2 BEFORE dispatching the remote command — never silently
discards input. With `--no-stdin` and an empty pipe (`< /dev/null`,
`true | inspect run …`), the run proceeds normally without
forwarding, since there is no data to drop.

---

## 8. Audit and revert

```sh
inspect audit ls --limit 20          # recent mutations
inspect audit ls --bundle <bundle-id> # entries from one bundle apply
inspect audit show <id>              # one entry with diff summary
inspect audit grep "atlas"           # search audit entries
inspect revert <audit-id>            # preview the reverse
inspect revert <audit-id> --apply    # restore the original
inspect revert --last 3              # walk the 3 most recent applied entries
```

Audit entries record: timestamp, user, host, verb, selector, args,
diff summary, previous and new SHA-256, snapshot path, exit code,
duration. Mode `0600`.

### 8.1 The revert contract (v0.1.3)

Every write verb pre-stages its inverse **before** dispatching the
mutation. The captured inverse lives in the entry's `revert` block
alongside `applied: true|false` (was the mutation actually run) and
`no_revert_acknowledged: true` (operator opted in to a free-form
mutation via `--no-revert`).

There are four `revert.kind` values:

| Kind | When | What `inspect revert` does |
|---|---|---|
| `command_pair` | `chmod` / `chown` / `mkdir` / `touch` / lifecycle stop / start | runs a single inverse remote command (e.g. `chmod 0644 …`) |
| `state_snapshot` | `cp` / `edit` / `rm` (rm snapshots before deleting) | restores the prior file content from the snapshot store |
| `composite` | bundle steps (multi-step inverse, F17) | replays per-step inverses in reverse |
| `unsupported` | `restart` / `reload` / `exec --no-revert` / legacy v0.1.2 entries | exits 2 with a chained explanation; never silently no-ops |

**`--revert-preview`** on any write verb prints the captured inverse
to stderr before applying, so you can see exactly what
`inspect revert <new-id>` will undo:

```sh
inspect chmod arte/atlas:/etc/app.conf 0600 --apply --revert-preview
# stderr: [inspect] revert preview arte/atlas:/etc/app.conf:
#         command_pair -- chmod 0644 /etc/app.conf
```

**`inspect exec --apply` is special.** Because the payload is
free-form shell, no inverse can be synthesised. `--apply` therefore
**refuses** unless you explicitly pass `--no-revert` to acknowledge
the trade-off. If your mutation is structured (file content,
permissions, lifecycle), use the matching write verb instead — they
all capture real inverses.

**Backward compatibility.** Audit entries written before v0.1.3
have no `revert` field; they are read as `kind: unsupported` and
`inspect revert` refuses with a chained hint pointing at
`inspect audit show`. Snapshot-style legacy entries (with
`previous_hash` set) still revert through the existing path.

### 8.2 Audit log integrity

If the file changed since your edit (hash mismatch), `revert` warns
and refuses without `--force`. That is a feature — it stops you from
clobbering a more recent change.

The audit log is **forensic, not tamper-proof**. A user with file
access can edit or delete entries. For regulated environments,
forward audit entries to an external log system.

Every audit entry is `fdatasync(2)`'d after write, so it survives
power loss on conformant filesystems. On filesystems that do not
implement `fsync` (some FUSE/network mounts) `inspect` warns once
and continues — it would rather record the entry than refuse the
operation.

---

## 9. Output formats and scripting

Every command supports the same output flags:

| Flag | Output |
|---|---|
| `--json` | `summary | data | next` envelope, single JSON object |
| `--jsonl` | one JSON record per line (good for streaming) |
| `--csv` | comma-separated, with header |
| `--table` | aligned ASCII table (default for TTYs) |
| `--md` | Markdown table (great for issue comments) |
| `--format '<go-template>'` | Go-template over each record |
| `--raw` | unformatted (e.g. raw log lines) |
| `--quiet` | suppress the `SUMMARY:` / `NEXT:` envelope on the Human path; data rows emit without the leading two-space indent so output is safe to pipe into `grep`, `awk`, `tail`, `head`, `wc -l`. Mutually exclusive with `--json` / `--jsonl` (those are already pipe-clean by construction). |

The JSON envelope has a `schema_version` field. New fields are
non-breaking; renames or removals bump the major. See
[docs/RUNBOOK.md](RUNBOOK.md) §4.

A few common patterns:

```sh
# Top 10 services by error count over 1h
inspect search 'topk(10, sum by (service) (count_over_time({source="logs"} |= "error" [1h])))' --json | jq

# Restart everything that errored in the last 5 minutes
inspect search '{source="logs"} |= "OOM"' --since 5m --json \
  | jq -r '.service' | sort -u \
  | xargs -I{} inspect restart arte/{} --apply

# Markdown status block for a GitHub issue
inspect fleet --ns 'prod-*' status --md
```

For the full reference: `inspect help formats`.

---

## 10. Recipes

A recipe is a named sequence of `inspect` invocations — a runbook in
data form. Two flavors ship:

- **Built-ins.** Compiled into the binary. List them with
  `inspect recipe list`.
- **User recipes.** YAML files under `~/.inspect/recipes/`. They follow
  the same dry-run/apply contract as the verbs they call.

```sh
inspect recipe list
inspect recipe run why-noisy arte/atlas
inspect recipe run rotate-cert prod-eu/atlas --apply
```

Mutating recipe steps respect `--apply` exactly the way the
underlying verbs do. For more detail: `inspect help recipes`.

---

## 11. Aliases and groups

### Aliases

Save a selector under a short name and reuse it everywhere:

```sh
inspect alias add @plogs 'arte/pulse,atlas/_:/var/log/*.log'
inspect alias ls
inspect logs @plogs --since 1h
```

Aliases work both as raw arguments and inside LogQL queries.

### Groups

Groups are defined inside a profile (`~/.inspect/profiles/<ns>.yaml`)
or in a shared config:

```yaml
groups:
  storage: [postgres, milvus, redis, minio]
  edge:    [nginx, envoy]
```

Use them in selectors: `inspect status arte/storage`.

For the full reference: `inspect help aliases`.

---

## 12. Fleet operations

`inspect fleet` is the orchestrator for multi-namespace operations.

```sh
inspect fleet --ns 'prod-*' status
inspect fleet --ns arte,beta ps
inspect fleet --ns 'prod-*' restart atlas --apply --yes-all
```

Concurrency is bounded (default 4, override with
`INSPECT_MAX_PARALLEL` or `--max-parallel`). The output is grouped per
namespace; partial failures do not abort the rest. The JSON envelope's
`summary.failed` and `data[*].namespace` fields make scripted handling
predictable.

The large-fan-out prompt fires here too. Always do a `--ns ... status`
or `--ns ... resolve` first to confirm the blast radius.

For more: `inspect help fleet`.

---

## 13. Block until a condition with `inspect watch`

`inspect watch` blocks the shell until a single target reaches a
condition you describe, then exits. It is the building block for
"wait for the deploy to be healthy before flipping traffic" and
for smoke checks inside CI pipelines.

```sh
inspect watch arte/atlas --until-status running --timeout 2m
inspect watch arte/atlas --until-cmd 'systemctl is-active atlas' --eq active
inspect watch arte/atlas --until-log '/var/log/atlas.log' --match 'ready'
inspect watch arte/api --until-http https://api.example.com/healthz --status 200
```

Four predicate kinds, one at a time:

| Predicate | Satisfied when |
|---|---|
| `--until-status <state>` | container/process reaches that state (e.g. `running`) |
| `--until-cmd <sh>` `--eq/--ne/--contains <s>` | command stdout matches the comparator |
| `--until-log <path>` `--match <re>` | a new line in the log matches the regex |
| `--until-http <url>` `--status <code>` | HTTP probe returns that status (`--insecure`, `--header`) |

Knobs that apply to all predicates:

- `--interval <dur>` — poll interval (default `2s`).
- `--timeout <dur>` — overall ceiling. Exits **124** when reached.
- `--quiet` — suppress the per-tick status line.
- `--insecure` (only with `--until-http`) — disables TLS
  verification for self-signed staging endpoints. **Never use
  against production.**

Exit codes: **0** condition met, **124** timeout, **130** Ctrl-C,
**2** invalid arguments. Each completed watch writes one audit
entry (`verb=watch`) so retries are reconstructable.

For the full reference: `inspect help watch`.

---

## 14. Multi-step orchestration with `inspect bundle`

`inspect bundle` runs a YAML-described sequence of mutations across
one or more targets, with preflight checks, ordered or parallel
steps, automatic rollback on failure, and postflight verification.
Every action is audited and correlated by a single `bundle_id` so
the whole apply is one forensic unit.

Two subcommands:

```sh
inspect bundle plan  ./rollout.yaml          # render the plan, do nothing
inspect bundle apply ./rollout.yaml --apply  # execute (gate is mandatory)
inspect bundle apply ./rollout.yaml --apply --no-prompt --reason "CHG-1234"
```

A minimal bundle:

```yaml
version: 1
name: rotate-atlas-config
preflight:
  - id: ssh-up
    check: ssh
    target: arte/atlas
steps:
  - id: edit-conf
    target: arte/atlas
    edit:
      path: /etc/atlas/atlas.conf
      sed: 's/timeout=10/timeout=30/'
  - id: restart
    target: arte/atlas
    restart: {}
    requires: [edit-conf]
    on_failure: { rollback_to: edit-conf }
postflight:
  - id: healthy
    check: http
    url: https://atlas.example.com/healthz
    status: 200
```

Key mechanics:

- **`requires`** builds a DAG. Forward references and cycles are
  rejected at `plan` time.
- **`matrix`** on a step expands one step into N parallel branches,
  capped by `INSPECT_MAX_PARALLEL` (hard cap 8).
- **`on_failure`** routes the step's exit:
  `abort` (default), `continue`, or `{ rollback_to: <step-id> }`.
- **Rollback** walks completed reversible steps in reverse order.
  If a rollback action itself fails, the bundle exits with a clear
  "mixed state" warning naming the step.
- **`--apply` is mandatory** for any mutation. Without it, every
  mutating step is a dry-run, regardless of bundle contents.
- **Audit correlation:** every step writes an audit entry tagged
  with `bundle_id` and `bundle_step`. Retrieve with
  `inspect audit ls --bundle <bundle-id>`.

Exit codes: **0** all steps succeeded (postflight included), **3**
a step failed and rollback completed cleanly, **4** rollback itself
failed (mixed state — operator action required), **2** schema
error, **130** Ctrl-C during apply (rollback runs).

When to reach for `bundle` instead of `fleet` or `recipe`:

- **`fleet`** = same verb across many namespaces, no ordering.
- **`recipe`** = a named multi-step workflow with no rollback.
- **`bundle`** = ordered or parallel steps with preflight, rollback,
  postflight, and forensic correlation. Use it for change-managed
  rollouts.

For the full reference and schema: `inspect help bundle`.

---

## 15. SSH lifecycle, ControlMaster, passphrases

`inspect` does **not** read your passphrase. It uses your system
`ssh`, which is configured to use `ControlMaster` so you only unlock
your key once per shell.

```sh
inspect connect arte             # opens the master socket
inspect connections              # show open masters
inspect disconnect arte          # close one
inspect disconnect-all           # close all
```

If something looks wrong:

- Ports tied up: `inspect disconnect-all`, then `inspect connect <ns>`.
- "ssh: connection refused": check `~/.ssh/config` for the namespace
  host. `inspect` does not invent connection info.
- Hung command: send `SIGINT` once; the run cancels and emits a
  partial-result envelope (no orphan SSH children).

For the full reference: `inspect help ssh` (and `inspect help ssh
--verbose` for the deep details on `ControlMaster`).

### 15.1 Per-namespace remote env overlay (v0.1.3)

The non-login SSH shell that `inspect run` / `inspect exec` lands
in often has a leaner `PATH` and no `LANG` than your interactive
`inspect connect` shell. That's why a clean target where
`cargo build` works after `connect` will turn around and fail with
`bash: cargo: command not found` from a `run` two minutes later.

`inspect` lets each namespace persist a small environment overlay
that is prefixed onto every remote `run`/`exec` for that namespace.
Values are double-quoted, so `$VAR` still expands on the remote,
but `;`/`&`/`|` stay literal — the overlay can't smuggle a second
command past the safety contract.

```sh
# See the current overlay
inspect connect arte --show

# Pin the right PATH (interactive walk: probes login vs non-login PATH
# and offers to write the union when the login PATH adds entries)
inspect connect arte --detect-path

# Or set values directly (atomic 0600 round-trip on ~/.inspect/servers.toml)
inspect connect arte --set-path '$HOME/.cargo/bin:$HOME/.local/bin:$PATH'
inspect connect arte --set-env LANG=C.UTF-8 --set-env RUST_BACKTRACE=1

# Drop a single key
inspect connect arte --unset-env RUST_BACKTRACE
```

Per-call overrides on `inspect run` and `inspect exec`:

| Flag | Effect |
|---|---|
| `--env KEY=VAL` (repeatable) | merge onto the namespace overlay (user wins on collision) |
| `--env-clear` | drop the namespace overlay for *this* call only |
| `--debug` | print the rendered remote command to stderr before transport |

The overlay is recorded in the audit log alongside the rendered
command, so `inspect why --revert` (and any forensic walk back over
`~/.inspect/audit/`) sees exactly what shipped.

### 15.2 Stale-session auto-reauth (v0.1.3)

OpenSSH's `ControlPersist`-backed master socket can go silent when
the remote sshd's idle timeout expires, when an iptables rule cuts
the long-lived TCP connection, or when the `inspect connect`
session's own `ServerAliveInterval` decides the peer is dead. The
v0.1.2 contract surfaced these as plain `exit 255` — the same code
the remote command itself returns when it can't be found — leaving
shell wrappers no way to tell "I need to re-auth" apart from "this
command is broken."

v0.1.3 splits the failure surface and, by default, transparently
re-auths once on stale sessions:

| Class | Exit | When it fires |
|---|---:|---|
| `transport_stale` | `12` | master socket / `ControlPersist` expired |
| `transport_unreachable` | `13` | DNS failure, no route, `Connection refused`, `Host key verification failed` |
| `transport_auth_failed` | `14` | every key rejected, or auto-reauth itself failed |
| `command_failed` | remote exit | non-zero exit from the operator's command |
| `ok` | `0` | success |

Default behaviour on `transport_stale`:

1. `inspect` prints
   `note: persistent session for <ns> expired — re-authenticating…`
   to stderr.
2. The persistent master socket is torn down and re-established
   through the same code path as interactive `inspect connect <ns>`
   (askpass / agent / `*_PASSPHRASE_ENV` semantics preserved).
3. The original step is re-run **exactly once**.
4. The retry's outcome — pass or fail — is final. There is no
   exponential backoff and no second retry.

Both the failed-original and the retry get audit entries linked by
`reauth_id`; the `connect.reauth` audit entry records the trigger
(`trigger=transport_stale,original_verb=run,selector=<sel>`) so a
post-hoc audit walker can reconstruct the cause.

**Opting out.** Two knobs disable auto-reauth:

```sh
# One-shot (CI runner that wants stale failures to surface as 12):
inspect run arte/api --no-reauth -- ./migrate.sh

# Persistently for a namespace:
inspect connect arte --set-auto-reauth false
# (or hand-edit ~/.inspect/servers.toml: `[namespaces.arte]\nauto_reauth = false\n`)
```

When auto-reauth is disabled and the dispatch hits `transport_stale`,
the SUMMARY trailer carries the chained recovery hint:

```
SUMMARY: run: 0 ok, 1 failed (ssh_error: stale connection — run
  'inspect disconnect arte && inspect connect arte' or pass --reauth)
```

The structured `--json` output gains a final `phase=summary`
envelope per `run`/`exec` invocation:

```json
{"_schema_version":1,"_source":"run","_medium":"run","server":"arte/api",
 "phase":"summary","ok":0,"failed":1,"failure_class":"transport_stale"}
```

---

## 16. Configuration reference

| Path | Purpose |
|---|---|
| `~/.inspect/config.toml` | namespaces, defaults, global aliases |
| `~/.inspect/profiles/<ns>.yaml` | discovered topology per namespace, mode 0600 |
| `~/.inspect/recipes/*.yaml` | user-defined recipes |
| `~/.inspect/audit/<YYYY-MM>-<user>.jsonl` | audit log |
| `~/.inspect/audit/snapshots/<sha>` | original-content snapshots for revert |

Useful environment variables:

| Variable | Effect |
|---|---|
| `INSPECT_<NS>_HOST` / `_USER` / `_KEY_PATH` | non-interactive `add` |
| `INSPECT_MAX_PARALLEL` | fleet concurrency cap (default 4) |
| `INSPECT_PREFIX` | install root for the one-line installer |
| `INSPECT_VERSION` | pin a version for the installer |
| `INSPECT_DOCKER_INSPECT_TIMEOUT` | seconds; pin a fixed `docker inspect` batch budget. Bypasses the v0.1.3 inventory-scaled formula `max(10s, 250ms × container_count)` capped at 60s. See [RUNBOOK §8](RUNBOOK.md#82-inventory-scaled-timeout-formula). |
| `INSPECT_DEBUG` / `RUST_LOG=debug` | surface debug-level discovery diagnostics (e.g. slow-but-successful `docker inspect` rounds) on stderr |
| `NO_COLOR` | disable ANSI in output |

---

## 17. Troubleshooting

| Symptom | First check | Likely cause |
|---|---|---|
| `inspect` exits 2 with `ssh: connection refused` | `~/.ssh/config` has the namespace host | namespace not configured locally |
| Empty `ps` output, no error | `inspect setup <ns> --force` | stale or missing profile, or Docker socket not reachable |
| `cargo build` failure on a fresh clone | rust-toolchain pin (1.75 minimum) | MSRV drift |
| Hung command, no output | `SIGINT` once; check `inspect why <selector>` | SSH ControlMaster stall |
| Slow first results across many servers | `INSPECT_MAX_PARALLEL=8 inspect …` | concurrency cap |
| Secrets visible in `--json` output | open a P0 issue | redactor bug — this is a contract |
| Selector "matches nothing" | the diagnostic lists what is available | typo, drifted profile, or wrong namespace |
| `inspect watch` exits **124** | predicate genuinely not met within `--timeout`, or interval too long | raise `--timeout`, lower `--interval`, or check the predicate by hand |
| `inspect bundle apply` exits **4** | rollback itself failed — bundle is in a mixed state | inspect `audit ls --bundle <id>`, finish the rollback by hand |

For maintainer-side incident handling, see [RUNBOOK.md](RUNBOOK.md) §3.

---

## 18. Translation guide (grep / stern / ssh / sed)

You probably already know the shape of the work. Here's how it maps.

| You usually do | With `inspect` |
|---|---|
| `grep -i "error" file.log` | `inspect grep "error" arte/atlas:/var/log/atlas.log -i` |
| `stern --since 30m pulse` | `inspect logs arte/pulse --since 30m` |
| `kubectl logs <pod> --since=30m \| grep -i error` | `inspect grep "error" arte/pulse --since 30m -i` |
| `ssh box "docker logs pulse --since 30m \| grep error"` | `inspect grep "error" arte/pulse --since 30m` |
| `ssh box "sudo sed -i 's/old/new/' /etc/foo.conf"` | `inspect edit arte/_:/etc/foo.conf 's/old/new/' --apply` |
| `scp ./file.conf box:/etc/file.conf` | `inspect cp ./file.conf arte/_:/etc/file.conf --apply` |
| `ssh box "docker restart pulse"` | `inspect restart arte/pulse --apply` |
| Loki: `{job="varlogs"} \|= "error"` | `inspect search '{server="arte", source="logs"} \|= "error"'` |

For a longer cookbook (and worked metric queries):
`inspect help examples`.
