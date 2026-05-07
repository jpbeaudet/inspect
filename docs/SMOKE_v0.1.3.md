# SMOKE — v0.1.3 release-readiness gate

End-to-end smoke test executed by an agent (Claude Code, this session)
against a real `arte` SSH host before tagging `v0.1.3`. Maps every
release-readiness gate in `INSPECT_v0.1.3_BACKLOG.md` lines 1057–1071
to an executable check.

**Topology.** One real host (`arte`) reached over SSH from this
codespace. No second VM, no `sshd_config` edits, no compose surgery on
real services. Read-mostly: ~80% of the smoke is read verbs hitting
real services for realism. The remaining ~20% (every write-surface
verb) is scoped to a single throwaway sandbox container created at the
start of the smoke and torn down at the end.

**Sandbox container.** One `nginx:alpine` container created via
`docker run -d --name inspect-smoke-<rand> --label inspect-smoke=1`.
Every write verb in the smoke targets this container. Cleanup at the
end is a single label-filtered `docker rm -f` — cannot affect anything
the smoke didn't create.

**Cleanup invariant.** All artifacts the smoke creates on `arte` are
either (a) inside the sandbox container, (b) at `/tmp/inspect-smoke-*`
on the host filesystem, or (c) labeled `inspect-smoke=1`. The
final cleanup phase removes all three classes idempotently.

---

## Conventions an agent running this smoke must know

These caught a previous smoke run. Read them once before any phase.

1. **`--json` envelope shape — two flavors.** Read-**aggregating**
   verbs (`status`, `health`, `why`, `audit ls`, `audit show`,
   `audit grep`, `audit gc`, `audit verify`, `cache show`,
   `compose ls`, `compose ps`, `recipe list`, `setup --json`)
   emit **one envelope per invocation** with top-level keys
   `{schema_version, summary, data, next, meta}`. The F1 services
   array is at `.data.services`; F7.5 state at `.data.state`;
   audit-ls entries at `.data.entries[]`; audit-show entry at
   `.data.entry`; compose ls projects at `.data.compose_projects[]`;
   cache provenance at `.meta.source.{mode, stale, runtime_age_s,
   inventory_age_s}`. Read-**listing** / **streaming** verbs
   (`ports`, `images`, `volumes`, `network`, `ps`, `find`, `ls`,
   `logs`, `grep`, `cat`, `search`, `run --stream`, `history show`)
   emit **NDJSON** — one JSON object per line, no top-level
   envelope. The verb's `-h` includes the line `Emit a single JSON
   envelope` for envelope verbs and `Emit line-delimited JSON (one
   record per line)` for NDJSON verbs — check `-h` first if unsure.
   Both shapes accept `--select '<jq filter>'` (F19, v0.1.3); on an
   envelope verb the filter sees the whole envelope, on a streaming
   verb the filter runs per-line.
2. **`--quiet` is mutex with `--json`.** This is the F7.4 contract:
   `--quiet` strips the human-renderer indent; `--json` produces
   structured output that does not need it. Combining them is a
   clap usage error (exit 2). Use `--json --select '<filter>'` for
   the canonical machine path; use `--quiet` alone when piping the
   human format to a non-JSON filter.
3. **Audit-list output is newest-first.** `inspect audit ls --limit
   N --json` returns the most recent N entries under
   `.data.entries[]` of the L7 envelope (commit `34ae25d` brought
   every audit verb under the standard envelope). The flag is
   `--limit` (NOT `--tail`); the most recent entry is
   `.data.entries[0]` / `head -1`, NOT `tail -1`. The projection
   is `(id, ts, verb, selector, exit, diff_summary, is_revert,
   reason)`; the `revert` block is **not** included — round-trip
   via `inspect audit show <id> --json --select '.data.entry'`
   to inspect `revert.kind` / `payload` / `preview`. (Pre-v0.1.3
   recipes that use `.[0]` / bare-array indexing date from the
   pre-L7 era and yield `null` against the current envelope.)
4. **Field selectors are `<ns>/<svc>`** (matches inventory) or
   `<ns>/<container_name>` (F5 dual-axis resolver — emits a
   one-line stderr breadcrumb pointing at the canonical form
   unless `INSPECT_NO_CANONICAL_HINT=1` is set; canonical form
   resolves silently).
5. **Streaming verbs emit raw bytes, not JSON, by default.** `logs`,
   `run`, `cat`, `grep`, `find`, `search`, `exec` emit raw lines
   (with L7 redaction applied) on the human path -- they are not
   structured. Add `--json` to get a per-line NDJSON envelope; the
   `--select` flag (and external `jq`) operate on the envelope
   form, not the raw text. Don't pipe the default human output
   into a JSON filter.
6. **Master socket reuse.** Once `inspect connect <ns>` has
   succeeded, every subsequent verb reuses
   `~/.inspect/sockets/<ns>.sock` and does **not** need the key
   passphrase env var. The setup precheck path was the one
   exception (fixed in commit 7d588d2 — precheck now reuses the
   master socket instead of spawning a fresh BatchMode probe).

---

## Pre-flight (before opening any SSH)

| Step | Command | Pass criteria |
|---|---|---|
| P0.1 | `cargo fmt --check` | exit 0 |
| P0.2 | `cargo clippy --all-targets -- -D warnings` | exit 0 |
| P0.3 | `cargo test 2>&1 \| grep -E "^test result\|^running" \| tail -40` | every suite `0 failed` |
| P0.4 | `cargo build --release --locked` | exit 0; binary at `target/release/inspect` |
| P0.5 | `target/release/inspect --version` | matches `Cargo.toml` |
| P0.6 | grep policy gate from `CLAUDE.md` ("No silent deferrals") | every hit legitimate |

Abort the smoke at the first P0 failure; do not open SSH.

---

## Inputs the operator hands over

1. SSH private key — saved to `~/.ssh/inspect_arte_ed25519` mode 0600
   (and `.pub` if available).
2. Connection details: `host`, `user`, `port` (default 22).
3. Confirmation that `docker run -d --name inspect-smoke-<rand> ...`
   on `arte` is acceptable and that the `inspect-smoke=*` label
   namespace is not in use.

The smoke writes nothing else outside `/tmp/inspect-smoke-*` and the
sandbox container.

---

## P1 — connect / setup / inventory (read-only)

Maps to: F1, F2, L4 (key-auth path; the password-auth path is unit-
covered and skipped here unless the operator explicitly stages a
`legacy-box` namespace).

```sh
inspect setup arte --force
inspect status arte
inspect status arte --json --select '{state: .data.state, services_count: (.data.services | length), summary, source_mode: .meta.source.mode}'
inspect connections
# connections --json: verify shape on contact (read verbs evolved at
# different points; the auth/session_ttl/expires_in fields are the
# L4 contract -- their wrapping may be NDJSON or .data array).
inspect connections --json | head -2
```

| Gate | Pass criteria |
|---|---|
| F1 | `inspect status arte` reports `services_count > 0` immediately after `--force` setup. The 2nd / 3rd field user's regression must not reproduce. |
| F2 | Healthy host: zero `warning:` lines on stderr from `setup` / `status`. (Slow-but-successful is debug-level; partial timeouts collapse to one summary line.) |
| F2 inventory-scaling | `INSPECT_DOCKER_INSPECT_TIMEOUT` not needed on a 10-15 container host. |
| L4 | `inspect connections` table includes `auth` / `session_ttl` / `expires_in` columns; JSON envelope carries the same fields. |
| F7.5 | `--json` output payload sits under `.data`; `.data.state` is one of `ok` / `no_services_matched` / `empty_inventory`. (`--json --quiet` is a clap usage error — see the conventions preamble.) |

---

## P2 — diagnostic surface (read-only)

Maps to: F4, F5, F7, F8, F10, L7, L9, L10.

```sh
# F4 deep-bundle on whichever real service has the lowest health score.
# If every real service is healthy, run against the sandbox container
# (created at the start of P3) instead.
inspect why arte/<svc>
inspect why arte/<svc> --no-bundle
inspect why arte/<svc> --log-tail 5
inspect why arte/<svc> --json --select '.data.services[0] | {recent_logs, effective_command, port_reality}'

# F8 cache freshness. The provenance line `SOURCE: live` /
# `SOURCE: cached <age>` is the F8 contract surface; the per-ns
# breakdown is in the human output of `status` itself, not in a
# separate `cache show <ns>` call. `inspect cache show` (no
# positional) lists ALL cached namespaces with runtime/inventory
# ages -- use that for the cross-ns view.
inspect status arte                       # first hit -> SOURCE: live
inspect status arte                       # second hit -> SOURCE: cached <Ns>
inspect status arte --refresh             # forces SOURCE: live
inspect cache show                        # cross-ns ages (no positional)
inspect cache clear arte                  # per-ns invalidate

# F5 dual-axis selector resolution. Pick a real container whose
# compose service name differs from its docker container name.
inspect status 'arte/<docker-name>'       # canonical hint on stderr
inspect status 'arte/<compose-name>'      # silent (canonical form)

# F7 ports filtering + L9 UDP probe + L10 port-level drift
inspect ports arte
inspect ports arte --proto udp
inspect ports arte --port 53
# ports --json emits NDJSON (one record per line; documented in -h).
# Slurp NDJSON into an array with --select-slurp for array-style queries.
inspect ports arte --json | head -3                              # one JSON record per line
inspect ports arte --json --select 'group_by(.proto) | map({proto: .[0].proto, count: length})' --select-slurp

# L10 port-drift (capture two snapshots ~30s apart; if real services
# don't change ports we instead drift the sandbox container after P3)
inspect status arte --json > /tmp/inspect-smoke-snap-1.json

# L7 redaction. Pick any log line on the host that contains an
# Authorization header, a Bearer token, a Postgres URL with creds,
# or a PEM block. (Every real host has these; we don't plant them.)
inspect logs arte/<svc> --tail 200 | grep -E '(Bearer|Authorization|postgres://)'
# Pass: every match shows '<redacted>' / 'user:****@' / '[REDACTED PEM KEY]'.
inspect logs arte/<svc> --tail 200 --show-secrets | grep -E '(Bearer|Authorization|postgres://)'
# Pass: with --show-secrets the same lines surface unmasked.

# F10 polish bundle
inspect cat arte/<svc>:/etc/passwd --lines 1-5
inspect cat arte/<svc>:/etc/passwd --lines 1-5 --json --select '{n, line}'
inspect grep --help | head -40           # MODEL/EXAMPLE/NOTE block present
inspect status arte --quiet | head -5    # pipe-clean rendering
inspect why arte/<container-not-service> # 3-line chained hint, exit 0
```

| Gate | Pass criteria |
|---|---|
| F4 | When run against an unhealthy service, `recent_logs[]` is populated, `effective_command` is non-null, `port_reality[]` cross-references config + entrypoint + listeners. Healthy services produce empty arrays + `null`, byte-for-byte unchanged from v0.1.2. |
| F4 cap | At most 4 extra remote commands per service per `why` invocation; partial failures don't kill the bundle. |
| F8 | `SOURCE:` line on every read envelope; second status hits cache; `--refresh` forces live; `cache show` reflects last fetch. |
| F8 invalidation | (Verified in P3 after a sandbox restart — first read after the mutation is `SOURCE: live`, no `--refresh` needed.) |
| F5 | Docker-container-name selector emits the canonical-hint breadcrumb on stderr; compose-service-name selector is silent. JSON `aliases[]` field present. |
| F7.3 / L9 | `--proto udp` narrows to UDP rows only; UDP rows tagged in JSON `proto` field; both `ss` and `netstat` fallback paths exercised on at least one host. |
| F10 | `--lines 1-5` returns exactly 5 records; JSON shape `{n, line}`; `--quiet` removes leading indent; `inspect grep --help` shows MODEL/EXAMPLE/NOTE block. |
| L7 | Authorization / Bearer / URL credentials masked in `inspect logs` output AND in the F18 transcript (verified in P6); `--show-secrets` bypasses. |

---

## P3 — write surface + revert (sandbox container only)

Maps to: F11 (load-bearing safety primitive), F15, L8, F8 mutation
invalidation. Every command in this phase targets the sandbox
container.

```sh
# Stand up the sandbox.
SMOKE_RAND=$(head -c 4 /dev/urandom | xxd -p)
SMOKE_CTR="inspect-smoke-${SMOKE_RAND}"
inspect run arte -- "docker run -d --name ${SMOKE_CTR} \
  --label inspect-smoke=1 nginx:alpine"
inspect run arte -- "docker ps --filter label=inspect-smoke=1"

# F11 universal pre-staged revert. (Sandbox is a bare container, not a
# compose service, so we exercise revert via run + revert audit linkage
# rather than `inspect compose restart`.)
inspect run arte --apply --revert-preview -- "docker stop ${SMOKE_CTR}"
inspect run arte --json --select '.audit_id' > /tmp/inspect-smoke-audit.json
inspect revert --last arte
inspect run arte -- "docker ps --filter name=${SMOKE_CTR} --format '{{.Status}}'"
# Pass: container is running again; audit log shows linked entries.
inspect audit show $(inspect query -r . < /tmp/inspect-smoke-audit.json) | head

# F8 mutation-invalidation
inspect status arte                       # SOURCE: cache
inspect run arte --apply -- "docker restart ${SMOKE_CTR}"
inspect status arte                       # SOURCE: live (no --refresh)

# F15 file transfer roundtrip on host fs
echo "smoke-payload-${SMOKE_RAND}" > /tmp/inspect-smoke-payload.txt
inspect put ./tmp/inspect-smoke-payload.txt arte/_:/tmp/inspect-smoke-up.txt
inspect run arte -- "sha256sum /tmp/inspect-smoke-up.txt"
inspect get arte/_:/tmp/inspect-smoke-up.txt /tmp/inspect-smoke-down.txt
diff /tmp/inspect-smoke-payload.txt /tmp/inspect-smoke-down.txt   # exit 0
inspect put ./tmp/inspect-smoke-payload.txt arte/_:/tmp/inspect-smoke-up.txt --mode 0640
inspect run arte -- "stat -c '%a' /tmp/inspect-smoke-up.txt"      # 640
# put creates -> revert removes (command_pair); put over existing ->
# state_snapshot. Verify the revert kind on the audit entries.
# revert.kind is NOT in the `audit ls --json` projection — round-trip
# the most recent put audit ids through `audit show <id> --json`:
for id in $(inspect audit ls --limit 5 --json --select '.data.entries[] | select(.verb=="put") | .id' --select-raw); do
  inspect audit show "$id" --json --select '.data.entry | "\(.id) verb=\(.verb) rk=\(.revert.kind)"' --select-raw
done

# L8 compose surface (read-only; no per-service down/up on the real
# compose services). If the host has compose projects:
inspect compose ls arte
inspect compose ps arte/<project>
inspect compose logs arte/<project> --tail 20 --match 'error' --exclude 'health'
```

| Gate | Pass criteria |
|---|---|
| F11 | Sandbox stop+revert: container returns to running. Audit log shows the original entry (`revert.kind = command_pair`, payload includes the inverse `docker start`) and the linked revert entry with `auto_revert_of` pointing at the original. |
| F11 cap | Every write-verb invocation in this phase carries `revert.applied: true` after revert; pre-revert audit entries have `applied: true` and `revert.kind` set. |
| F8 invalidation | First `status` after `docker restart` reports `SOURCE: live` with **no** `--refresh` flag. |
| F15 roundtrip | sha256 matches across `put` → `get`; `--mode 0640` reflected in `stat -c %a`; audit fields `transfer_direction` / `transfer_bytes` / `transfer_sha256` populated. |
| F15 revert kinds | `put` over a non-existent target → `revert.kind = command_pair` (rm); `put` over an existing target → `revert.kind = state_snapshot`. |
| L8 | `compose logs --match` / `--exclude` shape on the remote side compiles to a `grep -E` pipeline; per-service narrowing rejects `--volumes` / `--rmi`. |

---

## P4 — run / streaming / script / multi-step (sandbox only)

Maps to: F9, F14, F16, F17, L11, L12, L13. Every command targets the
sandbox container so failures cannot affect real services.

```sh
# F9 stdin forward
printf 'echo from-stdin-pipe\n' > /tmp/inspect-smoke-init.sh
cat /tmp/inspect-smoke-init.sh | inspect run arte \
  -- "docker exec -i ${SMOKE_CTR} sh"
# Pass: stdout contains 'from-stdin-pipe'.
inspect audit ls --limit 1 --json \
  --select '.data.entries[0] | {verb, stdin_bytes, stdin_sha256: .stdin_sha256}'
# Pass: stdin_bytes matches file size.

# F9 loud-failure contract
echo data | inspect run arte --no-stdin -- "cat"   # exit 2 + chained hint

# F14 script mode (heredoc with embedded sh + python; no local quoting)
inspect run arte --file ./tests/smoke/v013/migration.sh -- "${SMOKE_CTR}"
inspect audit ls --limit 1 --json \
  --select '.data.entries[0] | {script_path, script_sha256, script_bytes, script_interp}'
# Pass: script body deduped at ~/.inspect/scripts/<sha>.sh

# F16 streaming + Ctrl-C signal forwarding.
# Run for ~5 seconds then send SIGINT; remote process must die.
timeout --signal=INT 5 inspect run arte --stream \
  -- "docker logs -f ${SMOKE_CTR}" || true
inspect run arte -- "ps -ef | grep -c 'docker logs -f ${SMOKE_CTR}' \
  | grep -v grep || true"
# Pass: zero orphaned 'docker logs -f' processes on arte.
inspect audit ls --limit 1 --json --select '.data.entries[0].streamed'   # true

# L11 --stream + --file composition (lifted clap mutex)
inspect run arte --stream --file ./tests/smoke/v013/migration.sh \
  -- "${SMOKE_CTR}"
inspect audit ls --limit 1 --json --select '.data.entries[0].bidirectional'   # true

# F17 multi-step runner with injected step-3 failure + revert unwind.
# Manifest at tests/smoke/v013/migration.json; step 3 deliberately
# fails (`exit 7`), step 1 + 2 carry revert_cmd entries.
inspect run arte --steps ./tests/smoke/v013/migration.json \
  --revert-on-failure \
  --reason "v0.1.3 smoke F17 unwind probe"
echo "exit=$?"   # non-zero (the failure exit), not 0
inspect audit ls --limit 10 --json \
  --select '[.data.entries[] | select(.steps_run_id != null)] | length'
inspect audit ls --limit 10 --json \
  --select '[.data.entries[] | select(.verb == "run.step.revert")] | length'
# Pass: composite revert walks step 2 then step 1 inverses.
```

| Gate | Pass criteria |
|---|---|
| F9 | Audit `stdin_bytes` matches the file size; remote process actually consumed the bytes (verified by stdout content). `--no-stdin` with data on stdin exits 2 before dispatch. |
| F14 | Script runs end-to-end with embedded `psql -c "..."` / `python -c "..."` shapes and zero local quote-escaping. Audit captures `script_sha256` + `script_bytes`. |
| F16 | Output appears within ~1s of remote emission. `timeout --signal=INT` kills the remote `docker logs -f` cleanly (zero orphaned processes on `arte` after the verb returns). Audit `streamed: true`. |
| L11 | `--stream --file` no longer clap-rejected; audit `bidirectional: true`. Phase 1 (script-write) + phase 2 (PTY exec) + phase 3 (cleanup) all complete; `/tmp/.inspect-l11-*` is gone after the verb returns. |
| F17 | Pipeline stops at step 3 with non-zero exit; `--revert-on-failure` produces `run.step.revert` entries for steps 2 then 1 (reverse order); composite revert payload on the parent entry walks correctly. |
| F17 audit shape | Per-step entries share a `steps_run_id`; parent entry is `verb: run.steps`; per-step is `verb: run.step`. |

---

## P5 — connect-layer + env overlay + keychain

Maps to: F12, F13, L2.

```sh
# F12 env overlay. Probe the host's PATH shape first.
inspect run arte -- "echo \$PATH"          # non-login PATH
inspect run arte -- "ssh \$USER@localhost 'echo \$PATH'" 2>/dev/null \
  || true                                  # login PATH (may fail; OK)

# Stage a helper at a path likely absent from non-login PATH.
inspect run arte --apply -- "mkdir -p ~/.inspect-smoke-bin && \
  echo '#!/bin/sh\necho overlay-helper-ok' \
  > ~/.inspect-smoke-bin/inspect-smoke-helper && \
  chmod +x ~/.inspect-smoke-bin/inspect-smoke-helper"

# Without the overlay, expect 127 / not-found.
inspect run arte -- "inspect-smoke-helper" || \
  echo "expected: command not found"

# Apply the overlay non-interactively.
inspect connect arte --set-path "\$HOME/.inspect-smoke-bin:\$PATH"
inspect run arte -- "inspect-smoke-helper"
# Pass: prints 'overlay-helper-ok'. Audit entry has env_overlay field.

# F13 auto-reauth. We can't change ClientAliveInterval without
# touching sshd_config, so we exercise the same code path by
# explicitly killing the master mid-session.
inspect run arte -- "echo before-disconnect"
inspect disconnect arte
inspect run arte -- "echo after-reauth"   # must auto-reauth
# Pass: exit 0; one-line stderr notice; audit shows linked
# connect.reauth + retry_of entries. The full ClientAliveInterval
# variant is covered by tests/phase_f_v013.rs::f13_*.

# F13 distinct exit class (no auto-retry).
inspect disconnect arte
inspect run arte --no-reauth -- "echo blocked"
echo "exit=$?"   # expect 12 (transport_stale)
inspect run arte --no-reauth --json --select '.failure_class' \
  -- "echo blocked" 2>/dev/null   # "transport_stale"

# L2 keychain. Codespace likely has no DBus session bus.
inspect keychain test
# Pass: either Available + roundtrip ok, OR
# BackendUnavailable with chained hint pointing at `inspect help ssh`.
inspect keychain list
```

| Gate | Pass criteria |
|---|---|
| F12 | Helper resolves with overlay applied; audit entry on the dispatch verb has `env_overlay: {PATH: "..."}`. `--detect-path` non-interactive run on the codespace correctly auto-declines or succeeds based on tty status. |
| F13 auto-reauth | After explicit disconnect, the next verb returns exit 0; one-line stderr notice; audit shows the `connect.reauth` event linked to the verb's `retry_of`. |
| F13 transport class | With `--no-reauth`, same scenario exits **12** (not 255); `failure_class: "transport_stale"` in JSON envelope. |
| L2 | Either roundtrip passes or the unavailable backend reports cleanly with the chained hint. Never panics; never leaks the secret to stderr. |

---

## P6 — observability (transcript + audit GC)

Maps to: F18, L5.

```sh
# F18 transcript should have captured every verb invocation in P1-P5.
TODAY=$(date -u +%Y-%m-%d)
TRANSCRIPT=~/.inspect/history/arte-${TODAY}.log
test -f "${TRANSCRIPT}"
wc -l "${TRANSCRIPT}"
# Verify the fence shape.
grep -E '^── [0-9]+\.[0-9]+ arte #' "${TRANSCRIPT}" | head -5
grep -E '^── exit=[0-9]+ duration=[0-9]+ms audit_id=' "${TRANSCRIPT}" | head -5
# L7 redaction must be applied to the transcript file too.
! grep -E 'Bearer [A-Za-z0-9._-]{16,}' "${TRANSCRIPT}"
! grep -E 'postgres://[^:]+:[^@]+@' "${TRANSCRIPT}"
# F18 grep + cross-link to audit log.
inspect history show arte --grep 'docker restart' | head
SMOKE_AUDIT=$(inspect query -r . < /tmp/inspect-smoke-audit.json)
inspect history show arte --audit-id "${SMOKE_AUDIT}"
inspect history list arte | head

# L5 audit GC dry-run.
inspect audit gc --keep 1d --dry-run --json \
  --select '.data | {policy, entries_total, entries_kept, deleted_entries, freed_bytes}'
# Pass: dry-run reports counts but deletes nothing.
```

| Gate | Pass criteria |
|---|---|
| F18 | Transcript exists; opener / closer fences well-formed on every block; `audit_id` on the closer cross-links to the `~/.inspect/audit/<YYYY-MM>-<user>.jsonl` entry by ID. |
| F18 redaction | No Bearer tokens / no `user:pass@` URLs / no PEM bodies present in the file. (Argv `--password=` redaction covered by unit tests; we don't plant a password in the smoke.) |
| L5 | Dry-run reports correct entries-to-delete count; on-disk audit JSONL is byte-identical before vs after the dry-run. |

---

## P8 — F19 `--select` field-validation gate (must run before P7 cleanup)

Maps to: F19. Closes the v0.1.3 release scope. Each recipe exercises
one chokepoint in a way the synthetic-fixture acceptance tests in
`tests/jaq_select_v013.rs` cannot: (1) on a real audit log built
during P3/P4, (2) on a real compose host, (3) end-to-end with a real
SIGINT round-trip. **Run before P7 cleanup** — the sandbox container
and the audit entries P3/P4 produced are the inputs.

```sh
# (1) Identity-filter round-trip on a deterministic envelope verb.
# `--select '.'` is a no-op; the output bytes must equal the
# unfiltered envelope (modulo trailing newline). Proves the
# envelope chokepoint preserves round-trip identity through the
# serde→jaq→serde pipeline.
#
# IMPORTANT: use `audit ls`, NOT `status`. Two consecutive `status`
# calls produce different `meta.source.{mode, runtime_age_s}` (the
# second one hits the cache), which makes the diff non-deterministic
# and tests cache-freshness rather than the F19 round-trip. `audit
# ls` is read-only against an immutable on-disk projection — the
# byte stream is identical across consecutive calls. (P8-A.1
# follow-up; see "P8 follow-ups" section below.)
inspect audit ls --limit 5 --json > /tmp/inspect-smoke-audit-plain.json
inspect audit ls --limit 5 --json --select '.' > /tmp/inspect-smoke-audit-id.json
diff -q /tmp/inspect-smoke-audit-plain.json /tmp/inspect-smoke-audit-id.json
# Pass: files compare equal (or differ only by a single trailing newline).

# (2) audit show projection — extract `.data.entry.revert.kind`
# from a real recorded write entry. Closes CLAUDE.md `audit show`
# envelope-path note (the revert block is at .data.entry.revert,
# not .[0].revert).
#
# IMPORTANT: filter by `verb == "run"` (or whichever write verb P3
# / P4 used last). Filtering only by `is_revert == false` and
# `exit == 0` will pick up `connect` entries — connect succeeds,
# is not a revert, and has no `revert` block in the F11 contract,
# so `.data.entry.revert.kind` would yield null and trip
# `--select-raw`'s non-string error. (P8-A.2 follow-up; see "P8
# follow-ups" section below.)
WRITE_ID=$(inspect audit ls --limit 50 --json \
  --select '[.data.entries[] | select(.verb == "run") | select(.is_revert == false) | select(.exit == 0) | .id][0]' \
  --select-raw)
echo "WRITE_ID=${WRITE_ID}"
inspect audit show "$WRITE_ID" --json --select '.data.entry.revert.kind' --select-raw
# Pass: prints exactly one of: command_pair | state_snapshot | composite | unsupported.

# (3) compose ls projection — project-name list via the documented
# envelope path. Closes the CLAUDE.md "compose ls --json envelope
# path" footgun: pre-F19 the recipe was `jq -r '.compose_projects[].name'`
# (wrong — the path is under .data); the F19 contract pins
# .data.compose_projects[].name and --select-raw strips quotes.
inspect compose ls arte --json \
  --select '.data.compose_projects[].name' --select-raw
# Pass: at least one unquoted project name on stdout; exit 0.

# (4) history ↔ audit cross-link via --select-raw, replacing the
# pre-F19 `jq -r .` shape. Pulls the newest audit id and feeds it
# to `inspect history show --audit-id` to surface the matching
# transcript block.
LAST_ID=$(inspect audit ls --limit 1 --json \
  --select '.data.entries[0].id' --select-raw)
inspect history show arte --audit-id "$LAST_ID" | head
# Pass: at least one transcript line printed; ${LAST_ID} is non-empty.

# (5) Streaming chokepoint × F16 signal-forwarding × F19 per-line
# projection. Adding `--select '.line' --select-raw` to a
# `run --stream --json` invocation must NOT break F16's SIGINT
# forward — the remote `docker logs -f` must still die on Ctrl-C.
timeout --signal=INT 5 inspect run arte --stream --json \
  --select '.line' --select-raw \
  -- "docker logs -f ${SMOKE_CTR}" || true
inspect run arte -- "ps -ef | grep 'docker logs -f ${SMOKE_CTR}' \
  | grep -v grep | wc -l"   # expect 0
# Pass: zero orphaned `docker logs -f` processes on arte.
```

| Gate | Pass criteria |
|---|---|
| F19 round-trip identity | `--select '.'` output equals the unfiltered envelope (no byte mutation). |
| F19 audit show projection | `.data.entry.revert.kind` extracts as one of `command_pair` / `state_snapshot` / `composite` / `unsupported` for a real recorded write. |
| F19 compose ls projection | `.data.compose_projects[].name` with `--select-raw` returns ≥ 1 unquoted project name. |
| F19 history correlation | `inspect history show --audit-id $(... --select-raw)` surfaces the matching transcript block. |
| F19 stream × signal | Adding `--select '.line' --select-raw` to `run --stream --json` does not regress F16's SIGINT forward — zero orphaned remote processes. |

---

## P8 follow-ups — open traps surfaced by the F19 release smoke (must close before v0.1.3 tag)

The first F19 P8 dry-run against arte (2026-05-07) surfaced four
classes of trap. **The release is not pristine until every entry
below flips to ✅.** Each entry carries a repro recipe, the
hypothesis being tested, the current state, and the acceptance
criteria that flip it to ✅. The branch `v0.1.3-jaq` stays open
until all four close; `v0.1.3` is not tagged from a state with any
☐ entry below.

This tracker lives in `SMOKE_v0.1.3.md` rather than the backlog
because each item is reproducible through a P8-class recipe and
benefits from sitting next to the recipes that surfaced it. As
each item closes, this section gets updated in the same commit
that closes it; pre-tag, this section gets a final pass to confirm
all four are ✅.

`inspect` is a tool driven primarily by LLM agents. Every trap
below would silently mislead an agent reading help / running
recipes / correlating audit and transcript output. They are not
optional cleanups. They are the agent-friendliness floor.

### P8-A — Recipe corrections for the SMOKE itself (✅ fixed in this commit)

#### P8-A.1 — Recipe (1) was non-deterministic

The original recipe diff'd two consecutive `inspect status arte
--json` calls; the second hit the F8 cache, producing different
`meta.source.{mode, runtime_age_s}` fields. The diff therefore
*always* reported a difference even when the F19 round-trip was
correct, masking the actual contract under cache-freshness drift.
**The F19 round-trip itself is byte-equal** — keys land in
alphabetical order on both paths (no `preserve_order` feature
enabled on `serde_json`), numbers format identically, and the
serde→jaq→serde pipeline passes through unchanged. The original
recipe was just measuring the wrong thing.

**Status:** ✅ Fixed in this commit. Recipe (1) now diffs two
consecutive `inspect audit ls --limit 5 --json` calls. `audit ls`
is read-only against an immutable on-disk projection; consecutive
calls produce byte-identical output absent intermediate writes.

#### P8-A.2 — Recipe (2) didn't filter to write verbs

The original filter selected the most recent successful non-revert
audit entry. That filter accepts `connect` / `disconnect` /
`setup --discover` entries (all succeed, none are reverts), but
those verbs do not write a `revert` block — so
`.data.entry.revert.kind` yields `null` and `--select-raw`
correctly errors with "filter yielded non-string". The F19
behavior was right; the recipe was wrong. The first arte run hit
this exactly: the most recent successful entry was the `connect`
fired earlier in the session.

**Status:** ✅ Fixed in this commit. Recipe (2) now filters by
`verb == "run"`. P3/P4 always produce at least one successful
`run` audit entry, so the filter resolves deterministically.

### P8-B — `--select` against streaming error frames (☐ open)

**Repro:**
```sh
# Pick a guaranteed-bad target so the verb hits an error frame.
inspect run arte --stream --json --select '.line' --select-raw \
  -- "docker logs -f does-not-exist"
```

**Surfaced:** When the streaming verb's remote command fails to
start (image missing, container missing, permission denied), the
chokepoint emits a fallback envelope that doesn't carry a `.line`
key. `--select '.line'` yields `null`; `--select-raw` rejects null
as non-string with `error: filter --raw: filter yielded non-string`.
An agent tailing real-time logs sees a filter error every time the
verb encounters an error frame, even though the operator's intent
("show me lines") is well-defined.

**Question:** When a streaming verb emits a frame whose schema
doesn't match the operator's filter, should `--select` (a) error
per-frame as today, (b) skip silently, or (c) pass-through the
raw frame? Today's behavior is (a) — the most agent-hostile of
the three for a streaming use case. The jq-language `// empty`
operator is the workaround.

**Acceptance criteria — flips to ✅ when both:**
1. `inspect help select` documents the streaming-error-frame
   pattern explicitly with the worked recipe `--select '.line // empty' --select-raw`,
   landing under a new "common pitfall #7" entry.
2. SMOKE P8 grows a recipe (6) that exercises the `// empty`
   pattern against a real arte streaming verb (e.g.
   `inspect run arte --stream --json --select '.line // empty' --select-raw -- "echo hi"`)
   and validates that both data frames and trailing summary frames
   flow correctly.

### P8-C — F18 transcript correlation gap on `connect` entries (✅ Done — fix landed; awaiting re-smoke validation)

**Repro:**
```sh
LAST_ID=$(inspect audit ls --limit 1 --json --select '.data.entries[0].id' --select-raw)
inspect history show arte --audit-id "$LAST_ID"
# When LAST_ID is a connect entry, returns:
#   SUMMARY: history show: 0 blocks match --audit-id '<id>' against namespace 'arte'
```

**Surfaced:** `inspect audit ls` returns the connect entry
correctly. `inspect history show --audit-id <connect-id>` cannot
cross-reference it back to the F18 transcript and reports zero
matching blocks.

**Hypotheses (need investigation):**
1. F18 deliberately excludes `connect`/`disconnect` from the
   per-namespace transcript (intentional, but undocumented in
   `inspect help safety` / `inspect history show --help`).
2. The connect transcript fence doesn't include `audit_id=…` so
   the `--audit-id` finder has nothing to match.
3. A real F18 contract bug.

**This is not F19's fault** — F19's recipe (4) just happened to
pick a connect id when the seed step failed and no `run` audit
followed. But agents using the audit-id ↔ transcript correlation
pattern will silently lose connect/disconnect blocks if the
exclusion is hypothesis (1) or (2), and that is exactly the
silent-data-loss class the v0.1.3 release ships to close.

**Acceptance criteria — flips to ✅ when:**
1. The actual behavior is identified by reading
   `src/transcript/`, `src/commands/connect.rs`, and
   `src/commands/history.rs` capture sites.
2. Either:
   - (a) the `connect` verb is wired to write a fenced transcript
     block with the correct `audit_id=<id>` footer (matching the
     F18 contract every other namespaced verb already follows), or
   - (b) `inspect help safety` and `inspect history show --help`
     explicitly document the exclusion class so an agent reading
     help knows not to expect a block (and the exit code / SUMMARY
     line on a connect-id miss reflects the documented behavior,
     not "0 blocks match" as if it were a generic miss).
3. P8 grows a recipe that validates the chosen path against a
   real arte session.

**Resolution (2026-05-07).** Path (a) implemented end-to-end.
Diagnosis: hypothesis (1) was the surface trap (connect/disconnect
never called `transcript::set_namespace`, producing no block at
all), and hypothesis (3) was the latent class trap (F13 reauth
audit clobbered the verb's primary `audit_id` in the footer because
`set_audit_id` is first-write-wins and reauth was appended first
on the reauth-then-retry path). Fix landed all three in one commit
per the LLM-trap fix-on-first-surface policy (sweep the class):

- `src/commands/connect.rs` and `src/commands/disconnect.rs` now
  call `transcript::set_namespace` early (covering `--show` /
  `--set-env` / `--unset-env` / master-spawn / unknown-ns paths)
  and emit `verb=connect` / `verb=disconnect` audit entries with
  `revert.kind=unsupported` pointing at the inverse verb.
- `src/safety/audit.rs` factors `AuditStore::append` into a
  transcript-linking variant + a silent
  `append_without_transcript_link` variant; `src/exec/dispatch.rs`
  switches the F13 reauth append to the silent form so the verb's
  primary audit_id wins the F18 footer slot. Reauth audit remains
  on disk + `audit show`-discoverable.

Tests pinning the contract: 1 unit (`safety::audit::tests::
p8c_append_without_transcript_link_persists_but_does_not_link`,
exercises the reauth-then-verb append order that breaks pre-fix)
+ 2 integration (`p8c_connect_show_writes_transcript_block_under_namespace`,
`p8c_disconnect_against_unknown_ns_still_writes_transcript_block`).

Acceptance criterion #3 (P8 recipe against real arte) is the next
P8 dry-run — the existing recipe (4) round-trips
`audit ls → history show --audit-id` and will validate the fix
end-to-end without further changes.

### P8-D — `inspect run --apply` exit 2 + stderr swallow (🟡 stderr-surface fix landed; awaiting re-smoke)

**Repro:**
```sh
inspect run arte --apply -- "docker run -d --name foo --label inspect-smoke=1 nginx:alpine"
# Observed:
#   arte: exit 2
#   SUMMARY: run: 0 ok, 1 failed
#   DATA:    (none)
#   NEXT:    (none)
# The remote stderr is not surfaced anywhere on the operator's terminal.
```

**Surfaced:** During the F19 P8 smoke seed step, both the initial
`docker run -d` and the cleanup `docker rm -f` exited 2 on the
remote. SUMMARY correctly reports "0 ok, 1 failed", but the *why*
of the failure is invisible — no remote stderr surfaced, DATA is
empty. An agent driving `inspect run --apply` against a remote
host has no path from "exit 2" to "what to fix" without a side
channel (`inspect run … --stream` or shell-side ssh).

**This is not F19's fault.** The same recipe shape works in
SMOKE_v0.1.3.md P3 when run earlier in the v0.1.3 cycle. Possible
causes:
1. Remote-side `docker run` is genuinely failing (image pull rate
   limit, name conflict with a stale `inspect-smoke-*` container,
   daemon issue) and inspect's exit-code passthrough is correct —
   but the stderr-swallowing is a real F9/F10 gap that agentic
   callers cannot work around without `--stream`.
2. `inspect run --apply` semantics changed between F11
   (load-bearing safety primitive) and F19 — specifically, whether
   `--apply` is required / optional / forbidden for arbitrary
   commands without a `--revert-preview` companion. The F19 audit
   handoff added `--apply` where P3 uses bare `inspect run`; this
   inconsistency may be the source.
3. A real regression in the run dispatch path landed somewhere in
   the F19 sequence (C1–C4) that smoke testing the chokepoints
   only didn't catch.

**Acceptance criteria — flips to ✅ when:**
1. The repro is reduced to a minimal command (one container, no
   smoke labels, on a fresh sandbox or local docker if convenient).
2. Root cause identified, and:
   - (a) the bug is fixed if it's an inspect regression, OR
   - (b) `LONG_RUN` / `inspect help run` explicitly documents
     when `--apply` is required vs forbidden vs optional, with
     a worked recipe showing each form.
3. The remote stderr question is answered: either `inspect run`
   tees remote stderr to the operator's stderr by default and the
   smoke output should have shown it (regression), or it doesn't
   and the help docs say so + show the opt-in (`--show-output`,
   `--stream`, etc.).
4. P8 seed recipes are updated to the working form before the
   smoke is re-run.

**Progress (2026-05-07):** Criterion #3 closed. The streaming SSH
runner (`run_remote_streaming`) was capturing remote stderr only
for transport-class probing and silently dropping it on a genuine
command failure. A new `command_failure_stderr` helper now decides
whether to surface the captured stderr via `tee_eprintln!` (which
also feeds the F18 transcript): `None` for success, empty stderr,
max-sessions, or transport-classified failures (the latter two are
already surfaced via typed `anyhow::Error`); `Some(trimmed)` for
genuine command failures. 7 new unit tests in
`src/ssh/exec.rs::p8d_tests` pin the contract.

Criteria #1, #2, and #4 still open — once the next P8 dry-run
runs against arte WITH the fix, the surfaced remote stderr will
either:
- diagnose the docker exit 2 immediately (most likely:
  remote-side issue with the container shape, e.g. registry rate
  limit, daemon restart, or a name conflict with a stale
  `inspect-smoke-*` container the cleanup missed); we then update
  the seed recipe to the working form, OR
- show that nothing was wrong with the remote command (less
  likely), in which case we have a real `--apply` semantic bug
  to chase.

This entry flips to ✅ once the next P8 dry-run on arte produces a
clean PASS with surfaced stderr (or the remote-side cause is
documented as "expected").

### Re-running P8 after follow-ups close

Once all four entries above flip to ✅, run P8 end-to-end one more
time against arte. A clean PASS is the gate to ff-merge
`v0.1.3-jaq` into `v0.1.3`. Until then, the branch stays open and
unmerged.

---

## P7 — cleanup + final hygiene

```sh
# Sandbox container + every smoke-labeled side artifact.
inspect run arte --apply -- \
  "docker rm -f \$(docker ps -aq --filter label=inspect-smoke=1) 2>/dev/null; \
   rm -rf /tmp/inspect-smoke-* ~/.inspect-smoke-bin"
inspect run arte -- "docker ps --filter label=inspect-smoke=1 \
  --format '{{.Names}}' | wc -l"   # expect 0
rm -f /tmp/inspect-smoke-*

# Optional: clear the env overlay we set in P5 so the namespace
# returns to its original config.
inspect connect arte --unset-env PATH

# Re-run the local pre-commit gate to confirm nothing diverged.
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test 2>&1 | grep -E "^test result|^running" | tail -40
```

| Gate | Pass criteria |
|---|---|
| Cleanup | Zero `inspect-smoke=1` containers on arte; zero `/tmp/inspect-smoke-*` files locally or remote. |
| Hygiene | Local fmt / clippy / tests still green. |

---

## Pass / fail decision

**PASS** = every gate above green AND every entry in the
"P8 follow-ups" section above is ✅ AND every smoke listed in the
"Release readiness gate" bullets at the bottom of
`INSPECT_v0.1.3_BACKLOG.md` is covered. **Any ☐ in the P8
follow-ups blocks the tag.**

**FAIL** = any single gate red. Two flavours:
- Bug in inspect → fix it, add a regression test in
  `tests/phase_f_v013.rs::<id>_*` named after the smoke that caught
  it, and re-run the failing phase + every later phase. Do not skip
  phases.
- Bug in the smoke artifact / environment → fix the artifact (this
  file, the manifest, or the script) and re-run only the affected
  phase.

Tag `v0.1.3` only after a clean PASS.

---

## Limitations of this smoke (covered by unit / acceptance tests)

These contracts are not exercised end-to-end here because reproducing
them in the field requires environmental knobs the smoke deliberately
avoids. Each is fully covered by the suite under `tests/phase_f_v013.rs`:

- **F13 timeout-driven stale-session** (aggressive `ClientAliveInterval`
  on `sshd`). Requires editing `sshd_config` on the host. The smoke
  exercises the same code path via explicit `inspect disconnect`. Unit
  + acceptance tests in `tests/phase_f_v013.rs::f13_*` cover the
  timeout trigger directly.
- **L4 password-auth path.** Requires a second namespace configured
  for password auth. Covered by `tests/phase_f_v013.rs::l4_*` (13 of
  them). Field-mode is staged only when the operator explicitly
  configures a `legacy-box` namespace before running the smoke.
- **F2 docker-inspect-batched-timeout at scale** (37+ containers).
  Smoke runs against a 10-15 container host. The inventory-scaling
  formula and timeout-budget contract are pinned by 8 unit tests in
  `src/discovery/probes.rs`.
- **F11 every write-verb taxonomy.** Smoke exercises `command_pair`
  (run/restart) and `state_snapshot` (put-over-existing). The
  `composite` and `unsupported` kinds are covered by F17 (composite)
  and F6 compose (unsupported) acceptance tests.
- **L2 keychain in a working DBus session.** Codespace lacks a DBus
  session bus; the smoke confirms the unavailable-backend path
  reports cleanly. The roundtrip path is covered by 19 unit +
  acceptance tests in `src/keychain/` and `tests/phase_f_v013.rs`.

---

## Appendix — artifacts referenced by this runbook

- `tests/smoke/v013/migration.json` — 5-step F17 manifest with
  injected step-3 failure and step 1+2 revert_cmd entries.
- `tests/smoke/v013/migration.sh` — F14 / L11 script-mode payload
  with embedded `sh -c "..."` heredoc, and a Python -c block,
  driving against a sandbox container name passed as `$1`.
- `~/.ssh/inspect_arte_ed25519` — operator-supplied; mode 0600.

The two test artifacts are committed and intended to be reused for
v0.1.4+ smoke runs as new contracts land.
