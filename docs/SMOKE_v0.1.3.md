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

1. **`--json` envelope shape.** Every read verb wraps its payload
   under `.data`. Top-level keys are
   `{schema_version, summary, data, next, meta}`. So the F1 services
   array is at `.data.services`, not `.services`; F7.5 state is at
   `.data.state`, not `.state`; cache provenance is at
   `.meta.source.{mode, stale, runtime_age_s, inventory_age_s}`.
   The `next` field is the structured equivalent of the `NEXT:`
   line in human output.
2. **`--quiet` is mutex with `--json`.** This is the F7.4 contract:
   `--quiet` strips the human-renderer indent; `--json` produces
   structured output that does not need it. Combining them is a
   clap usage error (exit 2). Use `--json` alone when piping to
   `jq`; use `--quiet` alone when piping the human format to a
   non-`jq` filter.
3. **Audit-list output.** `inspect audit ls --tail N --json`
   returns the entries as a JSON array at the **top level** (no
   `.data` wrapper) — this verb predates the L7 envelope sweep.
   Use `.[]`, `.[0]`, etc. directly. Verify the shape on first
   contact in case this differs.
4. **Field selectors are `<ns>/<svc>`** (matches inventory) or
   `<ns>/<container_name>` (F5 dual-axis resolver — emits a
   one-line stderr breadcrumb pointing at the canonical form
   unless `INSPECT_NO_CANONICAL_HINT=1` is set; canonical form
   resolves silently).
5. **Verbs that output an envelope vs raw text.** Read verbs
   (`status`, `health`, `why`, `ls`, `ps`, `connections`,
   `ports`, `images`, `volumes`) emit the envelope. Streaming
   verbs (`logs`, `run`, `cat`, `grep`, `find`, `search`)
   emit raw lines (with redaction applied per L7) — those are
   not jq-able as JSON.
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
inspect status arte --json | jq '{state: .data.state, services_count: (.data.services | length), summary, source_mode: .meta.source.mode}'
inspect connections
inspect connections --json | jq '.data[] | {ns, auth, session_ttl}'
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
inspect why arte/<svc> --json | jq '.services[0] | {recent_logs, effective_command, port_reality}'

# F8 cache freshness
inspect status arte                       # first hit -> SOURCE: live
inspect status arte                       # second hit -> SOURCE: cache (within TTL)
inspect status arte --refresh             # forces SOURCE: live
inspect cache show arte
inspect cache clear arte

# F5 dual-axis selector resolution. Pick a real container whose
# compose service name differs from its docker container name.
inspect status 'arte/<docker-name>'       # canonical hint on stderr
inspect status 'arte/<compose-name>'      # silent (canonical form)

# F7 ports filtering + L9 UDP probe + L10 port-level drift
inspect ports arte
inspect ports arte --proto udp
inspect ports arte --port 53
inspect ports arte --json | jq '.[0] | {port, proto, host, container, declared_by}'

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
inspect cat arte/<svc>:/etc/passwd --lines 1-5 --json | jq '.[] | {n, line}'
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
inspect run arte --json | jq '.audit_id' > /tmp/inspect-smoke-audit.json
inspect revert --last arte
inspect run arte -- "docker ps --filter name=${SMOKE_CTR} --format '{{.Status}}'"
# Pass: container is running again; audit log shows linked entries.
inspect audit show $(jq -r . /tmp/inspect-smoke-audit.json) | head

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
inspect audit ls --tail 5 --json | jq '.[] | {verb, "rk": .revert.kind}'

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
inspect audit ls --tail 1 --json | \
  jq '.[0] | {verb, stdin_bytes, stdin_sha256: .stdin_sha256}'
# Pass: stdin_bytes matches file size.

# F9 loud-failure contract
echo data | inspect run arte --no-stdin -- "cat"   # exit 2 + chained hint

# F14 script mode (heredoc with embedded sh + python; no local quoting)
inspect run arte --file ./tests/smoke/v013/migration.sh -- "${SMOKE_CTR}"
inspect audit ls --tail 1 --json | \
  jq '.[0] | {script_path, script_sha256, script_bytes, script_interp}'
# Pass: script body deduped at ~/.inspect/scripts/<sha>.sh

# F16 streaming + Ctrl-C signal forwarding.
# Run for ~5 seconds then send SIGINT; remote process must die.
timeout --signal=INT 5 inspect run arte --stream \
  -- "docker logs -f ${SMOKE_CTR}" || true
inspect run arte -- "ps -ef | grep -c 'docker logs -f ${SMOKE_CTR}' \
  | grep -v grep || true"
# Pass: zero orphaned 'docker logs -f' processes on arte.
inspect audit ls --tail 1 --json | jq '.[0].streamed'   # true

# L11 --stream + --file composition (lifted clap mutex)
inspect run arte --stream --file ./tests/smoke/v013/migration.sh \
  -- "${SMOKE_CTR}"
inspect audit ls --tail 1 --json | jq '.[0].bidirectional'   # true

# F17 multi-step runner with injected step-3 failure + revert unwind.
# Manifest at tests/smoke/v013/migration.json; step 3 deliberately
# fails (`exit 7`), step 1 + 2 carry revert_cmd entries.
inspect run arte --steps ./tests/smoke/v013/migration.json \
  --revert-on-failure \
  --reason "v0.1.3 smoke F17 unwind probe"
echo "exit=$?"   # non-zero (the failure exit), not 0
inspect audit ls --tail 10 --json | \
  jq '[.[] | select(.steps_run_id != null)] | length'
inspect audit ls --tail 10 --json | \
  jq '[.[] | select(.verb == "run.step.revert")] | length'
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
inspect run arte --no-reauth --json -- "echo blocked" 2>/dev/null \
  | jq '.failure_class'   # "transport_stale"

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
SMOKE_AUDIT=$(jq -r . /tmp/inspect-smoke-audit.json)
inspect history show arte --audit-id "${SMOKE_AUDIT}"
inspect history list arte | head

# L5 audit GC dry-run.
inspect audit gc --keep 1d --dry-run --json | \
  jq '{policy, entries_total, entries_kept, deleted_entries, freed_bytes}'
# Pass: dry-run reports counts but deletes nothing.
```

| Gate | Pass criteria |
|---|---|
| F18 | Transcript exists; opener / closer fences well-formed on every block; `audit_id` on the closer cross-links to the `~/.inspect/audit/<YYYY-MM>-<user>.jsonl` entry by ID. |
| F18 redaction | No Bearer tokens / no `user:pass@` URLs / no PEM bodies present in the file. (Argv `--password=` redaction covered by unit tests; we don't plant a password in the smoke.) |
| L5 | Dry-run reports correct entries-to-delete count; on-disk audit JSONL is byte-identical before vs after the dry-run. |

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

**PASS** = every gate above green AND every smoke listed in
`INSPECT_v0.1.3_BACKLOG.md` lines 1064–1071 covered.

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
