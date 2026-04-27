# Inspect CLI — v0.1.1 Patch Spec

**Source:** Field notes from 2026-04-27 (two sessions, ~60 calls, production debugging on `arte` namespace)
**Supersedes:** v0.1.0
**Rule:** Nothing deferred. Every item ships in this release. Each one is a real friction point from real usage.

---

## Patch 1: `--follow` / `-f` (CRITICAL — documented but missing)

**Problem:** The bible (§7.2), help system, and cheatsheet all document `--follow` / `-f` on `inspect logs`. It doesn't exist. Users expect it. It's the #1 gap.

**Spec:**

```bash
inspect logs arte/ws-bridge --follow
inspect logs arte/ws-bridge --follow --match "error"
inspect logs arte/ws-bridge,api,worker --follow --merged
```

Behavior:
- Streams new log lines as they arrive, indefinitely until Ctrl+C
- Implemented via `docker logs -f <container>` on the remote over SSH
- Each line emits immediately (no buffering — use `ssh -tt` or `stdbuf -oL` to force line-buffered output from the SSH channel)
- Works with `--match` (Patch 3) to filter the stream in real time
- Works with `--merged` (Patch 5) to interleave multiple containers
- Works with `--json` for structured streaming output
- Works with `--since` to start the stream from a specific point and then follow
- The SUMMARY/DATA/NEXT envelope is omitted during follow (streaming mode); a summary is printed on Ctrl+C if the terminal supports it

**Exit behavior:**
- Ctrl+C: clean shutdown, print summary line ("Streamed 142 lines from arte/ws-bridge in 4m 12s"), exit 0
- SSH disconnection: reconnect automatically (reuse ControlMaster recovery from Patch 10), resume streaming, print notice

**Test:** `inspect logs arte/ws-bridge --follow` → see new lines appear within 1 second of the container producing them. No buffering delay.

---

## Patch 2: Fix phantom service names in discovery

**Problem:** Discovery generates short-name aliases (`api`, `worker`, `pulse`, `backend`) that resolve in the selector parser but match no actual Docker container. Users try the short name, get "No such container," lose trust.

**Root cause:** Discovery is deriving nicknames from Docker Compose labels, image names, or container name fragments, but the resolver then tries to pass these nicknames to `docker logs <nickname>` which needs the actual container name.

**Spec:**

1. During discovery, for every generated service short-name, **validate that it resolves to exactly one running container** by running `docker ps --filter name=<shortname> --format '{{.Names}}'` (or equivalent).
2. If the short name matches zero containers: **do not add it to the profile.** It's noise.
3. If the short name matches multiple containers: **do not add it as a simple alias.** Instead, note the ambiguity in `discovery_warnings` and let the user create explicit aliases if they want.
4. Always keep the full container name (e.g., `luminary-api`) as the primary identifier.
5. For Docker Compose services: extract `com.docker.compose.service` label and use that as the short name **only if** it maps to exactly one running container.

**Profile change:** Each service entry gains an explicit `container_name` field (the actual `docker ps` name) that is always used in Docker commands. The `name` field is the user-facing short name (Compose service name when available, container name otherwise).

```yaml
services:
  - name: luminary-api           # user-facing, used in selectors
    container_name: luminary-api  # actual docker container name, used in docker commands
    compose_service: api          # from label, informational only
```

**The selector resolver must always resolve to `container_name` when constructing Docker commands.** Never pass the short name or compose service name directly to `docker logs`/`docker exec`.

**Test:** After `inspect setup arte`, every service listed in `inspect ps arte` must be addressable by its listed name in `inspect logs arte/<name>`.

---

## Patch 3: `--match` and `--exclude` on `inspect logs` and `inspect grep`

**Problem:** Users want to filter log output without writing LogQL. `inspect logs arte/worker --tail 200 | grep "error"` doesn't work well because the output is structured records, not raw lines. Piping to grep breaks multi-line context.

**Spec:**

```bash
inspect logs arte/worker --since 5m --match "error"
inspect logs arte/worker --since 5m --match "embed-doc|413|split"
inspect logs arte/worker --since 5m --exclude "healthcheck"
inspect logs arte/worker --follow --match "error" --exclude "debug"
```

Flags:
- `--match <pattern>` / `-g <pattern>`: only show lines matching this regex. Multiple `--match` flags are OR'd.
- `--exclude <pattern>` / `-G <pattern>`: hide lines matching this regex. Multiple `--exclude` flags are OR'd. Excludes apply after matches.

These are convenience sugar for the LogQL `|=` and `!=` operators, applied at the verb level. They compose with all existing flags (`--since`, `--tail`, `--follow`, `--json`, etc.).

**Filter pushdown:** When `--match` is specified on `logs`, push the pattern to the remote side (via `grep` or `rg` piped after `docker logs`). This is the same filter pushdown that `inspect search` uses, applied at the verb layer.

Also add `--match` and `--exclude` to `inspect grep` for consistency (they act as additional filters on top of the primary pattern).

**Test:** `inspect logs arte/worker --since 5m --match "error" --exclude "healthcheck"` returns only lines containing "error" that don't contain "healthcheck."

---

## Patch 4: Secret masking on exec stdout

**Problem:** `inspect exec arte/api -- "env"` dumps all environment variables including `ANTHROPIC_API_KEY`, `TOOL_TAVILY_API_KEY` etc. plaintext into terminal and agent context. This is a real leak vector for LLM-agent-driven workflows.

**Spec:**

Secret masking is **on by default** for `inspect exec` output. Disable with `--show-secrets`.

**Pattern registry** (hardcoded in v0.1.1, configurable in v0.2.0):

Mask any value where the key matches these patterns (case-insensitive):
```
*_KEY, *_SECRET, *_TOKEN, *_PASSWORD, *_PASS, *_CREDENTIAL, *_CREDENTIALS,
*_API_KEY, *_APIKEY, *_AUTH, *_PRIVATE, *_ACCESS_KEY, *_SECRET_KEY,
DATABASE_URL, REDIS_URL, MONGO_URL, *_DSN, *_CONNECTION_STRING
```

**Masking behavior:**
- In `key=value` formatted output (env vars): replace value with `****` preserving the first 4 and last 2 characters for identification: `ANTHROPIC_API_KEY=sk-a****k3`
- In JSON output: same masking on matching keys
- In unstructured output: no masking (we can't reliably identify secrets in free text)

**Flags:**
- `--show-secrets`: disable masking, show raw values
- `--redact-all`: mask ALL env var values, not just pattern-matched ones (paranoid mode)

**Audit log:** When `--show-secrets` is used, record `secrets_exposed: true` in the audit entry.

**Test:** `inspect exec arte/api -- "env" --apply` shows `ANTHROPIC_API_KEY=sk-a****k3`. `inspect exec arte/api -- "env" --apply --show-secrets` shows the full value.

---

## Patch 5: Multi-container log merge (`--merged`)

**Problem:** Debugging a chain (ws-bridge → luminary-api → luminary-worker) requires three separate log calls and visual interleaving by timestamp. This is the same workflow that made stern popular — merged, time-sorted, multi-container output.

**Spec:**

```bash
inspect logs arte/ws-bridge,luminary-api,luminary-worker --since 2m --merged
inspect logs arte/ws-bridge,luminary-api,luminary-worker --follow --merged --match "413"
```

Behavior:
- `--merged` enables time-sorted interleaving of log lines from all selected services
- Each line is prefixed with the service name in color (stern convention): `[ws-bridge] 2026-04-27T14:32:18 ...`
- Sorting is by timestamp parsed from the log line (Docker json-file driver provides this)
- For `--follow --merged`: lines emit in real-time arrival order (not strictly time-sorted, because clock skew between containers is possible). This matches stern's behavior.
- Without `--merged`, multi-container selectors show logs grouped by container (current behavior, still useful)

**Implementation:** Fan out `docker logs` to N containers in parallel. Each stream tagged with the service name. Merge streams by timestamp (for batch) or by arrival time (for follow). Use a priority queue (BinaryHeap) for batch merging.

**Output:** The `_source` field in each JSON record already identifies the container. For human output, add a colored prefix. For `--json`, no change needed (records are already tagged).

**Test:** `inspect logs arte/ws-bridge,luminary-api --since 2m --merged` shows interleaved lines sorted by timestamp with service name prefix.

---

## Patch 6: Split `exec` into `run` (read) and `exec` (write)

**Problem:** 90% of `exec` usage in field testing was read-only (`env`, `docker logs`, `cat`, `ls`). The `--apply --allow-exec` double-flag pattern doubles call count on every diagnostic. The safety contract is correct for writes but absurd for reads.

**Spec:**

**New verb: `inspect run`** — read-only command execution. Runs immediately, no `--apply` needed, not audit-logged (it's a read).

```bash
inspect run arte/luminary-api -- "env"
inspect run arte/luminary-api -- "cat /etc/atlas.conf"
inspect run arte/luminary-api -- "docker logs --since 3m luminary-worker 2>&1 | grep 413"
```

**Existing verb: `inspect exec`** — write-capable command execution. Dry-run by default, `--apply` to execute, audit-logged.

```bash
inspect exec arte/postgres -- "psql -c 'VACUUM ANALYZE;'" --apply
```

The split:
- `run` = kubectl exec equivalent for diagnostics. No gate. No audit. Immediate.
- `exec` = write-capable. Gated. Audited. `--apply` required. The old `--allow-exec` flag is removed — `--apply` alone is sufficient (Patch 7 addresses this).

Secret masking (Patch 4) applies to both `run` and `exec` output.

**Help text update:** `inspect help write` covers `exec`. New `inspect help run` topic for read-only execution. `inspect help examples` updated with both patterns.

**Migration:** The old `inspect exec ... --apply --allow-exec` pattern still works (both flags accepted, `--allow-exec` is a no-op alias that prints a deprecation notice). Remove the alias in v0.2.0.

**Test:** `inspect run arte/luminary-api -- "env"` returns immediately with masked env output. No `--apply` needed. No audit entry.

---

## Patch 7: Collapse `--apply --allow-exec` into `--apply` alone

**Problem:** Two flags for the same intent. `--allow-exec` exists because exec couldn't distinguish read from write. With `run` (Patch 6) handling reads, `exec` is explicitly for writes. `--apply` is sufficient.

**Spec:**

- `inspect exec <selector> -- <cmd> --apply`: runs the command. One flag.
- `--allow-exec` becomes a deprecated alias for backward compatibility. Prints: `note: --allow-exec is deprecated; --apply is sufficient. see: inspect help exec`
- Remove `--allow-exec` entirely in v0.2.0.

**Test:** `inspect exec arte/postgres -- "psql -c 'SELECT 1'" --apply` works without `--allow-exec`.

---

## Patch 8: `inspect help <command>` fallback

**Problem:** `inspect help add` errors "unknown help topic 'add'". Users expect it to show help for the `add` command.

**Spec:**

When `inspect help <arg>` is called and `<arg>` is not a recognized help topic:
1. Check if `<arg>` is a recognized command name
2. If yes: display that command's `--help` output (same as `inspect <arg> --help`)
3. If no: show the error with suggestions (fuzzy match against both topics and commands)

```
$ inspect help add
# → equivalent to inspect add --help

$ inspect help serch
error: unknown topic or command 'serch'.
  Did you mean: search
  see: inspect help search  or  inspect search --help
```

**Test:** `inspect help logs`, `inspect help edit`, `inspect help search` all produce useful output. No "unknown topic" for any valid command.

---

## Patch 9: Progress indicator on slow log fetches

**Problem:** `inspect logs arte/ws-bridge --since 5m` hangs silently for 30 seconds on large log files. User thinks the tool is broken.

**Spec:**

When a log command produces no output for 2 seconds:
- Show a progress line: `Scanning logs for arte/ws-bridge...` (on stderr, so it doesn't pollute piped output)
- Update every 2 seconds with elapsed time: `Scanning logs for arte/ws-bridge... (6s)`
- Clear the progress line when first results arrive
- In `--json` mode: no progress line (machine consumer)
- In `--follow` mode: show `Waiting for new logs from arte/ws-bridge...` if no output after 5 seconds

**Implementation:** Use `indicatif` (already in the stack) for a simple spinner on stderr. The spinner runs on a background tokio task and is cancelled when the first record arrives.

**Test:** `inspect logs arte/<chatty-service> --since 1h` on a service with a large log file shows progress within 2 seconds instead of hanging silently.

---

## Patch 10: `--since-last` cursor for incremental polling

**Problem:** Agent workflows poll logs repeatedly (`--since 3m` every minute). This creates overlapping windows (re-fetching the same lines) or gaps (events between polls). There's no "give me everything since my last call."

**Spec:**

```bash
inspect logs arte/worker --since-last
inspect logs arte/worker --since-last --match "error"
inspect grep "error" arte --since-last
```

Behavior:
- On first call: equivalent to `--since 5m` (configurable default). Stores a cursor.
- On subsequent calls: fetches everything since the last cursor position. No overlap, no gaps.
- Cursor stored in `~/.inspect/cursors/<ns>-<service>.txt` (simple timestamp, mode 600)
- `--since-last` is mutually exclusive with `--since` (clear error if both specified)
- `inspect logs --reset-cursor arte/worker` resets the cursor for that service
- Cursor is per-namespace, per-service, per-user

**Cursor file format:**
```
# inspect cursor — do not edit
ns=arte
service=luminary-worker
last_ts=2026-04-27T14:32:18.443Z
last_call=2026-04-27T14:32:20Z
```

**For `--follow`:** `--since-last` sets the start point of the follow stream, then the cursor is updated continuously as lines arrive.

**Test:** 
1. `inspect logs arte/worker --since-last` returns results, records cursor
2. Wait 30 seconds
3. `inspect logs arte/worker --since-last` returns only new lines since step 1
4. No duplicates, no gaps

---

## Patch 11: Inner command exit code surfacing on `exec` and `run`

**Problem:** `inspect exec` reports "1 ok, 0 failed" but doesn't surface the inner command's exit code. `grep` returning 1 (no match) vs `psql` returning 1 (error) are very different, and neither is visible.

**Spec:**

- The output record gains an `_exit_code` field:

```json
{
  "schema_version": 1,
  "_source": "arte/luminary-api:exec",
  "_exit_code": 1,
  "summary": "Command exited with code 1",
  ...
}
```

- Human output: append exit code to the summary line:

```
Command completed (exit code: 1)
```

- The `inspect run` / `inspect exec` process exit code mirrors the inner command's exit code. If `docker exec ... grep foo` returns 1, `inspect run` returns 1. This enables shell composition:

```bash
inspect run arte/api -- "grep 'error' /var/log/app.log" && echo "found" || echo "not found"
```

- In fleet mode: per-target exit codes in the summary table.

**Test:** `inspect run arte/api -- "grep 'nonexistent' /etc/hosts"` exits with code 1 and shows "exit code: 1" in output.

---

## Patch 12: `--reason` audit comment on write verbs

**Problem:** Audit log shows what was done but not why. For agent traces and team forensics, the "why" is the most valuable part.

**Spec:**

```bash
inspect edit arte/atlas:/etc/foo 's/old/new/' --apply --reason "fixing 413 loop in embed pipeline"
inspect restart arte/pulse --apply --reason "deploying hotfix for SSE bug"
inspect exec arte/postgres -- "VACUUM ANALYZE" --apply --reason "post-migration cleanup"
```

- `--reason <text>`: optional free-text annotation on any write verb
- Stored in the audit log entry:

```json
{
  "ts": "2026-04-27T14:32:18Z",
  "verb": "edit",
  "reason": "fixing 413 loop in embed pipeline",
  ...
}
```

- Visible in `inspect audit ls`:

```
2026-04-27 14:32  edit   arte/atlas:/etc/foo   "fixing 413 loop in embed pipeline"
2026-04-27 14:35  restart  arte/pulse           "deploying hotfix for SSE bug"
```

- `inspect audit ls --reason <pattern>` filters by reason text
- If `--reason` is not provided, the field is `null` in JSON, blank in human output. Not required.

**Test:** `inspect edit ... --apply --reason "test"` then `inspect audit ls` shows the reason text.

---

## Patch 13: Discovery `docker inspect` per-container fallback

**Problem:** `inspect setup` warned about a 30s timeout on batch `docker inspect`. Not clear what data was lost.

**Spec:**

Current behavior: batch all container IDs into one `docker inspect <id1> <id2> ...` call.

New behavior:
1. Try batch `docker inspect` first (fast path)
2. If batch times out (>10s) or fails: fall back to per-container `docker inspect <id>` sequentially
3. If an individual container inspect times out (>5s): skip that container, add to `discovery_warnings` with explicit message: "Timed out inspecting container <name>. Info may be incomplete for this service."
4. Never lose data silently. Every skip is recorded in warnings.
5. At the end of discovery, if any containers were skipped, print: "Warning: <N> containers timed out during inspection. Run 'inspect setup arte --retry-failed' to retry."

**Test:** Discovery completes successfully even if one container's `docker inspect` hangs. Warnings clearly list which containers were skipped.

---

## Summary of changes

| Patch | Type | Field note # | Impact |
|---|---|---|---|
| 1 | Missing feature | #8 | `--follow` streaming |
| 2 | Bug fix | #1 | Phantom service names |
| 3 | New feature | #6 | `--match` / `--exclude` on logs |
| 4 | New feature | #4 | Secret masking on exec/run output |
| 5 | New feature | #9 | Multi-container `--merged` log view |
| 6 | Architecture | #5, #13 | Split `exec` into `run` (read) + `exec` (write) |
| 7 | UX fix | #13 | Collapse `--apply --allow-exec` to `--apply` |
| 8 | UX fix | #3 | `help <command>` fallback |
| 9 | UX fix | #2 | Progress indicator on slow fetches |
| 10 | New feature | #12 | `--since-last` polling cursor |
| 11 | New feature | #11 | Inner exit code surfacing |
| 12 | New feature | #14 | `--reason` audit comment |
| 13 | Bug fix | #7 | Discovery per-container fallback |

## Implementation order (suggested)

Phase A — Critical fixes (do first):
- Patch 2 (phantom services — trust issue)
- Patch 1 (`--follow` — missing documented feature)
- Patch 8 (`help <command>` — discoverability)

Phase B — The agent-workflow tier (high value):
- Patch 6 (split run/exec — eliminates double-flag friction)
- Patch 7 (collapse --allow-exec — cleanup from Patch 6)
- Patch 3 (`--match`/`--exclude` — most-requested filter)
- Patch 10 (`--since-last` — agent polling)

Phase C — Quality and safety:
- Patch 4 (secret masking — leak prevention)
- Patch 9 (progress indicator — UX)
- Patch 5 (`--merged` — multi-container)
- Patch 11 (exit code surfacing)
- Patch 12 (`--reason` on audit)
- Patch 13 (discovery fallback)

## Files to update after all patches

- `inspect help` topic index: add `run` to the command list
- `inspect help write`: document `run` vs `exec` split
- `inspect help examples`: update exec examples to use `run` for reads
- `inspect help search`: note that `--match` on logs is the Tier 1 equivalent of `|=`
- README / cheatsheet: add `--follow`, `--match`, `--merged`, `--since-last`, `run`
- Bible v6.2: update §7 (add `run` verb), §8 (simplify `exec`), §7.2 (add `--match`, `--exclude`, `--merged`, `--since-last`)

## Exit criteria for v0.1.1

1. `inspect logs arte/ws-bridge --follow --match "error"` streams filtered logs in real time
2. `inspect logs arte/ws-bridge,api,worker --follow --merged` shows interleaved multi-container stream
3. `inspect run arte/api -- "env"` runs immediately, secrets masked, no --apply needed
4. `inspect exec arte/postgres -- "psql ..." --apply` works with one flag, is audit-logged
5. `inspect logs arte/worker --since-last` polls incrementally with no gaps or duplicates
6. Every service in `inspect ps arte` is addressable by its listed name in all verbs
7. `inspect help logs` shows the logs command help
8. Slow log fetches show progress after 2 seconds
9. `inspect audit ls` shows `--reason` text when provided
10. Zero `--allow-exec` needed anywhere in the workflow
