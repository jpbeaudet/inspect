# Inspect — Operational Debugging CLI Bible (v6.2 draft)

**Author:** Co-CTO (JP)
**Status:** Design, iterating
**Implementation language:** Rust
**Target distribution:** Single static binary per platform, installed via `curl` or `cargo install inspect-cli` (binary name `inspect`)
**Last updated:** 2026-04-25
**Supersedes:** v5, v6
**Changes from v6:** Source aliases reinstated (§6.8). Write verbs added (§8) with strict safety contract: dry-run default, `--apply` to execute, `--diff` for edits, audit log always written. The tool now does **search AND transform**.

> **Scope.** A namespaced CLI tool for cross-server operational debugging *and* hot-fix application. Connects via SSH to one or more registered servers (each addressed by a short namespace name), auto-discovers what's running, and exposes that knowledge through three tiers: **direct verbs** for the common case (familiar to anyone who knows `stern`, `grep`, `kubectl`, `sed`, `scp`); a **LogQL search DSL** for cross-medium and pipelined queries (familiar to anyone who knows Grafana/Loki); and **`--json` + shell pipes** for the long tail. Read verbs are immediate. Write verbs follow a strict dry-run-by-default + `--apply` + audit-log contract.
>
> **Core constraints.**
> 1. Adding a server takes < 30 seconds.
> 2. SSH credentials are secure by default and identical across laptops, CI, and headless environments.
> 3. After first contact with a server, the tool *learns* it — no hand-written profile required.
> 4. Persistent SSH sessions per namespace mean passphrases are entered once per terminal session.
> 5. **Mutations are explicit.** Every write verb runs as a dry-run unless `--apply` is given. Every applied mutation is recorded in an audit log.
> 6. **Single static binary.** No language runtime required on either local or remote side.
> 7. **Sub-50ms cold start.** Must feel like a builtin, not an app.
> 8. **Conventional surface, not invented.** Verb commands borrow stern/rg/kubectl/sed/scp flags wholesale. The search DSL is LogQL — not "LogQL-inspired," LogQL — with the only extensions being reserved label names and a Splunk SPL `map` stage for cross-medium chaining.
> 9. **Three tiers, opt-in escalation.** Most usage never touches the DSL.
>
> **Out of scope (v1).** Web UI / TUI. Kubernetes-native modes. Persistent monitoring / alerting. Password auth. Remote agents. Distributed tracing. Per-user policy enforcement (v1 inherits host access controls).

---

## 1. Why This Tool Exists

Debugging a multi-service deployment today looks like this:

```
ssh myserver
docker ps
docker logs service-a 2>&1 | grep -i error | tail -20
exit
ssh myserver  # forgot to check service-b
docker logs service-b 2>&1 | grep -i error | tail -20
ssh myserver
sudo sed -i 's/old-endpoint/new-endpoint/' /etc/atlas.conf
docker restart atlas
exit
```

`inspect` collapses this. Find the issue, apply the fix, verify — same tool, same session.

```
# Tier 1 — read (find the issue)
inspect grep "error" arte/pulse,atlas --since 30m
inspect why arte/atlas

# Tier 1 — write (fix it, dry-run first)
inspect edit arte/atlas:/etc/atlas.conf 's|old-endpoint|new-endpoint|'   # shows diff
inspect edit arte/atlas:/etc/atlas.conf 's|old-endpoint|new-endpoint|' --apply
inspect restart arte/atlas --apply

# Verify
inspect logs arte/atlas --since 30s --follow

# Tier 2 — LogQL for cross-medium
inspect search '
  {server="arte", service="atlas", source="logs"} or
  {server="arte", service="atlas", source="file:/etc/atlas.conf"}
  |= "milvus"
' --since 30m

# Tier 3 — JSON + shell composition
inspect search '{source="logs"} |= "OOM"' --since 5m --json \
  | jq -r '.service' | sort -u \
  | xargs -I{} inspect restart arte/{} --apply
```

---

## 2. Design Principles

1. **Namespace-first.** Every command targets one or more namespaces.
2. **Auto-discovery is the default.** First contact scans and produces a profile.
3. **One selector grammar everywhere.** `<server>/<service>[:<path>]` works in every verb and inside LogQL labels. Borrowed from kubectl.
4. **Three tiers, sharply separated.** Tier 1 = verbs (flags only). Tier 2 = LogQL (one quoted string). Tier 3 = `--json` + shell.
5. **The search DSL is LogQL.** Not "LogQL-inspired." LogQL. Two extensions: reserved label names (`server`, `service`, `source`) and a `map` stage (Splunk SPL convention).
6. **Always-quoted DSL.** `inspect search '...'`. The `|` inside is the DSL's. The shell `|` is outside. They never meet.
7. **`--json` is universal.** Every command produces line-delimited JSON with `--json`. Schema is versioned (`schema_version` field).
8. **Structured output: SUMMARY / DATA / NEXT.** Every command, both human and JSON modes.
9. **Persistent SSH per namespace.** Passphrase entered once per terminal session.
10. **Mutations are safe-by-default, not absent.** Write verbs preview by default (dry-run); execute only with `--apply`. Every applied mutation goes to an audit log. Content edits show a diff before applying. Destructive ops confirm interactively. The tool lets you mutate anything your SSH credentials allow; it just makes you mean it.
11. **Server-agnostic and language-agnostic.** Talks to Docker and shell.
12. **Aliases for fluency, not for vocabulary.** `i` = `inspect`. `@name` for saved selectors. No parallel natural-language vocabulary.
13. **Fail loud, fail informative.** Exact error + what would fix it.
14. **No invented syntax where industry has a convention.** Every flag, verb, operator, and stage has a precedent.

---

## 3. Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                         CLI INVOCATION                          │
└─────────────────────────────────────────────────────────────────┘
    inspect <verb> <selector> [flags]
    inspect search '<LogQL>' [--since dur] [--tail N] [--follow]
    inspect fleet <verb> [--ns pattern]
              ↓
    ┌─────────────────────────────────┐
    │  Namespace resolver             │
    │  - INSPECT_<NS>_* env vars      │
    │  - ~/.inspect/servers.toml      │
    └─────────────────────────────────┘
              ↓
    ┌─────────────────────────────────┐
    │  Persistent SSH manager         │
    │  - openssh crate (native-mux)   │
    │  - one ControlMaster per ns     │
    │  - prompts passphrase ONCE      │
    └─────────────────────────────────┘
              ↓
    ┌─────────────────────────────────┐
    │  Profile loader / discovery     │
    │  - cached profile per ns        │
    │  - drift check async            │
    └─────────────────────────────────┘
              ↓
    ┌─────────────────────────────────┐
    │  Alias + Selector resolver      │
    │  - @name → literal expansion    │
    │  - <server>/<service>[:<path>]  │
    │  - globs, regex, groups, ~sub   │
    └─────────────────────────────────┘
              ↓
    ┌──────────────────────┬────────────────────────┐
    │  Read verb dispatcher│  Write verb dispatcher │
    │  logs/grep/cat/ls/   │  restart/stop/start/   │
    │  find/ps/status/why  │  reload/cp/edit/rm/    │
    │                      │  exec/chmod/chown/...  │
    │                      │  ↓                     │
    │                      │  Safety gate:          │
    │                      │  dry-run → --apply     │
    │                      │  --diff → audit log    │
    └──────────────────────┴────────────────────────┘
              ↓ (only for `search`)
    ┌─────────────────────────────────┐
    │  LogQL parser (chumsky)         │
    │  log queries / metric queries   │
    │  map stage (cross-medium)       │
    └─────────────────────────────────┘
              ↓
    ┌─────────────────────────────────┐
    │  Query planner + executor       │
    │  filter pushdown, parallel      │
    │  fanout, streaming/aggregating  │
    └─────────────────────────────────┘
              ↓
    ┌─────────────────────────────────┐
    │  Output renderer                │
    │  SUMMARY/DATA/NEXT              │
    │  human or --json                │
    └─────────────────────────────────┘
```

Single Rust process per invocation. `openssh` crate maintains ControlMaster lifetimes via OpenSSH's native multiplexing protocol.

---

## 4. Namespace & Credential Management

### 4.1 Namespace concept

A **namespace** = a short, user-chosen alias for a server (`arte`, `prod`, `staging`). Local to the user's environment.

### 4.2 Storage layers (priority order)

1. **Per-namespace env vars** (`INSPECT_<NS>_HOST`, `_USER`, `_KEY_PATH`, `_KEY_PASSPHRASE_ENV`, `_PORT`) — primary path
2. **`~/.inspect/servers.toml`** — for daily developer use (mode 600)
3. **Interactive `inspect add`** — for first-time setup

Env wins over file when both are set. Inline key for CI: `INSPECT_<NS>_KEY_INLINE` (base64, mutually exclusive with `KEY_PATH`).

### 4.3 Persistent SSH sessions (via OpenSSH ControlMaster)

Each namespace gets one persistent multiplexer via the **`openssh` crate with `native-mux` feature**: real ControlMaster (20+ years of security review), native protocol access from Rust (structured exit codes, separate stdout/stderr, async), zero re-implementation (host-key verification, `known_hosts`, ssh-agent, ProxyJump all delegated to OpenSSH). Existing `~/.ssh/config` ControlMaster setups are reused.

Control socket at `~/.inspect/sockets/<ns>.sock` (mode 600).

**Resolution order at connect time:** inspect-managed socket → user's existing ControlMaster → ssh-agent → passphrase from env var → interactive prompt (`rpassword`). After successful auth, multiplexer starts automatically.

**Configuration:** `persist = true` (default), `persist_ttl = "4h"` (Codespace default; `"30m"` elsewhere).

**Lifecycle commands:** `inspect connect <ns>` / `disconnect <ns>` / `connections` / `disconnect-all`.

### 4.4 What the tool will NOT do

Never store passphrases on disk. Never accept private keys inline on disk (env only). Never accept password auth. Never auto-trust unknown host keys. Never silently fail on missing credentials. Never apply a mutation without `--apply`. Never modify `.bashrc` or shell rc files.

### 4.5 Server management commands

`inspect list` / `add <ns>` / `remove <ns>` / `test <ns>` / `show <ns>` (passphrases redacted).

---

## 5. Auto-Discovery & Server Profile

### 5.1 Discovery

`inspect setup <ns>` scans the connected host:

| Source | What it learns |
|---|---|
| `docker ps/inspect` | Containers, mounts, env, network, ports, log driver, labels |
| `docker volume/network/images` | Volumes, networks, images |
| `ss -tlnp` / `netstat -tlnp` | Host-level listening ports |
| `systemctl list-units` | Non-Docker services |
| Health endpoint heuristics | `/health`, `/healthz`, `/ping`, `/status` |
| Log location heuristics | Log driver inspection; fallback to `docker logs` |
| Remote tooling probe | Detects `rg`, `jq`, `journalctl`, `sed` on remote |

Each source is best-effort. Missing permissions degrade with explicit warnings. **Async drift check on every command** (never blocks). Full re-discovery on explicit `setup` or cache TTL expiry (7d default). Local edits preserved across re-discovery.

### 5.2 Profile schema

```yaml
schema_version: 1
namespace: arte
host: arte.luminary.internal
discovered_at: 2026-04-25T14:32:18Z
remote_tooling: { rg: true, jq: true, journalctl: true, sed: true }

services:
  - name: pulse
    container_id: 8a3f...
    image: luminary/pulse:1.4.2
    ports: [{host: 8000, container: 8000, proto: tcp}]
    health: http://localhost:8000/health
    health_status: ok
    log_driver: json-file
    log_readable_directly: true
    mounts: [{source: /opt/luminary/pulse/config, target: /etc/pulse, type: bind}]
  - name: atlas
    depends_on: [milvus, postgres, minio]

volumes: [...]
images: [...]
networks: [...]
groups:
  storage: [postgres, milvus, redis, minio]
  knowledge: [pulse, atlas, synapse, nexus-be]
```

### 5.3 Permission degradation matrix

| Capability | Best path | Fallback | Effect |
|---|---|---|---|
| Container inventory | `docker ps` | — | Required |
| Log reading | direct file read | `docker logs` | Slower for time-range |
| Filter pushdown | remote `rg` | remote `grep` | Slower |
| Host port scan | `ss -tlnp` (root) | container-only | Misses non-Docker |
| File edits | remote `sed -i` | local read-modify-write | Slower |

---

## 6. Selectors — Universal Addressing

### 6.1 Grammar

```
<selector> ::= <server-spec> [ "/" <service-spec> ] [ ":" <path-spec> ]
            |  "@" <alias-name>
```

### 6.2 Server-spec

`arte` | `arte,prod` | `'prod-*'` | `all` | `'~staging'`

### 6.3 Service-spec

`pulse` | `pulse,atlas` | `'milvus-*'` | `'/milvus-\d+/'` (regex, stern-style) | `storage` (group) | `'*'` | `'~synapse'` | `_` (host-level)

### 6.4 Path-spec

`arte/atlas:/etc/atlas.conf` | `arte/atlas:/etc/` | `arte/atlas:/var/log/*.log` | `arte/_:/var/log/syslog`

### 6.5 Resolution order for service names

1. Container short name → 2. Profile aliases → 3. Profile groups → 4. Globs/regex → 5. Subtractive. Name collisions emit a warning; container short name wins.

Empty resolution → friendly error listing available servers, services, groups, aliases. Never silent.

### 6.6 Examples

```
arte                          all services on arte
arte/pulse                    one service
arte/pulse,atlas              two services
'prod-*/storage'              storage group, all prod servers
arte/atlas:/etc/atlas.conf    one file inside atlas
arte/_:/var/log/syslog        host-level file
@plogs                        alias expansion
```

### 6.7 Aliases — short names for selectors

Long selectors get tedious. Aliases let users define short `@name` handles, usable anywhere a selector is expected.

#### Saved aliases

```
$ inspect alias add plogs '{server="arte", service="pulse", source="logs"}'
$ inspect alias add prod-logs '{server=~"prod-.*", source="logs"}'
$ inspect alias add storage-prod 'prod-*/storage'
$ inspect alias add atlas-conf '{server="arte", service="atlas", source="file:/etc/atlas.conf"}'

$ inspect alias list
@plogs           {server="arte", service="pulse", source="logs"}
@prod-logs       {server=~"prod-.*", source="logs"}
@storage-prod    prod-*/storage
@atlas-conf      {server="arte", service="atlas", source="file:/etc/atlas.conf"}

$ inspect alias remove plogs
```

Stored in `~/.inspect/aliases.toml` (mode 600).

#### Use anywhere

```bash
# In verb commands (verb-style aliases)
inspect logs @storage-prod --since 1h
inspect grep "error" @storage-prod
inspect restart @storage-prod --apply

# In LogQL queries (LogQL-style aliases)
inspect search '@plogs |= "error"'
inspect search '@atlas-conf or @prod-logs |= "milvus"'
```

`@` marks an alias; without it, the literal selector is parsed.

#### Type compatibility

Verb-style aliases (`prod-*/storage`) work in verb commands. LogQL-style aliases (`{server=...}`) work in `inspect search`. Misuse produces a clear error with the fix:

```
error: alias '@atlas-conf' is a LogQL selector, not a verb selector.
       For verb commands, run: inspect alias add atlas-v 'arte/atlas'
```

#### One-off use via shell variables

```bash
SEL='{server="arte", service="atlas", source="file:/etc/atlas.conf"}'
inspect search "$SEL |= \"milvus\""
```

#### v1 limits

No parameterization. No chaining (aliases can't reference other aliases). No inline `let`-binding in queries. (All relaxed in v2 if there's demand.)

---

## 7. Tier 1 — Read Verbs

No DSL. No mutations. Flags only.

### 7.1 Verb catalog

| Verb | Purpose | Convention source |
|---|---|---|
| `logs <sel>` | tail / view container logs | stern, kubectl, docker |
| `grep <pat> <sel>` | search content in logs or files | grep, rg |
| `cat <sel>:<path>` | read a file | unix cat |
| `ls <sel>:<path>` | list directory contents | unix ls |
| `find <sel>:<path> [pat]` | find files matching a pattern | unix find, fd |
| `ps <sel>` | list running containers | unix ps, docker ps |
| `status <sel>` | service inventory + health rollup | systemctl, kubectl |
| `health <sel>` | detailed health check per service | k8s probes |
| `volumes/images/network/ports <sel>` | list respective resources | docker |
| `why <sel>` | diagnostic walk (dependency graph) | (novel — see §12) |
| `recipe <n> [<sel>]` | multi-step diagnostic recipe | (novel — see §12) |
| `connectivity <sel>` | connectivity matrix | (novel — see §12) |
| `setup/discover <ns>` | run full discovery | (tool-specific) |

### 7.2 Flags — direct grep/rg/stern parity

`--since <dur>` `--until <dur>` `--tail <n>` `--follow`/`-f` (stern, kubectl, journalctl)
`-i` `-s` `-w` `-F` `-E` `-A N` `-B N` `-C N` `-v` `-m N` `-c` (grep, rg)
`--json` `--no-color` (rg, universal)

**Smart-case default** (rg): all-lowercase pattern → case-insensitive; any uppercase → case-sensitive. `-i`/`-s` override.

### 7.3 Examples

```bash
inspect logs arte/pulse --since 1h --tail 200 --follow
inspect grep "error" arte --since 1h
inspect grep "error" arte/pulse,atlas --since 1h -C 3 --tail 50
inspect grep "timeout" 'prod-*/storage' --since 1h
inspect status 'prod-*'
inspect cat arte/atlas:/etc/atlas.conf
inspect why arte/synapse
```

---

## 8. Tier 1 — Write Verbs (search AND transform)

Write verbs mutate remote servers. Strict safety contract: dry-run by default, `--apply` to execute, audit log always written, `--diff` for content edits.

### 8.1 Verb catalog

| Verb | Purpose | Convention source |
|---|---|---|
| `restart <sel>` | restart container(s) | docker restart |
| `stop <sel>` | stop container(s) | docker stop |
| `start <sel>` | start container(s) | docker start |
| `reload <sel>` | send SIGHUP / reload | systemctl reload |
| `cp <local> <sel>:<path>` | push file local→remote | scp, kubectl cp |
| `cp <sel>:<path> <local>` | pull file remote→local | scp, kubectl cp |
| `edit <sel>:<path> '<sed>'` | sed-style content edit (atomic) | sed -i |
| `rm <sel>:<path>` | delete file (with confirm) | rm |
| `mkdir/touch <sel>:<path>` | create dir/file | mkdir, touch |
| `chmod <sel>:<path> <mode>` | change permissions | chmod |
| `chown <sel>:<path> <owner>` | change ownership | chown |
| `exec <sel> -- <cmd>` | run arbitrary command | docker exec |

### 8.2 The safety contract

**1. Dry-run by default.** Without `--apply`, shows what would happen and exits zero.

```
$ inspect restart arte/pulse
DRY RUN. Would restart 1 service:
  arte/pulse  (currently running, uptime 4h 12m)
Re-run with --apply to execute.
```

**2. `--diff` for content edits.** `edit` and `cp` to existing paths show a unified diff. The diff is exactly what gets applied.

```
$ inspect edit arte/atlas:/etc/atlas.conf 's/timeout=30/timeout=60/'
DRY RUN. Would edit 1 file:

--- arte/atlas:/etc/atlas.conf
+++ arte/atlas:/etc/atlas.conf (proposed)
@@ -42,7 +42,7 @@
-timeout = 30
+timeout = 60

Re-run with --apply to execute.
```

**3. Audit log + snapshots.** Every `--apply` appends to `~/.inspect/audit/<YYYY-MM>-<user>.jsonl` (mode 600) and saves the original content to `~/.inspect/audit/snapshots/<hash>` before mutating:

```json
{
  "schema_version": 1, "ts": "2026-04-25T14:32:18Z",
  "user": "jp", "host": "laptop-jp",
  "verb": "edit", "selector": "arte/atlas:/etc/atlas.conf",
  "args": "s/timeout=30/timeout=60/",
  "diff_summary": "1 file, +1 -1",
  "previous_hash": "sha256:9c1f...", "new_hash": "sha256:4b88...",
  "snapshot": "~/.inspect/audit/snapshots/sha256-9c1f...",
  "exit": 0, "duration_ms": 412
}
```

The snapshot is the original file content, stored locally, keyed by its hash so identical files are deduplicated. Config files are kilobytes; the disk cost is trivial; the confidence boost is not.

`inspect audit ls` / `inspect audit show <id>` / `inspect audit grep <pattern>`.

**`inspect revert <audit-id>`.** Restores the file at the recorded selector to the snapshot content. Follows the same safety contract: dry-run by default (shows a reverse diff), `--apply` to execute, audit-logged as a revert. If the current remote content doesn't match the `new_hash` (someone else changed it since), the revert warns and requires `--force`.

**4. Confirmation for irreversible ops.** `rm`, `chmod`, `chown` prompt interactively with `--apply`. Skip with `--yes`.

**5. Atomic writes.** `edit` writes temp file then renames. Partial failures leave original intact.

**6. Multi-target: best-effort across targets.** If one target fails, others still apply. Summary lists successes and failures. Exit 0 only if all succeeded.

**7. Large-fanout interlock.** Selectors matching >10 targets prompt interactively even with `--apply` (skip with `--yes-all`).

### 8.3 The `edit` verb

Workhorse for hot-fix config changes. Standard `sed` expression (`s/old/new/g`). Runs on remote via `sed -i` when applied; in dry-run, fetches a copy and produces the diff locally.

Multi-target edits apply the same expression to every target:

```bash
inspect edit '*/atlas:/etc/atlas.conf' 's|http://old-host|http://new-host|g'
inspect edit '*/atlas:/etc/atlas.conf' 's|http://old-host|http://new-host|g' --apply
```

For complex edits beyond `sed`, pull-edit-push:

```bash
inspect cp arte/atlas:/etc/atlas.conf ./atlas.conf.local
$EDITOR ./atlas.conf.local
inspect cp ./atlas.conf.local arte/atlas:/etc/atlas.conf --diff    # preview
inspect cp ./atlas.conf.local arte/atlas:/etc/atlas.conf --apply   # push
```

### 8.4 Trust model

v1: **if your SSH credentials let you do it, `inspect` lets you do it.** No privilege escalation. No per-user policies inside `inspect`. The tool is a safer, auditable interface to operations the user could already perform via raw SSH. Per-user policies (write-restricted tokens, approval flows for prod) deferred to v2.

### 8.5 Search-then-transform composition (Tier 3)

```bash
# Find OOM'd services, restart them
inspect search '{source="logs"} |= "OOM"' --since 5m --json \
  | jq -r '.service' | sort -u \
  | xargs -I{} inspect restart arte/{} --apply

# Find old-endpoint in configs, preview replacement
inspect grep "old-endpoint" 'prod-*' --json \
  | jq -r '"\(.server)/\(.service):\(.path)"' | sort -u \
  | xargs -I{} inspect edit {} 's|old-endpoint|new-endpoint|g'
```

### 8.6 Examples

```bash
inspect restart arte/pulse                                    # dry-run
inspect restart arte/pulse --apply                            # execute
inspect reload '*/gateway' --apply                            # fleet reload
inspect cp ./fixed.conf 'prod-*/pulse:/etc/pulse.conf' --diff # preview push
inspect cp ./fixed.conf 'prod-*/pulse:/etc/pulse.conf' --apply
inspect cp arte/atlas:/var/log/atlas.log ./atlas.log          # pull (no --apply needed)
inspect edit '*/atlas:/etc/atlas.conf' 's|timeout=30|timeout=60|' --apply
inspect exec arte/postgres -- "psql -c 'VACUUM ANALYZE;'" --apply
```

---

## 9. Tier 2: The Search DSL (LogQL)

When the user needs multiple sources of different mediums in one query, a pipeline of transformations, aggregations, or cross-medium chaining — they reach for `inspect search '...'`. The query is **always a single quoted string**.

### 9.1 Why LogQL

LogQL is Grafana Loki's query language. Industry-standard for log search, designed for filter pushdown, composable (selectors + stages + metric queries), has a published spec. Adopting it contributes zero new vocabulary. LLM agents trained on public LogQL corpus generate correct queries first try.

### 9.2 Reserved labels

| Label | Meaning | Examples |
|---|---|---|
| `server` | namespace | `"arte"`, `"prod-eu"` |
| `service` | container/service tag (or `_` for host-level) | `"pulse"`, `"storage"` |
| `source` | medium + locator | `"logs"`, `"file:/etc/atlas.conf"`, `"dir:/etc"`, `"discovery"`, `"state"`, `"volume:milvus-data"`, `"image"`, `"network"`, `"host:/var/log/syslog"` |

### 9.3 Selectors

```
{server="arte", service="pulse", source="logs"}
{server=~"prod-.*", service="storage", source="logs"}
{server="arte", service!~"canary-.*", source="logs"}
```

Operators: `=` exact, `!=` not exact, `=~` regex, `!~` not regex. Standard LogQL. Aliases (`@name`) substitute before parsing.

### 9.4 Multi-source: `or` between selectors

```
{server="arte", service="atlas", source="logs"} or
{server="arte", service="atlas", source="file:/etc/atlas.conf"}
|= "milvus"
```

Each selector independently filter-pushed (logs → remote `docker logs | rg`; file → remote `cat | rg`). Results merge into one pipeline.

### 9.5 Filter operators

`|= "literal"` contains · `!= "literal"` doesn't contain · `|~ "regex"` regex match · `!~ "regex"` regex doesn't match

### 9.6 Pipeline stages (log queries, streaming)

| Stage | Purpose | LogQL? |
|---|---|---|
| `\| json` | parse line as JSON | yes |
| `\| logfmt` | parse key=value | yes |
| `\| pattern "<...>"` | positional extraction | yes |
| `\| regexp "<...>"` | named regex groups | yes |
| `\| line_format "<tpl>"` | reformat output `{{.label}}` | yes |
| `\| label_format new=expr` | add/rename labels | yes |
| `\| <field> <op> <value>` | filter on parsed field | yes |
| `\| drop label1, label2` | remove labels | yes |
| `\| keep label1, label2` | retain only listed | yes |
| `\| map { <sub-query> }` | cross-medium chaining | **extension** (Splunk SPL) |

Field comparison: `==`, `!=`, `>`, `>=`, `<`, `<=`, `=~`, `!~`. Boolean: `and`, `or`, `not`, parentheses.

### 9.7 Metric queries (aggregations)

Log queries stream records. Metric queries aggregate — they need the full window. **A query is either log or metric, never both.** This is LogQL's split.

```
count_over_time({server="arte", source="logs"} |= "error" [5m])
rate({server="arte", source="logs"} |= "error" [5m])
sum by (service) (count_over_time({server="arte", source="logs"} |= "error" [5m]))
topk(5, sum by (service) (rate({server="arte", source="logs"} |= "error" [1h])))
```

Aggregation functions: `sum`, `avg`, `min`, `max`, `stddev`, `stdvar`, `count`, `topk`, `bottomk`.
Range functions: `count_over_time`, `rate`, `bytes_over_time`, `bytes_rate`, `absent_over_time`.

### 9.8 Cross-medium chaining: `map` stage

Runs a sub-query per unique value of a label. Borrowed from Splunk SPL's `map` command with `$field$` interpolation:

```
{server="arte", source="logs"} |= "error" 
  | json
  | map { {server="arte", service="$service", source=~"file:.*"} |~ "$service" }
```

For each unique `service` value in the parent stream, runs the sub-query in parallel. The merged output is the stage's result. `map` is `inspect`'s only domain extension to LogQL.

**`$field$` is not a shell variable.** The `$field$` interpolation is consumed by `inspect` inside the DSL, not by the shell. The surrounding shell never sees it because the entire query is single-quoted (`'...'`). If you accidentally double-quote the query, the shell will try to expand `$service` as an environment variable and you'll get an empty string. Rule of thumb: `inspect search` queries are always single-quoted.

### 9.9 Streaming vs aggregating

Log queries stream by default. Metric queries always aggregate. `--follow` works with log queries. `map` streams the parent, then streams sub-queries in parallel (output ordering not stable).

### 9.10 Filter pushdown

`|=`/`|~` after logs selector → remote `rg` or `grep`. `--since`/`--until` → `docker logs --since` or `journalctl --since`. Service filtering → container selection. `--tail N` → early termination. If `rg` isn't available remotely, falls back to `grep` with a one-time hint.

### 9.11 Selector flags

`--since <dur>` `--until <dur>` `--tail <n>` `--follow`/`-f` `--json` `--no-color`

No `--server`, `--service`, `--source` flags. Those go in the selector.

### 9.12 Examples

```bash
inspect search '{server=~"prod-.*", service="storage", source="logs"} |= "error"' --since 1h
inspect search '{server="arte", source="logs"} | json | status >= 500' --since 1h
inspect search 'sum by (service) (count_over_time({server="arte", source="logs"} |= "error" [5m]))'
inspect search 'topk(5, sum by (service) (rate({server="arte", source="logs"} |= "error" [1h])))'
inspect search '{server="arte", source="logs"} |= "milvus" | json | map { {server="arte", service="$service", source=~"file:.*"} |~ "milvus" }' --since 30m
inspect search '@plogs or @atlas-conf |= "milvus"' --since 30m
inspect search '{server=~"prod-.*", source="logs"} |= "error"' --follow
```

### 9.13 Concrete grammar (BNF)

```
search           ::= "search" query selector_flag*
query            ::= log_query | metric_query

log_query        ::= selector_union (filter | stage)*
selector_union   ::= selector ("or" selector)*
selector         ::= "{" label_matcher ("," label_matcher)* "}" | "@" alias_name
label_matcher    ::= label_name match_op string_or_regex
match_op         ::= "=" | "!=" | "=~" | "!~"

filter           ::= "|=" string | "!=" string | "|~" regex | "!~" regex
stage            ::= "|" stage_name stage_args?
map_stage        ::= "map" "{" log_query "}"

metric_query     ::= range_aggregation | vector_aggregation
range_aggregation::= range_fn "(" log_query "[" duration "]" ")"
vector_aggregation::= agg_fn ("by" "(" label_list ")")? "(" (range_agg | vector_agg) ")"

selector_flag    ::= "--since" duration | "--until" duration | "--tail" int 
                   | "--follow" | "-f" | "--json" | "--no-color"
duration         ::= int unit | rfc3339
unit             ::= "s" | "m" | "h" | "d" | "w"
```

---

## 10. Tier 3: JSON & Shell Composition

Every command supports `--json`. Line-delimited JSON, one record per line, stable schema.

### 10.1 Record schema

Every record carries: `schema_version`, `_source`, `_medium`, `server`, `service`, plus medium-specific fields (`timestamp`/`line` for logs, `path`/`content` for files, `container`/`status` for state, etc.). Schema versioned; breaking changes bump `schema_version`.

### 10.2 Examples

```bash
inspect logs arte --since 1h --json | jq -r '.service' | sort | uniq -c | sort -rn | head -10
inspect ps 'prod-*' --json | jq -r '"\(.server)/\(.service)"' | fzf
inspect search '{source="logs"} |= "OOM"' --since 5m --json \
  | jq -r '.service' | sort -u \
  | xargs -I{} inspect restart arte/{} --apply
```

---

## 11. Output Contract: SUMMARY / DATA / NEXT

Every command, both human and JSON modes, returns three layers:

```
SUMMARY:    one-sentence headline
DATA:       structured table or list
NEXT:       up to 3 suggested follow-up commands with rationale
```

JSON shape (stable, versioned):

```json
{
  "schema_version": 1,
  "summary": "8 services, 7 healthy, 1 down (neo4j).",
  "data": { "services": [...] },
  "next": [
    {"cmd": "inspect why arte/neo4j", "rationale": "diagnose the down service"},
    {"cmd": "inspect restart arte/neo4j", "rationale": "dry-run; add --apply to execute"}
  ],
  "meta": { "ns": "arte", "discovered_at": "...", "drift_warning": null }
}
```

Cross-source correlation: errors clustered in time, health cascading by dependency graph, volume/image drift, cross-medium signals. Each correlation has an explicit rule; if it can't be computed cheaply, it's omitted.

---

## 12. Other Commands

### 12.1 Recipes

Multi-step diagnostic and remediation flows in YAML. Mutating recipes require `mutating: true` and run as dry-run unless `--apply` is given to the recipe itself.

```yaml
name: deploy-check
steps: [status, health, "search '{source=\"logs\"} |= \"error\"' --since 5m", connectivity]
correlate: true
```

Default recipes shipped with v1: `deploy-check`, `disk-audit`, `network-audit`, `log-roundup`, `health-everything`.

### 12.2 `why` — Service diagnostic

Walks the dependency graph from the profile. Shows which dependencies are healthy, which are failing, and suggests the likely root cause.

### 12.3 `connectivity` — Connectivity matrix

Renders the connectivity matrix from the profile, optionally probing live.

---

## 13. Fleet (Multi-Server) Operations

`search` handles multi-server via wildcard selectors. `inspect fleet <verb>` does the same for other verbs.

```
inspect fleet status --ns 'prod-*'
inspect fleet restart pulse --ns 'prod-*' --apply
```

Named groups in `~/.inspect/groups.toml`. Credential heterogeneity handled per-namespace. If one namespace fails, fleet continues with the rest. Concurrency capped at `INSPECT_FLEET_CONCURRENCY` (default 8). Fleet write verbs obey the same safety contract; large-fanout interlock triggers on total target count.

---

## 14. Implementation Notes (Rust)

### 14.1 Stack

| Concern | Choice |
|---|---|
| CLI | `clap` (derive) |
| SSH | `openssh` crate, `native-mux` |
| Async | `tokio` |
| Output | `comfy-table` + `crossterm` + `indicatif` |
| Diff | `similar` (Myers diff) |
| Config | `toml` + `serde` |
| Profile | `serde_yaml` |
| LogQL parser | `chumsky` |
| Regex | `regex` (same as rg) |
| Time | `humantime` + `chrono` |
| Glob | `globset` (same as rg) |
| Passphrase | `rpassword` |
| Secrets | `zeroize` |
| Hashing | `sha2` |
| Errors | `thiserror` |

### 14.2 Security posture

Mode 600 on all config/socket/audit files. Passphrases never on disk, never logged, never in errors. Private keys only via env var. Never auto-trust unknown host keys. All write verbs default to dry-run. Every applied mutation in audit log. No telemetry, no phone-home, no LLM calls.

### 14.3 Performance targets

| Metric | Target |
|---|---|
| Cold start | < 50ms warm, < 100ms cold |
| `status` round-trip | < 200ms (cached profile, persistent SSH) |
| `search` across 5 servers | < 2s for first results, streaming after |
| Write-verb dry-run | < 500ms single target, < 2s for 10-target fanout |
| Drift check | async, never blocks foreground |

### 14.4 Distribution

GitHub Releases (x86_64/aarch64, linux/darwin), `cargo install inspect-cli`, Homebrew tap, one-line `curl` installer, Docker image. Single static binary, ~8-12MB via musl target.

---

## 15. The "How to Use" Reference

Ships with the binary at `inspect help`. Readable in 60 seconds.

```
INSPECT — QUICK REFERENCE

1. Add & connect:
     $ inspect add arte
     $ inspect connect arte              # one passphrase, whole session

2. Look at it:
     $ inspect status arte               # what's running, what's healthy
     $ inspect why arte/pulse             # diagnose a service

3. Search (Tier 1 — like grep/stern):
     $ inspect grep "error" arte --since 1h
     $ inspect logs arte/pulse --since 30m --follow

4. Fix it (Tier 1 — dry-run first):
     $ inspect edit arte/atlas:/etc/atlas.conf 's|old|new|'        # shows diff
     $ inspect edit arte/atlas:/etc/atlas.conf 's|old|new|' --apply
     $ inspect restart arte/atlas --apply

5. Cross-medium search (Tier 2 — LogQL):
     $ inspect search '{server="arte", source="logs"} |= "error"' --since 1h
     $ inspect search '{...} or {source="file:/etc/atlas.conf"} |= "milvus"'

6. Compose with shell (Tier 3):
     $ inspect search '...' --json | jq '...' | xargs inspect restart ...

SELECTORS (used in every verb):
  <server>[/<service>][:<path>]  or  @alias-name

  server:   arte | prod,staging | 'prod-*' | all | '~staging'
  service:  pulse | pulse,atlas | 'milvus-*' | /regex/ | storage | '*' | _ | '~name'
  path:     /etc/atlas.conf | '/var/log/*.log'

  Aliases:  inspect alias add plogs '{server="arte",service="pulse",source="logs"}'
            inspect search '@plogs |= "error"'

READ VERBS: logs, grep, cat, ls, find, ps, status, health, volumes, images,
  network, ports, why, recipe, connectivity, search, fleet
  flags: --since --until --tail -f -i -s -w -F -E -A -B -C -v -m -c --json

WRITE VERBS (dry-run default — add --apply):
  restart, stop, start, reload, cp, edit, rm, mkdir, touch, chmod, chown, exec
  flags: --apply --diff --yes --yes-all

LOGQL SEARCH (always one quoted string):
  labels:   server, service, source
  filters:  |= "lit"  != "lit"  |~ "regex"  !~ "regex"
  stages:   | json  | logfmt  | pattern  | regexp
            | line_format  | label_format  | drop  | keep
            | <field> <op> <value>
            | map { <sub-query> }           (Splunk-style $field$)
  metrics:  count_over_time({...} [5m])     rate({...} [5m])
            sum by (service) (count_over_time({...} [5m]))
            topk(5, sum by (service) (rate({...} [1h])))

TRANSLATION GUIDE:
  grep -i "error" file       → inspect grep "error" arte/svc:/path -i
  stern --since 30m pulse    → inspect logs arte/pulse --since 30m
  sed -i 's/old/new/' file   → inspect edit arte/svc:/path 's/old/new/' --apply
  scp file host:/path        → inspect cp file arte/svc:/path --apply
  docker restart pulse       → inspect restart arte/pulse --apply
  {job="x"} |= "error"      → inspect search '{server="arte",source="logs"} |= "error"'

EXIT CODES:  0 = success/dry-run  |  1 = no matches (search/grep)  |  2 = error
```

---

## 16. Phased Rollout

We ship v1 complete.

- *Phase 0*: Cargo project, clap CLI, namespace resolver, add/list/remove/test/show
- *Phase 1*: Persistent SSH (`openssh` native-mux), connect/disconnect, Codespace detection
- *Phase 2*: Discovery + status/health, profile schema, async drift
- *Phase 3*: Selector parser + resolver, alias expansion, alias commands
- *Phase 4*: Read verbs (logs, grep, cat, ls, find, ps, status, health, volumes, images, network, ports)
- *Phase 5*: Write verbs + safety contract + audit log + diff renderer (restart, stop, start, reload, cp, edit, rm, mkdir, touch, chmod, chown, exec)
- *Phase 6*: LogQL parser (chumsky) — log queries, metric queries, all stages, or-union
- *Phase 7*: Source readers (logs, file, dir, discovery, state, volume, image, network, host) + map stage (cross-medium chaining) — built and tested together since map sub-queries depend on reader interfaces
- *Phase 8*: Filter pushdown + streaming + remote tooling probe
- *Phase 9*: Connectivity, `why`, recipes (default + user, including mutating recipes)
- *Phase 10*: Structured output (SUMMARY/DATA/NEXT, correlations, JSON schema versioning, audit commands)
- *Phase 11*: Fleet for all verbs
- *Phase 12*: Distribution (curl installer, brew tap, GitHub releases, Docker image)

**v2 (future):** OS keychain, per-user policies, TUI mode, Kubernetes discovery, distributed tracing, pure-`russh` fallback, parameterized/chained aliases.

---

## 17. Out of Scope (v1)

Web UI / TUI. Kubernetes-native modes. Persistent monitoring / alerting. Password-based SSH auth. Remote agents. Metrics aggregation beyond LogQL metric queries. Distributed tracing. OS keychain. LLM integration of any kind. Per-user policies (deferred to v2). Parameterized / chained aliases (deferred to v2).

---

## 18. Success Criteria

1. New user, fresh laptop → configured access in under 60 seconds
2. `inspect connect <ns>` → no further passphrase prompts for the TTL
3. Cold-start verb → under 200ms (cached profile)
4. `inspect grep "error" arte --since 1h` → first results in under 2 seconds across 5 servers
5. `inspect setup <ns>` → usable profile in under 30 seconds
6. Selector grammar works uniformly across all verbs and inside LogQL labels
7. `{...} or {...}` multi-source works regardless of mediums
8. SUMMARY/DATA/NEXT is stable enough that scripts consume any command's `--json` without per-command parsing
9. Cross-medium `map` works without shell fallback
10. Tool works identically on laptops, CI, Docker, Codespaces
11. Aliases work in verb commands and LogQL queries
12. Every write verb produces a meaningful dry-run preview without `--apply`
13. Every `--apply` is recorded in the audit log
14. Atomic in-place edits: failed `edit` leaves file unchanged
15. `inspect revert <audit-id>` restores the snapshotted original; a revert of a revert works
16. Single static binary, < 15MB, no runtime deps
16. **A devops with grep/stern/kubectl/sed/scp experience uses Tier 1 fluently within 5 minutes**
17. **A Loki/Grafana user writes correct Tier 2 queries on the first try**
18. **An LLM agent given the LogQL spec + reserved-labels table generates correct queries and dry-run-then-apply sequences on the first try**

---

## 19. Appendix: Why These Choices

**Why Rust?** Python startup (~150ms) too slow. Python distribution is the silent killer of internal tools. Go would work; user prefers Rust for ceiling and skills.

**Why `openssh` crate with `native-mux`?** 20+ years of OpenSSH security review. Native protocol access. Zero re-implementation of host-key verification, ssh-agent, ProxyJump.

**Why three tiers?** A unified DSL forces every user to learn it. Three tiers means 80% never see a DSL.

**Why LogQL?** Industry-standard via Grafana/Loki. Published spec. Designed for filter pushdown. Log/metric query split. LLMs trained on public LogQL corpus generate correct queries.

**Why `<server>/<service>` selectors instead of v5's `ns:tag:medium:locator`?** v5's form had no precedent. `<server>/<service>` mirrors kubectl. Medium/locator move into the LogQL `source` label.

**Why drop v5's Form A (medium-first)?** Three equivalent surfaces multiply cognitive load. One surface means one way to write any query.

**Why drop `then`?** Always-quoted DSL means `|` lives inside the string. No ambiguity.

**Why `map` instead of v5's `for-each-X`?** Splunk SPL's `map` with `$field$` is a known pattern. Same feature, conventional syntax.

**Why split log/metric queries?** Streaming stages emit incrementally. Aggregations need the full window. LogQL's split keeps each type coherent.

**Why aliases in v1?** LogQL selectors with full source addresses are long. Aliases cost little (substitution before parsing) and pay off where friction lives.

**Why mutations in v1?** Operational debugging ends in a hot fix. Two-context (inspect then ssh) kills the time savings. The safety contract (dry-run + apply + diff + audit) makes mutations more auditable than raw SSH.

**Why dry-run by default?** "Preview then apply." Same model as `terraform plan/apply`, `helm --dry-run`, `ansible --check`. Forces `--apply` to catch wrong-target selectors before they become incidents.

**Why diff for content edits?** The diff *is* the change. If the diff is wrong, the apply will be wrong.

**Why save snapshots (not just hashes) in the audit log?** A hash tells you *what* changed but can't undo it. Saving the original file content to `~/.inspect/audit/snapshots/` costs kilobytes per edit and gives you `inspect revert <audit-id>` — the difference between "I know what I broke" and "I can fix what I broke." Users who trust the revert are users who actually use write verbs on production.

**Why `$field$` (Splunk-style) rather than `{field}` or `$field` for map interpolation?** `$field$` is Splunk SPL's convention. `{field}` collides with LogQL's `{{.field}}` in `line_format`. `$field` without the trailing `$` is ambiguous with shell variables. The double-`$` delimiter is unambiguous inside a single-quoted string and is what Splunk users and LLMs trained on Splunk docs expect.

**Why no LLM integration?** The tool gives devops and scripts a reliable surface. LLM-driven workflows pipe `--json` output to the LLM of their choice. Embedding LLM calls adds latency, privacy issues, and API dependencies.
