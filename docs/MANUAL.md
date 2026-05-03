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

### 3.6 Drift detection (B4 + L10, v0.1.3)

`inspect setup --check-drift` compares the live host against the
cached profile without re-discovering. Output is structured so
agents can branch on it without parsing prose:

- **Container-level (B4, v0.1.2)**: `added` / `removed` /
  `changed` (image bumped in place, same container id).
- **Port-level (L10, v0.1.3)**: `port_changes` array with four
  `kind` values:

  | kind | meaning | example |
  |---|---|---|
  | `added` | port present in live, absent in cached | new `:443/tcp` exposed |
  | `removed` | port present in cached, absent in live | `:11211/tcp` closed |
  | `bind` | same `(container_port, proto)`, different host | `5432:5432/tcp` → `5433:5432/tcp` (collision dodge) |
  | `proto` | same `(host, container_port)`, different proto | DNS flipped from `:53/tcp` to `:53/udp` |

Worked example:

```sh
$ inspect setup arte --check-drift
SUMMARY: drifted (current=...  cached=...)
DATA:
  ~1 container changed:
    api (img:1 → img:2)
  ⚓2 port-level changes:
    db   bind  (5432:5432/tcp → 5433:5432/tcp)
    dns  proto (53:53/tcp → 53:53/udp)
NEXT:    inspect setup arte    (refresh the cached profile)
```

`--json` emits the same structure as a stable envelope:

```json
{
  "added": [], "removed": [],
  "changed": [{"name":"api","from":"img:1","to":"img:2"}],
  "port_changes": [
    {"container":"db","kind":"bind",
     "before":{"host":5432,"container":5432,"proto":"tcp"},
     "after":{"host":5433,"container":5432,"proto":"tcp"}},
    {"container":"dns","kind":"proto",
     "before":{"host":53,"container":53,"proto":"tcp"},
     "after":{"host":53,"container":53,"proto":"udp"}}
  ]
}
```

The cheap probe captures `{{.Ports}}` per container in the same
ssh round-trip used for ids / names / images, so adding port-level
diff costs zero extra round-trips. The parser handles every
`docker ps` Ports shape collected from the field corpus: IPv4
(`0.0.0.0:5432->5432/tcp`), bracketed IPv6 (`[::]:53->53/udp`),
ranges (`0.0.0.0:8000-8002->8000-8002/tcp`, expanded to N records),
unbound exposed ports (`5432/tcp`, recorded as `host: 0`), and
comma-separated lists. Unrecognized tokens are silently dropped —
better to under-report than to mis-report a port change that
didn't happen.

Container-level adds / removes do **not** also fan their per-port
deltas into `port_changes` — the container-level entry implies
its ports moved with it. Only containers present in **both**
snapshots contribute to `port_changes`.

UDP port changes flow through the same path as TCP (L9 made
`proto: "udp"` first-class on the cached side; the parser already
understood `/udp` tokens).

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

`inspect ports <ns>` accepts three server-side filters so you don't
have to pipe through `grep` (and lose the SUMMARY/NEXT envelope):

- `--port <n>` — keep only rows mentioning a specific port number.
- `--port-range <lo-hi>` — keep only rows in `[lo, hi]` (inclusive).
- `--proto tcp|udp|all` (L9, v0.1.3) — narrow to one transport.
  Default `all` runs both TCP (`ss -tlnp` / `netstat -tlnp`) and
  UDP (`ss -ulnp` / `netstat -ulnp`) probes in one ssh round-trip;
  `tcp` or `udp` skips the other probe entirely.

`--port` and `--port-range` are mutually exclusive; `--proto`
composes with both. The token-aware matcher handles both the
`0.0.0.0:8200` and `8200/tcp` shapes, so it doesn't fire on
incidental digits inside an interface name or a netns label. The
SUMMARY's "N listener(s)" count reflects the filtered total, not
the raw row count.

Each emitted row prefixes the data line with `[tcp]` or `[udp]`
so the proto is visible at a glance, and the JSON envelope carries
an explicit `proto` field on every row (matches the host-listener
records cached in the profile).

```sh
inspect ports arte --port 8200
inspect ports arte --port-range 8000-8999
inspect ports arte --proto udp                 # DNS forwarders, syslog receivers, etc.
inspect ports arte --proto tcp --port-range 8000-8999
```

**Why UDP matters (L9, v0.1.3).** Pre-L9 the host-listener probe
scanned only TCP, so UDP services on managed appliances (DNS
forwarders, mDNS responders, syslog receivers on `:514/udp`,
IPSec daemons, WireGuard endpoints) were invisible to `inspect
ports` and `inspect status`. The v0.1.3 probe runs both axes and
tags every cached listener record with `proto`. UDP listeners
shown by `ss -uln` are *bound sockets*, not "the service is
actually receiving traffic" — operators chasing dead UDP services
still need a real probe (e.g., `dig @host` for DNS); inventory is
necessary but not sufficient.

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

### 7.4 Output redaction (v0.1.1 + v0.1.3 expansion)

Every line streamed from a remote command runs through a
four-masker pipeline before reaching local stdout (or a JSON
envelope's `line` field). Applies to `run`, `exec`, `logs`, `cat`,
`grep`, `find`, `search`, `why`, and the merged follow stream.

The four maskers run in fixed order on every line:

1. **PEM private-key blocks (v0.1.3).** Recognized BEGIN forms:
   `-----BEGIN PRIVATE KEY-----` (PKCS#8),
   `-----BEGIN ENCRYPTED PRIVATE KEY-----`,
   `-----BEGIN RSA PRIVATE KEY-----` (PKCS#1),
   `-----BEGIN EC PRIVATE KEY-----`,
   `-----BEGIN DSA PRIVATE KEY-----`,
   `-----BEGIN OPENSSH PRIVATE KEY-----`, and
   `-----BEGIN PGP PRIVATE KEY BLOCK-----`. The BEGIN line emits
   one `[REDACTED PEM KEY]` marker; every interior line plus the
   matching END line is suppressed entirely. Public certificates
   (`-----BEGIN CERTIFICATE-----`) and public keys
   (`-----BEGIN PUBLIC KEY-----`) pass through unchanged.

2. **HTTP / cookie headers (v0.1.3).** Case-insensitive
   word-bounded match on `Authorization`, `X-API-Key`, `Cookie`,
   `Set-Cookie` followed by `:` — replaces the entire value
   portion with `<redacted>`. Catches `curl -v` traces and
   reverse-proxy logs. Word boundary on the name avoids false
   positives on prose like `MyAuthorization`.

3. **URL credentials (v0.1.3).** Masks the password portion of
   `scheme://user:pass@host` to `user:****@host`, preserving
   scheme, username, and host. Covers `postgres`, `mysql`,
   `redis`, `mongodb`, `mongodb+srv`, `amqp`, `http`, `https`,
   and any other scheme matching the userinfo grammar.

4. **`KEY=VALUE` env-var lines (v0.1.1).** Scans every line for
   `KEY=VALUE` and masks the value when the key looks like a
   secret. The mask form is `head4****tail2` (values shorter
   than 8 chars become `****`). The `export ` prefix and matching
   quote pairs are preserved.

   Recognized key shapes:

   - Suffixes: `_KEY`, `_SECRET`, `_TOKEN`, `_PASSWORD`, `_PASS`,
     `_CREDENTIAL[S]`, `_APIKEY`, `_AUTH`, `_PRIVATE`,
     `_ACCESS_KEY`, `_DSN`, `_CONNECTION_STRING`.
   - Exact: `DATABASE_URL`, `REDIS_URL`, `MONGO_URL`,
     `POSTGRES_URL`, `POSTGRESQL_URL`.

Inside an active PEM block, the other three maskers do not fire
on the suppressed lines — the entire block body is replaced with
the single marker. The header and URL maskers compose on a single
line (a `Cookie:` value containing a URL credential is masked
once by the header masker; the URL masker has nothing to do).

Opt-out flags (apply to **all four** maskers in one go):

- `--show-secrets` — verbatim output. On `exec`, this stamps
  `[secrets_exposed=true]` into the audit args so reviewers can
  tell apart verbatim from masked runs.
- `--redact-all` — mask **every** `KEY=VALUE` pair (env masker
  only; the other three already redact unconditionally on match).

When any of the four maskers fired during an `exec`, the audit
args captures `[secrets_masked=true]` and the JSONL audit entry's
`secrets_masked_kinds` field records the canonical ordered subset
(e.g. `["pem", "header"]` or `["url", "env"]`) so reviewers can
tell two redacted runs apart by *which* pattern almost leaked.

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

### 7.6 `inspect run --file` / `--stdin-script` (v0.1.3)

Multi-step bash heredocs with embedded `psql -c "..."`,
`python -c '...'`, and `cypher-shell <<CYPHER` blocks used to
require an escape pass at every shell layer crossed (your shell →
ssh → bash → docker exec → psql). Script mode replaces that with
"the script body never crosses a shell-parsing boundary on the
local side."

```sh
# Field-tested migration heredoc — zero local quote-escapes:
inspect run arte/atlas --file migrate-vault.sh

# Same script body via heredoc-on-stdin (the canonical form for
# scripts you don't want on disk):
inspect run arte/atlas --stdin-script <<'BASH'
set -euo pipefail
psql -c "SELECT 'embedded \"double\" quote';"
python3 -c 'print("hi from $")'
docker exec atlas-vault sh -c "vault operator step-down"
BASH

# Args after `--` become $1, $2, ... in the script:
inspect run arte/atlas --file deploy.sh -- v1.2.3 production
# remote: bash -s -- 'v1.2.3' 'production' (script's $1 / $2)
```

**How it works.** The script body rides in via the same byte-for-byte
stdin pipe F9 forwards on, and the remote command becomes
`bash -s -- <args>` (or `<interp> -` for non-bash interpreters
declared via shebang). No local shell beyond the one that invoked
`inspect` ever parses the script, so embedded quotes, `$`,
`\`, and heredocs survive untouched.

**Shebang dispatch.** A leading
`#!/usr/bin/env <interp>` or `#!/path/to/<interp>` line picks the
remote interpreter:

| Interpreter | Remote dispatch |
|---|---|
| `bash` / `sh` / `zsh` / `ksh` / `dash` | `<interp> -s -- <args>` |
| `python3` / `python` / `node` / `ruby` / ... | `<interp> - <args>` (POSIX `-` reads stdin) |
| `(none)` | `bash -s -- <args>` (default) |

Interpreter names are sanitized to `[A-Za-z0-9_.-]`; anything else
falls back to `bash` (defense against malformed shebangs).

**Container-targeted scripts.** Selectors that resolve to a
container render as `docker exec -i <ctr> <interp> -s …` — the `-i`
keeps stdin attached so the script body flows in.

**Audit.** Every script-mode invocation writes a per-step audit
entry with:

- `script_path` — absolute path of the local file (`null` for
  `--stdin-script`)
- `script_sha256` — hex SHA-256 of the body
- `script_bytes` — body length
- `script_interp` — selected interpreter
- `rendered_cmd` — the actual remote command line (e.g.
  `docker exec -i 'atlas' bash -s -- 'v1.2.3'`)

The body itself is dedup-stored at
`~/.inspect/scripts/<sha256>.sh` (mode 0600) so post-mortem
reconstruction works even if the operator deletes the local file.
With `--audit-script-body`, the body is also inlined under
`script_body` in the audit JSONL.

**Mutual exclusion.**

- `--file` and `--stdin-script` together → clap exit 2.
- `--file` or `--stdin-script` with `--no-stdin` → clap exit 2.
- `--stdin-script` with a tty (or empty stdin) → exit 2 with a
  chained `--file`-pointing recovery hint.

**Size cap.** Script-mode shares F9's `--stdin-max` budget
(default 10 MiB). Above the cap, exit 2 with the chained hint
pointing at `inspect put` (F15).

**Composes with the rest of v0.1.3.** Script mode dispatches
through the same SSH executor as bare `inspect run`, so the
namespace env overlay (F12), stale-session auto-reauth (F13), and
all output-shape contracts (F7.4 `--quiet`, F10.7 `--clean-output`,
F8 cache, F9 stdin audit) compose unchanged.

### 7.7 File transfer: `inspect put` / `inspect get` / `inspect cp` (F15, v0.1.3)

`inspect put <local> <ns>:/path` uploads a local file to a remote
path; `inspect get <ns>:/path <local>` downloads. Both ride the
persistent ControlPath master used by every other namespace verb,
so they inherit the namespace's auth, audit log, F11 revert
capture, F12 env overlay, and F13 stale-session auto-reauth.
`inspect cp` is the bidirectional convenience (operator types `cp`,
arg shape decides direction; the audit records the canonical
`put` / `get` verb).

```sh
# Upload a compose file edit to the host.
inspect put ./atlas.yml arte:/etc/compose/atlas.yml --apply

# Pull a config off the host for editing.
inspect get arte:/etc/compose/atlas.yml ./atlas.yml

# Container filesystem (selector names a service).
inspect put ./vault.hcl arte/atlas-vault:/etc/vault/config.hcl --apply

# Stream a small file straight to stdout.
inspect get arte:/etc/issue -

# bidirectional convenience — direction inferred from arg shape.
inspect cp ./fix.conf arte:/etc/svc.conf --apply       # → dispatches put
inspect cp arte:/var/log/syslog ./syslog --apply       # → dispatches get
```

**Selector forms.**
- `arte/_:/path` — host filesystem.
- `arte:/path` — F7.2 shorthand for `arte/_:/path`.
- `arte/<svc>:/path` — container filesystem; dispatched via
  `docker exec -i <ctr> sh -c '...'`.

Host vs container is decided unambiguously by the selector — never
by a flag.

**Atomic-write contract.** `put` writes through a `<path>.tmp`
sibling and atomically renames into place. The temp file inherits
mode + ownership from the prior file at `<path>` (via
`chmod --reference` / `chown --reference`) before the rename, so
edits never silently widen permissions on a `0600 root:root` config.
On a brand-new file (no prior to mirror from), the temp is created
with the SSH user's umask and owner.

**Flags on `put` / `cp`.**
- `--mode <octal>` — chmod the remote after upload (overrides the
  inherited mirror; e.g. `--mode 0755` to make a script executable).
- `--owner <user[:group]>` — chown the remote after upload.
  Requires the SSH user have permission to chown.
- `--mkdir-p` — create missing parent directories on the remote
  (`mkdir -p`) before writing. Without this, a missing parent dir
  surfaces as `error: remote parent directory does not exist` and
  the transfer aborts.

**Revert.** `put` captures a `revert.kind = state_snapshot` audit
entry when the target file exists, so `inspect revert <id>`
restores the prior content byte-for-byte from the snapshot store.
On a brand-new file (no prior content), the inverse is a
`command_pair` `rm -f -- <path>` so revert deletes the
freshly-created file. `get` is read-only on the remote, so its
`revert.kind` is `unsupported` (the operator deletes the local
file to undo); the audit entry still records the bytes + sha256
so a later `put` of the same content is verifiable byte-for-byte.

**Audit fields.** Every transfer writes an audit entry with
`verb` ∈ `{put, get}` (canonical, even if the operator typed `cp`),
`transfer_direction` ∈ `{up, down}`, `transfer_local`,
`transfer_remote`, `transfer_bytes`, `transfer_sha256`. The bytes
+ sha are computed during the transfer so the audit log carries a
complete fingerprint of the content that crossed the boundary,
without storing the bytes themselves.

**Size cap.** None. The pre-F15 `cp` had a 4 MiB hard cap because
the body was base64-encoded into the command argv; F15 streams
through SSH stdin, so the only practical limits are the operator's
patience and the remote disk. Above 1 MiB, a one-line warning fires
to stderr (silence with `INSPECT_CP_WARN_BYTES=0`) because the
streaming transfer briefly monopolises the multiplexed channel.

**Out of scope for v0.1.3.** `--since <duration>` / `--max-bytes
<size>` on `get` (deferred to v0.1.5; the dedicated `inspect logs
--since` already covers log-retrieval), `--resume` for partial
transfers (deferred to v0.1.5; chunked-protocol design pass).

### 7.8 Streaming long-running commands: `inspect run --stream` (F16, v0.1.3)

`inspect run --stream` (alias `--follow`) line-streams the remote
command's stdout/stderr to local stdout as it arrives, instead of
buffering until the remote process exits. Use it for the long tail
of commands that produce output indefinitely until SIGINT —
`docker logs -f`, `tail -f /var/log/...`, `journalctl -fu vault`,
`python -m monitor`, anything that "never returns".

```sh
# Tail a container's logs until you Ctrl-C.
inspect run arte --stream -- 'docker logs -f atlas-vault'

# Same thing, with the --follow alias for muscle-memory parity.
inspect run arte --follow -- 'tail -f /var/log/syslog'

# Watch a long migration step in real time.
inspect run arte --stream -- '/usr/local/bin/migrate-vault.sh'
```

**What `--stream` does.** It forces SSH PTY allocation (`ssh -tt`)
on the dispatch. Two effects flow from the PTY:

1. **Line-buffered remote output.** Most CLI tools (`grep`, `awk`,
   `python`, ...) detect "stdout is a TTY" via `isatty(1)` and switch
   from block-buffered (4 KB chunks) to line-buffered output. With
   `--stream`, the remote tool sees a PTY and flushes line-by-line,
   so output appears locally in real time instead of in bursts.
2. **End-to-end Ctrl-C.** Local Ctrl-C (SIGINT) reaches the local
   `ssh` client, which forwards it through the PTY layer to the
   remote process. The command actually dies on the remote — it is
   not orphaned the way it would be without a PTY (where the local
   ssh would close the channel on Ctrl-C and the remote process
   would inherit `init` as its parent and keep running).

**Default timeout: 8 hours under `--stream`.** Streaming runs are
expected to terminate via Ctrl-C, not by reaching the per-target
timeout, so the default is bumped from the bare-`run` 120 s to
match `inspect logs --follow`. Override with `--timeout-secs <N>`
either way.

**Audit-field shape.** Every `--stream` invocation produces a run
audit entry with `streamed: true`, `failure_class`, the
`rendered_cmd`, and the wall-clock `duration_ms`. Pre-F16
`inspect run` was un-audited unless stdin was forwarded (F9) or
the dispatch wrapper retried under F13; `--stream` joins those
triggers, so a multi-hour migration's `--stream` blocks are
recoverable from the audit log alone. Non-streaming runs
*omit* the `streamed` field entirely (it is `Option<T>` with
`skip_serializing_if = "is_false"`), so audit tooling that filters
on `streamed` catches only the long-running invocations.

**Composes with `--file` (F14) and `--stdin-script` (L11, v0.1.3).**
Both script-mode sources work alongside `--stream`. `--stream
--file <script>` delivers the script body in one shot via `bash
-s`, then the running script's output streams back.

L11 (v0.1.3): `--stream --stdin-script` now composes via two-phase
dispatch — pre-L11 the combo was clap-rejected because feeding
the script body via SSH stdin and forcing `-tt` PTY for streaming
output put both directions through the same tty layer (line-
discipline echo, cooked-mode munging, interactive bash prompts on
a non-tty stdin). L11 splits the dispatch in two so the
directions never interleave:

  Phase 1 (write).   ssh + `cat > /tmp/.inspect-l11-<sha>-<pid>.sh
                     && chmod 700 <…>` with the script body piped
                     via stdin. No PTY; one ssh round-trip;
                     `umask 077` ensures the file is operator-only.
  Phase 2 (run).     ssh -tt + `<interp> <tempfile> -- <args>` with
                     PTY for line-streaming output. No stdin
                     payload (already on disk).
  Phase 3 (cleanup). ssh + `rm -f <tempfile>`. Runs unconditionally
                     after phase 2 so a non-zero script exit leaves
                     no orphan; failures here are warnings (the
                     verb has already produced its output).

The remote temp filename includes the script's SHA-256 prefix +
the local PID, so concurrent `inspect run` invocations on the
same script never collide. Container selectors take the same
shape with `docker exec -i <ctr> sh -c '…'` wrapping each phase.

```sh
$ cat ./long-script.sh | inspect run arte --stream --stdin-script
# phase 1: writes /tmp/.inspect-l11-ab1c2d3e-12345.sh
# phase 2: streams the script's output line-by-line
# phase 3: cleans up the temp file
```

The audit entry stamps `bidirectional: true` alongside the
existing `streamed: true` and `script_sha256` fields so post-
mortem queries can identify L11 invocations:
`inspect audit ls --bidirectional`. `--stream --file <script>`
also takes the L11 path when the file would otherwise need to be
piped via stdin (see `inspect run --help` for the full
flag-composition matrix).

**`inspect logs --follow` interop (no overlap).** F16 does **not**
replace `inspect logs --follow` — the dedicated logs verb keeps its
existing semantics (selector-aware, source-tier-aware, structured
output, `--since` / `--match` / `--merged`). F16 is for the case
when the operator wants streaming for a non-logs command, or for a
multi-step script that includes a streaming step. If the question
is "tail a container's logs," reach for `inspect logs <ns>/<svc>
--follow`; if it is "tail anything else," `inspect run --stream` is
the right hammer.

**Limitations.** `--stream` relies on `ssh -tt` for SIGINT
propagation, which works for every remote process that respects
SIGINT. The corner case of a process that ignores SIGINT but exits
on SIGTERM/SIGHUP (rare; mostly daemonised services) is not
handled in v0.1.3 — escalation to channel-close on a second
Ctrl-C is on the v0.1.5 polish list. The PTY can also alter
output formatting (CRLF line endings, color codes) for tools that
key off `isatty(1)`; if a downstream pipe needs strictly
LF-terminated bytes, capture under bare `inspect run` instead of
`--stream` or post-process with `tr -d '\r'`.

### 7.9 Multi-step runner: `inspect run --steps` (F17, v0.1.3)

`inspect run --steps <manifest.json>` (or `--steps -` to read the
manifest from stdin) dispatches an ordered list of steps against a
single resolved target, returning structured per-step exit codes
that an LLM-driven wrapper can reason about — fixing the long-time
problem that a 5-line heredoc whose step 3 failed still reported
`1 ok, 0 failed` because the outer `bash -c` exited 0.

**Manifest shape.** A JSON object with a single `steps` array:

```json
{
  "steps": [
    {"name": "snapshot",  "cmd": "tar czf - /data > /tmp/snap.tgz",
     "on_failure": "stop"},
    {"name": "stop-app",  "cmd": "docker compose stop app",
     "on_failure": "stop", "revert_cmd": "docker compose start app"},
    {"name": "migrate",   "cmd_file": "./migrate.sh",
     "on_failure": "stop", "timeout_s": 600},
    {"name": "start-app", "cmd": "docker compose start app",
     "on_failure": "stop", "revert_cmd": "docker compose stop app"},
    {"name": "verify",    "cmd": "curl -fsS http://localhost/health",
     "on_failure": "continue"}
  ]
}
```

Per-step fields:

- **`name`** (required, must be unique within the manifest) —
  used in output blocks, the STEPS table, and the per-step audit
  entry's `step_name` field.
- **`cmd`** (required unless `cmd_file` is set) — a `bash -c`-shaped
  command body, dispatched against the target with the F12 env
  overlay applied.
- **`cmd_file`** (alternative to `cmd`) — path to a local script
  file. F14 composition: the file is read and shipped via
  `bash -s`, with `script_sha256` + `script_bytes` + `script_path`
  recorded on the per-step audit entry exactly as `inspect run
  --file` would.
- **`on_failure`** (optional, default `"stop"`) — `"stop"` aborts
  the pipeline on first non-zero exit; `"continue"` records the
  failure and proceeds.
- **`timeout_s`** (optional, default 8 hours) — per-step wall-clock
  cap in seconds. Reuses the executor's existing timeout
  mechanism (the SSH child is killed + drained on overrun).
- **`revert_cmd`** (optional) — declared inverse for the F11
  composite revert. When set, the per-step audit entry records
  `revert.kind = "command_pair"` with this string as the payload.
  When absent, the per-step `revert.kind = "unsupported"` (the
  step's `cmd` is a free-form bash body with no general inverse)
  and `--revert-on-failure` skips it with a warning.

**Per-step output (human format).** Without `--stream`, each step
emits a `STEP <name> ▶` opening marker, then its captured output
line-by-line with the `<ns> | …` prefix, then a `STEP <name> ◀
exit=N duration=Ms` closing marker.

L12 (v0.1.3): under `--stream`, the boundaries switch to the F18
transcript fence format so the live tail of a multi-step
migration matches the per-day transcript fence shape exactly:

```
── step 1 of 3: snapshot ──
arte/atlas | tarring atlas_milvus...
arte/atlas | done.
── step 1 ◀ exit=0 duration=12300ms audit_id=01HXR9Q5YQK2 ──
── step 2 of 3: stop-app ──
arte/atlas | Stopping atlas-app ... done
── step 2 ◀ exit=0 duration=1100ms audit_id=01HXR9Q66... ──
── step 3 of 3: migrate ──
…
```

The audit_id on each step closer cross-links back to that step's
`run.step` audit entry; copy-paste it into `inspect audit show`
without further translation.

L12 also wires the L7 redaction pipeline (PEM → header → URL →
env) into the per-step live tee, so a step that emits a
`Bearer <token>` header, a `postgres://user:pass@host` URL, or a
PEM private-key block has the secret masked BEFORE it reaches
the operator's terminal AND before it lands in the captured
`targets[].stdout` audit field. `--show-secrets` bypasses every
masker (same contract as `inspect run`).

After every step has run (or a stop-on-failure step has aborted),
a STEPS summary table prints:

```
SUMMARY: STEPS: 5 total, 4 ok, 1 failed, 0 skipped
DATA:
  ✓ snapshot     exit=0   duration=12300ms
  ✓ stop-app     exit=0   duration=1100ms
  ✗ migrate      exit=1   duration=8700ms (stopped pipeline)
  · start-app    exit=0   duration=0ms
  · verify       exit=0   duration=0ms
  skipped: start-app, verify
NEXT:
  step 'migrate' aborted the pipeline; inspect audit show <id> for the full table
  inspect revert <id> to walk the composite inverse
```

**Per-step output (`--json`).** A single structured JSON object:

```json
{
  "v": 1,
  "ns": "arte/atlas",
  "verb": "run.steps",
  "steps_run_id": "1714571234567-a3f2",
  "manifest_sha256": "<64 hex chars>",
  "target_labels": ["arte/atlas"],
  "steps": [
    {
      "name": "snapshot",
      "cmd": "...",
      "status": "ok",
      "targets": [
        {"label": "arte/atlas", "exit": 0, "duration_ms": 12300,
         "stdout": "...", "status": "ok", "audit_id": "..."}
      ]
    }
  ],
  "summary": {"total": 5, "ok": 4, "failed": 1, "skipped": 0,
              "stopped_at": "migrate", "auto_revert_count": 0,
              "target_count": 1}
}
```

This is the contract LLM-driven wrappers can reason about — no
prose parsing, no defensive markers. (Per-line `phase: "begin"` /
`line` / `phase: "end"` envelopes also stream during dispatch for
live progress; the final summary record is the canonical post-run
shape.)

**Audit-log shape.** Every `--steps` invocation produces:

- One **parent** audit entry (`verb: "run.steps"`) with
  `revert.kind = "composite"`, `manifest_sha256`, `manifest_steps`
  (the ordered name list), and `steps_run_id` set to the parent's
  own id.
- One **per-step** audit entry per dispatched step (`verb:
  "run.step"`), each with the same `steps_run_id`, the step's
  `step_name`, its captured `rendered_cmd`, exit code, duration,
  and per-step `revert.kind` (`"command_pair"` if the manifest
  declared a `revert_cmd`, else `"unsupported"`).
- One **auto-revert** audit entry per inverse executed under
  `--revert-on-failure` (`verb: "run.step.revert"`), with
  `is_revert: true`, `auto_revert_of: <original-step-id>`, and
  `reverts: <original-step-id>`.

Post-hoc: `inspect audit show <steps_run_id>` displays the parent
record (the per-step entries can be joined by filtering audit on
the matching `steps_run_id`); `inspect revert <steps_run_id>`
walks the parent's composite payload in reverse manifest order
and dispatches each per-step inverse — same dry-run gate as every
other write verb.

**`--revert-on-failure`.** Requires `--steps`. When a step fails
with `on_failure: "stop"`, the runner walks the inverses of the
steps that already ran (Ok or Failed status) in reverse manifest
order **in the same invocation** and dispatches each as its own
audit-logged auto-revert. Steps with no declared `revert_cmd` are
skipped with a one-line warning rather than aborting the unwind.
This is the migration-operator's missing primitive: a 5-step
manifest where step 3 fails with `--revert-on-failure` correctly
unwinds steps 1 and 2 without a separate `inspect revert`
invocation.

**Composes with the rest of v0.1.3.** F11 (revert contract:
composite payload + per-step capture); F12 (env overlay applied
per (step, target)); F13 (auto-reauth wraps each per-(step,
target) dispatch — a stale-socket failure mid-pipeline triggers
transparent reauth + retry on the failing pair without aborting
the rest of the pipeline); F14 (`cmd_file` rides the same
`bash -s` + script-store path as `inspect run --file`); F16
(`--steps --stream` forces PTY allocation on every per-step
dispatch — live line-buffered output, end-to-end Ctrl-C
propagation through the active step's PTY, per-step + parent
audit entries record `streamed: true`).

**YAML input (`--steps-yaml <PATH>`).** Same manifest schema as
`--steps`, just YAML-encoded. Convenient for operators who keep
their migration manifests alongside CI/CD pipelines. Mutex with
`--steps`. Both flags belong to a clap `manifest_source` ArgGroup
so `--revert-on-failure` accepts either.

**`--reason` plumb-through.** `--reason "<text>"` echoes to
stderr at start (matching bare `inspect run` semantics) AND
stamps onto the parent `run.steps` audit entry's `reason` field.
After a 4-hour migration, the operator's intent is recoverable
from the audit log alone — no terminal scrollback required.

**Per-step output cap: 10 MiB per (step, target).** Live printing
is unaffected; only the captured copy that feeds the audit + JSON
output stops growing past the cap and stamps `output_truncated:
true` on the per-target result. Protects the local process from
OOM on a step that emits many GB. Cap matches the F9
`--stdin-max` default for consistency.

**Multi-target dispatch (v0.1.3).** When the selector resolves to
N>1 targets, each manifest step fans out across all N targets
**sequentially within the step**. The step's aggregate `status`
is `ok` only if every target succeeded; `failed` if any target's
exit was non-zero; `timeout` if any target overran `timeout_s`.
`on_failure: "stop"` applies globally — any target's failure
aborts the next manifest step on every target. Each (step,
target) pair writes its own `run.step` audit entry with the
target's label as the entry's `selector`; `--revert-on-failure`
fans the inverse out across every target the step ran on. The
JSON output's per-step record has a `targets[]` array (with
`label`, `exit`, `duration_ms`, `stdout`, `stderr`,
`output_truncated`, `status`, `audit_id`, `retried`); the
summary's `target_count` exposes N. Multi-target is sequential
within each step in v0.1.3; parallel fan-out within a step is
intentionally out of scope (output interleaving + audit-link-
ordering races would need a separate design pass).

**Mutex with `--file` / `--stdin-script` / `--steps-yaml` ↔
`--steps`** — clap rejected. Mixing `--steps` with `--file` would
be ambiguous (which script body wins?); `--steps` and
`--steps-yaml` are two spellings of the same flag.

---

### 7.10 First-class compose verbs: `inspect compose` (F6, v0.1.3)

Compose is the dominant deployment shape on the hosts inspect
targets, but v0.1.2 treated compose projects as opaque collections
of containers. To inspect a compose project's effective config or
restart a single service, operators dropped back to:

```sh
inspect run arte -- 'cd /opt/luminary-onyx && sudo docker compose ps'
```

…and lost structured output, audit trail, redaction, and selector
grammar in the process. F6 ships a complete first-class compose
sub-verb cluster so this fallback is no longer the path of least
resistance.

**Discovery.** `inspect setup <ns>` now runs `docker compose ls
--all --format json` and caches a `compose_projects: [...]` list on
the namespace's profile (one entry per project: `name`, `status`,
`compose_file`, `working_dir`, `service_count`, `running_count`).
The discovery probe is best-effort: hosts without `docker compose`
return an empty list silently. `inspect compose ls --refresh` re-
probes live without waiting for the next setup.

**Selectors.** The compose sub-verbs use a slightly narrower
grammar than the generic verbs:

```
<ns>                        for `compose ls`
<ns>/<project>              for `compose ps`, `compose config`,
                            aggregated `compose logs`,
                            `compose restart --all`,
                            `compose up`, `compose down`,
                            project-wide `compose pull` / `compose build`
<ns>/<project>/<service>    for narrowed `compose logs`,
                            per-service `compose pull` / `compose build`,
                            the safe `compose restart`,
                            and `compose exec`
```

The existing `<ns>/<service>` form continues to work for the
generic verbs (`inspect logs`, `inspect restart`) because F5's
resolver tries the compose service label first.

**Read sub-verbs** (no audit, no apply gate):

```sh
inspect compose ls arte                          # list projects
inspect compose ls arte --refresh                # bypass cache
inspect compose ps arte/luminary-onyx            # per-service table
inspect compose config arte/luminary-onyx        # merged YAML, redacted
inspect compose logs arte/luminary-onyx --tail 200
inspect compose logs arte/luminary-onyx/onyx-vault --follow
```

`config` runs every line through the L7 four-masker pipeline
(PEM / header / URL / env), so secret-shaped values in
`environment:` blocks are masked unless you pass `--show-secrets`.
`logs` uses the same redactor on every emitted line.

**Write sub-verbs** (audited; require `--apply`):

```sh
inspect compose up arte/luminary-onyx --apply
inspect compose down arte/luminary-onyx --apply --yes
inspect compose down arte/luminary-onyx --volumes --apply --yes-all
inspect compose pull arte/luminary-onyx --apply
inspect compose pull arte/luminary-onyx/onyx-vault --apply
inspect compose build arte/luminary-onyx --no-cache --apply
inspect compose restart arte/luminary-onyx/onyx-vault --apply
inspect compose restart arte/luminary-onyx --all --apply --yes-all
```

Each write records an audit entry with `verb=compose.<sub>` and
the bracketed-tag `args` field:

```
[project=<name>] [service=<name>] [compose_file_hash=<sha-12>] [...]
```

`compose_file_hash` is the first 12 hex chars of the project's
compose file SHA-256, fetched via `cat <compose_file>` at audit
time. A post-mortem can verify the compose file did not change
between the audit and a re-run by re-hashing it on the host and
comparing prefixes.

`pull` and `build` stream their output via the streaming-capturing
runner so multi-minute pulls and 30+-minute builds remain visible
in real time. `up` / `down` / `restart` are buffered.

`down --volumes` is **destructive**: it removes named volumes.
The dry-run preview surfaces a `(DESTRUCTIVE: --volumes would
remove named volumes)` warning when applicable. Pair with
`--apply --yes-all` only after you've verified the volume
contents are recoverable.

**Restart's defensive default.** Without a service portion,
`compose restart` refuses to fan out unless `--all` is passed:

```sh
$ inspect compose restart arte/luminary-onyx
error: selector 'arte/luminary-onyx' targets the whole project — pass
--all to confirm restarting every service, or narrow to
'arte/luminary-onyx/<service>'.
hint: `inspect compose ps arte/luminary-onyx` lists the services in
this project.
```

The intent is "you didn't tell me which service, prove you really
mean every service." With `--all`, restart enumerates services via
`docker compose -p <p> config --services` and iterates per-service
so each gets its own audit entry.

**`compose exec` is `inspect run`-style** — no audit, no apply gate,
output redacted unless `--show-secrets`:

```sh
inspect compose exec arte/luminary-onyx/onyx-vault -- ps -ef
inspect compose exec arte/luminary-onyx/onyx-vault -u root -- df -h
inspect compose exec arte/luminary-onyx/onyx-vault -w /app -- bash -c 'ls && env'
```

Use the audited write verbs (`compose restart`, `compose up`,
`compose down`) for state mutations; use `compose exec` for
inspection and fast iteration inside a running service container.

**Revert kind = `unsupported`.** All five compose write verbs
record `revert.kind = unsupported` in their audit entries. The
preview field names the rollback command when one exists
(e.g. `compose down` for a `compose up` audit), so
`inspect revert <id>` returns useful chained hints rather than
silently no-opping.

**`inspect status` integration.** Status reads
`profile.compose_projects` for every selected namespace and emits
a `compose_projects: N` line in the human DATA section (omitted
when N=0 to avoid noise on plain container hosts). The `--json`
output always includes a `compose_projects` array with the same
shape as `compose ls --json`, so agents navigate without
re-binding.

**JSON schemas.** See `inspect help compose` for the full
per-sub-verb JSON schema reference.

**Per-service narrowing (L8, v0.1.3).** `compose up`/`down`/
`pull`/`build` now accept `<ns>/<project>/<service>` for one-service
operations:

```sh
inspect compose up arte/luminary-onyx/onyx-vault --apply       # bring up just one service
inspect compose down arte/luminary-onyx/onyx-vault --apply     # stop + rm just one service
inspect compose pull arte/luminary-onyx/onyx-vault --apply     # pull just one image
inspect compose build arte/luminary-onyx/onyx-vault --no-cache --apply
```

Per-service `compose down` uses the explicit
`docker compose -p <p> stop <svc> && rm -f <svc>` shape rather than
`compose down <svc>` (which behaves inconsistently across compose
versions). Other services in the project remain running. The verb
refuses `--volumes` and `--rmi` on per-service selectors — both are
project-scoped and silently honoring them against one service would
either no-op (confusing) or wipe data shared with siblings (worse).

Audit args carry `[service=<svc>]` alongside the existing
`[project=<p>] [compose_file_hash=<sha>]` so post-mortem queries
can filter for the per-service slice.

**`compose logs` triage surface (L8, v0.1.3).** The verb now
matches `inspect logs`'s F8 contract:

```sh
inspect compose logs arte/luminary-onyx --tail 200 --match ERROR --exclude healthcheck
inspect compose logs arte/luminary-onyx --merged --follow         # explicit multi-service stream
inspect compose logs arte/luminary-onyx --cursor ./onyx.cursor --tail 200
```

`--match` / `--exclude` push down to a remote `grep -E` pipeline so
the SSH transport never carries lines we are about to drop.
`--merged` is an assertion flag — project-level form is already
interleaved with `[<service>]` prefixes (compose's default); the
flag rejects per-service selectors so the contract is unambiguous.
`--cursor <PATH>` resumes from the ISO-8601 timestamp recorded in
the cursor file, forces `--timestamps` on docker compose logs, and
writes the latest seen timestamp back atomically on stream end.
Mutex with `--since`.

**Bundle `compose:` step kind (L8, v0.1.3).** Bundle steps can
drive compose actions structurally:

```yaml
steps:
  - id: stop-api
    target: arte
    compose:
      project: luminary-onyx
      action: down                # up|down|pull|build|restart
      service: api                # optional
      flags:
        volumes: false
    rollback: |
      true
```

Plan-time validates the project against the namespace's cached
profile (project must exist) and rejects unknown flag keys per
action. Per-service `down` rejects `flags.volumes` / `flags.rmi`
(both project-scoped). Audit shape mirrors the standalone
`inspect compose <action>` verbs: `verb=compose.<action>`,
`args="[project=…] [service=…] [compose_file_hash=…]"`,
`revert.kind=command_pair` for up/down/restart/build (the inverse
points at the matching compose verb), `revert.kind=unsupported`
for pull. Bundle steps are still single-target — the bundle's
`host:` (or step's `target:`) supplies the namespace.

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

### 8.3 Retention and orphan-snapshot GC: `inspect audit gc` (L5, v0.1.3)

`~/.inspect/audit/` and its `snapshots/` subdirectory grow without
bound by default. A team running 50 mutations per day will accumulate
years of JSONL plus orphaned snapshot files. `inspect audit gc`
prunes both in one pass.

```sh
# Preview: delete entries older than 90 days, sweep orphan snapshots.
inspect audit gc --keep 90d --dry-run

# Apply.
inspect audit gc --keep 90d

# Or by entry count: keep newest 100 per namespace.
inspect audit gc --keep 100

# Machine-readable output for pipelines.
inspect audit gc --keep 90d --json
```

`--keep` takes either a duration suffix (`d`/`w`/`h`/`m`) or a bare
integer. Integer mode keeps the newest N entries **per namespace**
— namespace is derived from the entry's `selector` field
(`arte/atlas-vault` → `arte`); selector-less entries group under the
sentinel `_`. `--keep 0` is rejected: refusing to silently delete
every entry is the only safe default.

The `--json` envelope is stable and top-level (no `data` wrapper):

```json
{
  "_source": "audit.gc",
  "_medium":  "audit",
  "server":   "local",
  "dry_run":  false,
  "policy":   "90d",
  "entries_total":           240,
  "entries_kept":            127,
  "deleted_entries":         113,
  "deleted_snapshots":        47,
  "freed_bytes":         1572864,
  "deleted_ids":            [...],
  "deleted_snapshot_hashes":[...]
}
```

`freed_bytes` covers both JSONL shrinkage (rewritten in place via
atomic `tmp.gctmp.<pid>` → `rename(2)`; fully-emptied files are
unlinked) and snapshot file sizes, so an operator can size the next
retention window from the report alone.

**The pinned-snapshot invariant is non-negotiable.** A snapshot under
`~/.inspect/audit/snapshots/sha256-<hex>` is **never** deleted while
any retained audit entry references it via `previous_hash`,
`new_hash`, the `snapshot` filename, or a `revert.payload` (state
snapshot kind, including nested `state_snapshot` records inside the
F17 composite-revert JSON array). That is the F11 revert contract,
and the GC enforces it as the only invariant the operator cannot
relax.

#### Automatic GC via `~/.inspect/config.toml`

Set `[audit] retention` once and the GC fires lazily on every audit
append:

```toml
# ~/.inspect/config.toml
[audit]
retention = "90d"   # or "100" for newest-N-per-namespace; unset = manual only
```

The trigger is gated by a once-per-minute cheap-path marker
(`~/.inspect/audit/.gc-checked`): the marker's mtime acts as a
debounce, and within the cheap path only the *oldest* JSONL file's
mtime is probed against the retention threshold. If the oldest is
fresher than the cutoff, the GC no-ops without scanning the directory.
Errors from the lazy path are deliberately swallowed so a transient
GC failure can never break the just-appended audit record. Manual
`inspect audit gc` and the lazy trigger share the same code path —
the only difference is who calls it.

The global config file is distinct from per-namespace `servers.toml`:
`~/.inspect/config.toml` is reserved for cross-cutting policy that is
not keyed on a server. A missing file is not an error — the lazy GC
simply stays off until you opt in.

### 8.4 Session transcripts: `inspect history` (F18, v0.1.3)

The structured audit log (`~/.inspect/audit/`) answers *what verbs
ran with what arguments + what changed*. The session transcript at
`~/.inspect/history/<ns>-<YYYY-MM-DD>.log` answers *what did the
operator see on their terminal*. The two are complementary; F11
revert and L5 retention work against the audit log; F18 transcripts
make 4-hour migrations queryable after the fact.

```sh
# Today's transcript for one namespace.
inspect history show arte

# A specific past day (transparently decompresses .log.gz).
inspect history show arte --date 2026-04-28

# Find the block that ran a specific destructive command.
inspect history show arte --grep 'docker volume rm'

# Cross-reference from a structured audit hit back to the operator
# transcript block.
inspect history show --audit-id 01HXR9Q5YQK2

# List every transcript file with byte sizes.
inspect history list

# Apply the [history] retention now (otherwise it fires lazily once
# per day from the next verb's finalize).
inspect history rotate

# Delete a namespace's pre-cutoff history (audit log untouched).
inspect history clear arte --before 2026-01-01 --yes
```

**Format.** Each verb invocation produces one fenced block:

```text
── 2026-04-28T14:32:11Z arte #b8e3a1 ──────────────────────────
$ inspect run arte -- 'docker ps --format "{{.Names}}"'
arte | atlas-vault
arte | atlas-pg
── exit=0 duration=423ms audit_id=01HXR9Q5YQK2 ──
```

The `── … ──` fence pattern is `awk '/^── /,/^── exit=/'`-friendly
so block extraction is trivial without a parser. The trailing
`audit_id=` cross-links back to the structured audit entry —
forensic round-trip is one `inspect audit show <id>` away.

**Scope.** "Every verb invocation **against a namespace**" gets a
transcript. Operator-tooling verbs (`inspect help`, `inspect list`,
`inspect audit ls`, `inspect history ...` itself) produce no
transcript file — they would always be near-empty. Verbs that
resolve a namespace via `verbs::runtime::resolve_target` (run,
status, logs, why, exec, edit, cat, grep, find, restart, ports,
etc.) produce one block; verbs that handle namespaces directly
(`inspect cache clear <ns>`) call `transcript::set_namespace`
explicitly so they end up in the right per-ns file too.

**Captured surface.** The transcript reflects what the terminal
showed: SUMMARY/DATA/NEXT envelopes, JSON envelopes (one block per
invocation, even multi-line JSONL pipelines), streaming line-by-line
output from F16 `--stream` and `inspect logs --follow`, F14 script
mode (script body referenced by sha256 + stored separately under
`~/.inspect/scripts/`), F15 file transfers (paths + sizes + sha256
in argv line, transferred bytes not in body), F17 multi-step blocks
per (step, target). `error::emit` tees stderr too, so failure cases
are captured.

**Redaction.** Every line tee'd to the transcript runs through the
L7 four-masker pipeline (PEM / Authorization / URL credentials /
KEY=VALUE) before being appended. Per-namespace
`[namespaces.<ns>.history].redact = "off"` writes raw lines to the
transcript file (file mode 0600 already restricts exposure — use
this for forensic dumps where you need raw output and trust local
disk security). `--show-secrets` on the originating verb bypasses
both stdout and transcript redaction.

```toml
# ~/.inspect/servers.toml
[namespaces.arte]
host = "arte.internal"
user = "ubuntu"

# Per-namespace transcript override.
[namespaces.arte.history]
disabled = false              # default false; true skips writes entirely
redact = "normal"             # "normal" (default) | "strict" | "off"
```

`disabled = true` skips the transcript write for this namespace
(the audit log is still written — F11 contract is independent).

**Retention.** `[history]` in `~/.inspect/config.toml`:

```toml
[history]
retain_days = 90              # default 90; older files deleted on rotate
max_total_mb = 500            # default 500; cap across all namespaces
compress_after_days = 7       # default 7; older files gzipped on rotate
```

`inspect history rotate` runs the full pass: deletes files older
than `retain_days`, gzips files older than `compress_after_days`,
evicts oldest first when total bytes exceed `max_total_mb`. A lazy
trigger fires once per day from the next verb's `finalize` so an
operator never has to remember to rotate. Today's transcript is
never gzipped or evicted — it's the active write target.

**Performance.** Output is buffered in memory during the verb and
written in one shot at finalize: **one `fdatasync(2)` per verb
invocation**, regardless of how much output the verb produced.
A 10-minute streaming verb produces exactly 1 fsync against the
transcript file. The buffer is capped at 16 MiB; overflow is
replaced with a `[transcript truncated: buffer cap reached]` marker.

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
  `abort` (default), `continue`, `rollback`, or
  `{ rollback_to: <step-id> }`.
- **Per-branch rollback (L6, v0.1.3).** When a `parallel: true` +
  `matrix:` step fails partway, rollback inverts ONLY the
  succeeded branches; failed and skipped branches log a
  `bundle.rollback.skip` audit entry with a why-skipped
  explanation. The `{{ matrix.<key> }}` reference in a
  `rollback:` block resolves to **each succeeded branch's
  value** — distinct from the forward `exec:` body which still
  receives the full per-branch matrix.
- **Rollback** walks completed reversible steps in reverse order.
  If a rollback action itself fails, the bundle exits with a clear
  "mixed state" warning naming the step.
- **`--apply` is mandatory** for any mutation. Without it, every
  mutating step is a dry-run, regardless of bundle contents.
- **Audit correlation:** every step writes an audit entry tagged
  with `bundle_id` and `bundle_step`. Per-branch entries from a
  matrix step also carry `bundle_branch` (`<key>=<value>`) and
  `bundle_branch_status` (`ok` | `failed` | `skipped`). Retrieve
  with `inspect audit grep <bundle-id>` or visualize with
  `inspect bundle status <bundle-id>` (see below).

### 14.1 Per-branch outcome reports: `inspect bundle status` (L6, v0.1.3)

After a bundle apply, `inspect bundle status <bundle-id>` reads the
audit log and renders a human-friendly per-step + per-branch table:

```text
SUMMARY: bundle status: id=01HXR9...  2 step(s), 6 audit entries
DATA:
  step `tar-volumes` (matrix):
    ✓ volume=atlas_milvus  (12300ms)
    ✓ volume=atlas_etcd     (4100ms)
    ✗ volume=aware_milvus   (1500ms)
  step `restart`: ✓ exec arte/atlas (840ms)
NEXT:
  inspect audit show <id>          # zoom into a specific entry
  inspect audit grep '<bundle-id>' # match every entry tagged with this bundle
```

Markers: ✓ ok forward, ✗ failed, · skipped (peer branch failed
first), ↶ rollback ok. `--json` returns
`{bundle_id, entries_total, steps[{step, kind, branches[{branch,
status, audit_id, verb, exit, duration_ms, is_revert}]}]}` for
agent consumption.

The bundle id is matched by prefix; ambiguous prefixes exit non-zero
with the full match list, and unknown prefixes exit non-zero with a
chained hint. Long-running operators can pipe a recent
`inspect audit ls` line straight into `inspect bundle status` to
inspect the full transaction shape.

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

### 15.3 Password authentication and `inspect ssh add-key` (L4, v0.1.3)

Some legacy or locked-down hosts only accept SSH password
authentication. v0.1.3 promotes this from "shell out to plain ssh
and lose the audit trail" to a first-class onboarding path with
its own audited migration helper.

**Configure the namespace.** In `~/.inspect/servers.toml`:

```toml
[namespaces.legacy-box]
host = "legacy.internal"
user = "admin"
auth = "password"                # default is "key"; opt in here
password_env = "LEGACY_BOX_PASS" # optional; falls back to interactive prompt
session_ttl  = "12h"             # optional; default 12h for password auth, capped at 24h
```

`password_env` only applies when `auth = "password"`; configuring
it under key auth is rejected at config-load time. `session_ttl`
parses durations like `"30m"`, `"4h"`, `"12h"`, `"24h"`, `"3600s"`;
anything above 24h is rejected so a forgotten session does not
stay live indefinitely. Key auth is unchanged — only password
mode picks up the longer default and the 24h cap.

**Connect.** With `LEGACY_BOX_PASS` exported, `inspect connect
legacy-box` consumes it once and opens a 12h `ControlPersist`
master. Without the env var (or with `inspect connect --interactive`
to bypass it), `inspect` prompts on the controlling tty up to three
times before aborting with a chained `see: inspect help ssh` hint.
Every `inspect <verb> legacy-box/...` for the rest of the TTL
window rides the same master without re-prompting.

`inspect` emits a one-time warning on the first password connect
per namespace:

```
warning: password auth is less secure than key-based.
Run 'inspect ssh add-key legacy-box' to migrate.
```

The marker `~/.inspect/.password_warned/<ns>` is touched after the
warning fires so subsequent connects stay quiet. `inspect ssh
add-key --apply` clears the marker when it flips a namespace off
password auth, so re-onboarding the same namespace later re-warns.

**Implementation note.** The password branch forces
`PubkeyAuthentication=no`, `PreferredAuthentications=password`,
and `NumberOfPasswordPrompts=1` at the ssh layer, so an
agent-loaded key cannot pre-empt the operator's intent to
authenticate by password and the per-call retry loop in
`inspect connect` controls the total attempt count.

**Migrate to keys.** Once the password session is open, run:

```sh
$ inspect ssh add-key legacy-box                 # dry-run preview
DRY-RUN: inspect ssh add-key legacy-box would: would generate
  ed25519 keypair at /home/op/.ssh/inspect_legacy-box_ed25519 and
  install /home/op/.ssh/inspect_legacy-box_ed25519.pub; would prompt
  to rewrite servers.toml: auth="key", drop password_env/session_ttl
hint: re-run with --apply to perform.

$ inspect ssh add-key legacy-box --apply         # do it
Flip namespace 'legacy-box' to auth="key" with key_path="..."
and drop password_env/session_ttl? [y/N] y
note: servers.toml updated — auth="key", key_path="..."
SUMMARY: ssh.add-key on 'legacy-box' — installed=true generated=true config_rewritten=true
DATA:
  key_path:   /home/op/.ssh/inspect_legacy-box_ed25519
  pubkey:     /home/op/.ssh/inspect_legacy-box_ed25519.pub
  audit_id:   1746...
NEXT:    inspect connect legacy-box (now key auth)
```

The verb generates an ed25519 keypair (or accepts an existing one
via `--key <path>`), installs the public half on the remote
`~/.ssh/authorized_keys` over the open ssh master (idempotent —
running twice does not duplicate the line), normalizes remote
permissions, verifies the install by re-reading the file, and
optionally rewrites the namespace's `servers.toml` entry to flip
to key auth. Key flags:

| Flag | Effect |
|---|---|
| `--apply` | required to perform the install + audit-log entry |
| `--key <path>` | reuse an existing private key (refuses if `<path>.pub` is missing) |
| `--no-rewrite-config` | install only; skip the auth-flip prompt |
| `--reason <text>` | attached to the audit entry (≤240 chars) |

The verb refuses `--apply` when no live ssh session is open — a
fresh password prompt would defeat the "enter password once"
value of the migration. The error points at `inspect connect <ns>`.
On non-tty stdin, the auth-flip auto-declines (no config writes
without explicit confirmation).

**Audit shape.** Every `--apply` run produces one entry:

```json
{"verb":"ssh.add-key","selector":"legacy-box",
 "args":"[key_path=/home/op/.ssh/inspect_legacy-box_ed25519] \
[generated=true] [installed=true] [config_rewritten=true]",
 "exit":0,"applied":true,
 "revert":{"kind":"command_pair","preview":"inspect ssh add-key legacy-box --apply",
           "command":"ssh legacy-box -- 'sed -i \"\\|<pubkey-line>|d\" ~/.ssh/authorized_keys'"}}
```

`revert.kind=command_pair` documents the manual `authorized_keys`
remove. The verb does not attempt to revoke the public key
automatically — that requires further operator intent (and is
exactly the sort of "automatic key revocation as a side effect"
that operators want to be explicit about).

**Session state in `inspect connections`.** The session-state verb
gains three columns to surface the L4 contract at a glance:

```
$ inspect connections
SUMMARY: 2 connection(s)
DATA:
  NAMESPACE             HOST                              STATUS    AUTH      TTL    EXPIRES_IN SOCKET
  arte                  deploy@arte.example.invalid:22    alive     key       30m    14m02s     /home/op/.inspect/sockets/arte.sock
  legacy-box            admin@legacy.internal:22          alive     password  12h    11h47m     /home/op/.inspect/sockets/legacy-box.sock
```

`expires_in` is an upper bound: ControlPersist resets on every
traffic, so the real lifetime is at least this long. `--json`
output gains matching `auth` / `session_ttl` / `expires_in` keys
on every connection record.

### 15.4 Credential lifetime: ssh-agent vs OS keychain vs env var (L2, v0.1.3)

inspect supports three different lifetimes for SSH credentials.
Pick the one that matches how long you want the secret to
survive. The default is recommended for almost everyone; the
keychain is the explicit opt-in for operators who want
cross-reboot persistence without leaving secrets in env vars or
shell history.

**Option 1 — default (ssh-agent + ControlMaster, one prompt per
shell session).** The first `inspect connect <ns>` prompts (or
reads `key_passphrase_env` / `password_env` if configured).
Subsequent verbs ride the master socket without re-prompting
until the configured TTL expires or you run
`inspect disconnect <ns>`. Logout / reboot clears the agent; the
next session prompts once again. **Most operators want this.**

**Option 2 — `--save-passphrase` (OS keychain, persists across
sessions and reboots).** L2 (v0.1.3). Opt in with:

```sh
$ inspect connect arte --save-passphrase           # key auth
$ inspect connect legacy-box --save-password       # password auth (alias)
```

`inspect` prompts once, opens the master, and saves the
credential to the OS keychain under service `inspect-cli`,
account `<ns>`. Subsequent `inspect connect <ns>` invocations in
fresh shell sessions auto-retrieve from the keychain — but only
for namespaces previously saved (no implicit cross-namespace
lookup). Manage stored entries with:

```sh
$ inspect keychain list                # which namespaces are saved
$ inspect keychain remove legacy-box   # delete one entry (audited)
$ inspect keychain test                # probe backend reachability
```

Backends:

| Platform | Store | Notes |
|---|---|---|
| macOS | Keychain Services | accessed via `security` framework |
| Windows | Credential Manager | also reachable from WSL2 |
| Linux | Secret Service (DBus) | covers GNOME Keyring + KDE Wallet |

When the OS keychain backend is unavailable (no keyring daemon,
no session bus, container without a desktop):

- `inspect connect --save-passphrase` warns once on stderr and
  falls back to per-session prompt. The master still comes up.
- Auto-retrieval during normal connects silently treats backend
  errors as "not stored" (no stderr line per call).
- `inspect keychain test` exits non-zero with a chained hint
  pointing at which dependency is missing.

**Option 3 — env var (CI / scripted use).** Configure the
namespace's `key_passphrase_env` (key auth) or `password_env`
(password auth) field in `~/.inspect/servers.toml`:

```toml
[namespaces.legacy-box]
host = "legacy.internal"
user = "admin"
auth = "password"
password_env = "LEGACY_BOX_PASS"
```

Export the variable in your shell / CI environment; `inspect
connect` consumes it once at master start. The value is never
copied to inspect's own files.

**Resolution order** (per namespace, per auth mode):

| Step | Key auth | Password auth (L4) |
|---|---|---|
| 1 | live socket reuse | live socket reuse |
| 2 | user `~/.ssh/config` mux | user mux |
| 3 | ssh-agent identity | (skip — `PubkeyAuthentication=no`) |
| 4 | `key_passphrase_env` | `password_env` |
| 5 | **OS keychain (L2)** | **OS keychain (L2)** |
| 6 | interactive prompt | interactive prompt (3 attempts) |

**Index file.** `~/.inspect/keychain-index` (mode 0600) lists the
namespaces with stored entries. Holds **no** secret material —
only namespace names. Self-healing: `inspect keychain list`
prunes any index entry the backend no longer recognizes (e.g.,
operator deleted via `Keychain Access.app` directly).

**Audit shape.** `inspect keychain remove` writes one entry:

```json
{"verb":"keychain.remove","selector":"<ns>",
 "args":"[was_present=true|false]","exit":0,
 "revert":{"kind":"unsupported",
   "reason":"keychain.remove has no inverse — re-save with
             'inspect connect <ns> --save-passphrase'"}}
```

`save` is implicit in `inspect connect --save-passphrase` and is
audited by the connect entry; the keychain module emits no
separate row.

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
