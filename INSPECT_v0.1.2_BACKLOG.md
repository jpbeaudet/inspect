# Inspect CLI — v0.1.2 Patch Backlog

**Rule:** Ship when 10+ items accumulated OR one critical issue found.
**Source:** Field notes, ongoing usage on `arte` namespace.
**Status:** Collecting.

---

## Backlog (10 items)

### B1 — Compound error hint: profile migration + dead SSH agent
**Source:** v0.1.1 field note #15
**Severity:** Medium
**Problem:** Profile migration prompts `inspect setup arte`, which fails with `Permission denied (publickey)` because SSH agent was respawned with no identities. Two-layer failure with no breadcrumb between them. Agents can't reason through this.
**Fix:** When discovery fails with `Permission denied` AND `inspect connections` shows the namespace as `stale` or disconnected, chain the error:
```
error: SSH auth failed for arte. Your session may have expired.
  → run: ssh-add <keyfile>
  → run: inspect connect arte
  → then retry: inspect setup arte
  see: inspect help ssh
```
**Test:** Simulate expired agent, run `inspect setup arte`, verify chained hint appears.

---

### B2 — Selector migration notice in error message
**Source:** v0.1.1 field note #16
**Severity:** Low
**Problem:** v0.1.0 selectors used full container names (`luminary-worker`); v0.1.1 uses discovered short names (`worker`). Error already lists available services (excellent), but doesn't explain *why* the old name stopped working.
**Fix:** When a selector fails to resolve AND the input looks like a full Docker container name (contains `-` prefix matching a known service), append:
```
note: v0.1.1 uses discovered service names. 'luminary-worker' is now 'worker'.
```
No `--legacy-selectors` shim. Just a one-line explanation in the error. Remove the note in v0.3.0 when migration is old news.
**Test:** `inspect logs arte/luminary-worker` shows the note alongside the available services list.

---

### B3 — `--match` with zero results should exit 0, not exit 1
**Source:** v0.1.1 field note #17
**Severity:** Medium
**Problem:** `inspect logs arte/worker --tail 50 --match 'nonexistent'` exits 1 because the remote `grep` returns 1 for no matches. For `inspect grep`, exit 1 on no match is correct (grep convention). For `inspect logs --match`, the filter is narrowing a log view — "no lines matched" is not an error.
**Fix:**
- `inspect grep`: keep exit 1 on no match (grep convention)
- `inspect logs --match`: normalize remote-grep exit 1 to exit 0 with message: `(no matches for 'nonexistent' in 5m window)`
- `inspect logs --follow --match`: no change needed (stream stays open waiting for matches)
- Reserve non-zero exit codes for real failures (connection, parse, auth)
**Test:** `inspect logs arte/worker --match 'xyzzy123'` exits 0 with `(no matches in window)` message.

---

### B4 — Drift check: human-readable diff instead of raw hashes
**Source:** v0.1.1 field note #18
**Severity:** Low
**Problem:** `inspect setup --check-drift` shows raw SHA-256 hashes. User can't tell *what* changed without re-running full discovery.
**Fix:** Compare cached profile against a lightweight live probe (`docker ps` + `ss -tlnp`) and show a summary:
```
Drift detected on arte:
  +2 containers: luminary-foo, luminary-bar
  -1 container: luminary-old
  +1 port: 9090/tcp (new)

Run 'inspect setup arte' to update the profile.
```
Keep the hash in `--json` output for machine consumers. Human output gets the diff summary.
**Test:** Add a container on arte, run `inspect setup --check-drift`, verify human-readable diff.

---

### B5 — CI: bump GitHub Actions off Node 20 before deprecation
**Source:** Release run [#24984573508](https://github.com/jpbeaudet/inspect/actions/runs/24984573508) (v0.1.1 publish, 2026-04-27)
**Severity:** Low (time-bound)
**Problem:** Both `.github/workflows/ci.yml` and `.github/workflows/release.yml` pin Node-20 actions that GitHub is deprecating:
- `actions/checkout@v4`
- `actions/upload-artifact@v4`
- `actions/download-artifact@v4`
- `softprops/action-gh-release@v2`

Timeline (per GitHub annotation):
- **2026-06-02** — runners default to Node 24; some Node-20 actions may misbehave.
- **2026-09-16** — Node 20 removed from runners entirely; pinned actions break.

The opt-in env var `FORCE_JAVASCRIPT_ACTIONS_TO_NODE24=true` is a stopgap, not a fix.
**Fix:** Bump each action to its current Node-24-compatible major:
- `actions/checkout@v5` (or whichever major declares Node 24 support at fix time)
- `actions/upload-artifact@v5`
- `actions/download-artifact@v5`
- `softprops/action-gh-release@v3` (or pin to a SHA that supports Node 24)

Verify by running release on a no-op tag in a fork, or by re-tagging a patch release.
**Test:** Push a `vX.Y.Z-rc1` tag in a fork; release workflow completes without Node deprecation annotations.
**Deadline:** Land before 2026-09-16 to avoid a broken release pipeline.

---

### B6 — `inspec-clifeedback.md` keeps reappearing at `$PWD`
**Source:** Field retrospective — Phase 0 snapshot run on `arte` (heavy destructive workload, 2026-04-27)
**Severity:** Medium (papercut, but recurring — second session in a row)
**Problem:** Some inspect command (suspected `inspect setup arte --force`) re-emits `inspec-clifeedback.md` at `$PWD` even after the user has archived it. Creates spurious git diffs at the workspace root every session and surprises users who didn't ask for the file.
**Fix:** Pick one of:
  - (a) Write to `~/.config/inspect/feedback.md` (or `$XDG_CONFIG_HOME/inspect/`) by default — never touch `$PWD`.
  - (b) Make the write opt-in via `--write-feedback` (or env `INSPECT_WRITE_FEEDBACK=1`).
  - (c) Skip the write if a file with the same name exists anywhere from `$PWD` upward to `$HOME` (treat as "user has already dealt with this").
Recommend (a) as the primary fix, with (b) as the override. (c) alone is fragile.
**Test:**
  - From a clean repo, run `inspect setup <ns> --force` and verify `inspec-clifeedback.md` is **not** created at `$PWD`.
  - Verify the same content lands under `~/.config/inspect/feedback.md` (or wherever the chosen path is).
  - Run twice in a row; confirm no duplicate / no clobber surprise.

---

### B7 — Streaming progress / heartbeat for long-running `exec` ops
**Source:** Field retrospective — Phase 0 snapshot run (`pg_dump` of 4.7M-row table ran ~90s with zero output); previous Knowledge Tab session flagged the same gap for the embed-doc temporal flow
**Severity:** Medium (operator confidence — "is it alive or wedged?")
**Problem:** `inspect exec <ns> -- <long cmd> --apply` produces no output until the remote command finishes. For a 90-second `pg_dump` or a multi-minute `tar` of a volume, the user has no signal that the process is alive. Pre-CLI flow at least streamed stderr; the CLI buffers it.
**Fix:**
  - Stream remote stdout/stderr live by default (don't buffer to completion). Most shell tools already write progress to stderr; just don't swallow it.
  - For commands that genuinely produce no output, emit a periodic heartbeat to the local stderr after N seconds of silence: `[inspect] still running on arte (45s elapsed, pid=12345)`. Configurable via `--heartbeat <secs>` / `--no-heartbeat`; default ~30s.
  - Don't conflate this with the final captured-output record — heartbeats are local-only UX, the audit log still records the real exit + output.
**Test:**
  - `inspect exec <ns> -- 'sleep 60 && echo done' --apply` shows at least one heartbeat line before `done`.
  - `inspect exec <ns> -- 'for i in 1 2 3; do echo step $i; sleep 1; done' --apply` shows each `step N` line within ~1s of the remote echo (not buffered to the end).
  - `--no-heartbeat` suppresses the local heartbeat but does not affect streamed remote output.

---

### B8 — Output truncation indicator is too easy to miss
**Source:** Field retrospective — Phase 0 snapshot run (`\dt+` on full atlas PG cluster truncated mid-row; looked like a complete result)
**Severity:** Medium (correctness risk — a missed truncation can mask a missing row/service)
**Problem:** `inspect run` truncates large outputs by default, which is fine, but the truncation marker is subtle enough that a tired operator reads the partial table as the full answer. The cut also happens mid-row, so the last visible row looks valid.
**Fix:**
  - Make the truncation footer loud and unambiguous, on its own line, e.g.:
    ```
    ── output truncated: 312 of 1840 lines shown (re-run with --no-truncate or | cat) ──
    ```
    Use a distinct color (yellow/bold) when stdout is a TTY; plain text otherwise.
  - Truncate on **line boundaries only** — never mid-row / mid-line.
  - In `--json` mode, set a `truncated: true` field plus `lines_shown` / `lines_total` so machine consumers can detect it.
  - Optionally: when the trailing content looks tabular (header + `─` separator detected), drop the last partial row before the marker rather than show it.
**Test:**
  - Run a query that produces ~5000 lines of tabular output; verify the footer is clearly visible, mentions the line counts, and that no row is cut mid-line.
  - `--json` output for the same command includes `truncated: true` and the counts.
  - `--no-truncate` shows the full output with no footer.

---

### B9 — `inspect bundle run <plan.yaml>` — declarative grouped ops with rollback
**Source:** Field retrospective — Phase 0 snapshot run on `arte`; reaffirmed as a **load-bearing** request after the Phase 0 → Phase 7 atlas centralization migration plan landed (every phase is the same stop → dump → swap → restart → validate → rollback shape).
**Severity:** High — promoted from "future feature" to backlog item by the field user. Every migration phase becomes a yaml file in `ops/migrations/` that gets PR-reviewed, dry-runnable, and replayable. Without it, each phase stays a senior-eng-only checklist gated by tribal knowledge.
**Problem:** Today operators compose multi-step destructive sequences out of ~12+ individual `inspect exec` calls with no atomicity, no rollback, no parallelism, and no replay. One mid-sequence failure means manual recovery from a half-applied state.
**Fix:** Ship `inspect bundle run <bundle.yaml>` (real run) and `inspect bundle plan <bundle.yaml>` (dry-run). YAML describes an ordered list of steps, each a normal `exec` / `run` / verb invocation, with `on_failure: abort | rollback | rollback_to: <step_id> | continue` per step.

Top-level structure (field-validated):
```yaml
name: atlas-phase-0-snapshot
host: arte                  # default namespace for all steps
reason: "Phase 0 pre-flight snapshot"

vars:                       # simple string interpolation only — no logic
  snapshot_dir: /srv/snapshots/2026-04-27
  services:
    clients: [atlas-api, nexus-api, onyx-api]

preflight:                  # gate the whole bundle; fail fast before any step runs
  - check: disk_free
    path: /srv/snapshots
    min_gb: 50
  - check: docker_running
    services: [atlas-pg, aware-milvus, atlas-vault]

steps:
  - id: stop-clients
    exec: docker compose -f /srv/atlas/docker-compose.yml stop {{ services.clients }}
    on_failure: abort

  - id: dump-pg-atlas
    exec: docker exec atlas-pg pg_dumpall -U postgres | gzip > {{ snapshot_dir }}/atlas-pg.sql.gz
    requires: [stop-clients]
    on_failure: { rollback_to: stop-clients }

  - id: tar-volumes
    parallel: true
    matrix:
      volume: [atlas_milvus, atlas_etcd, aware_milvus]
    exec: docker run --rm -v {{ matrix.volume }}:/src -v {{ snapshot_dir }}:/dst alpine tar czf /dst/{{ matrix.volume }}.tar.gz -C /src .

rollback:                   # bundle-level reverse ops; runs on any unhandled failure
  - exec: docker compose -f /srv/atlas/docker-compose.yml start {{ services.clients }}

postflight:                 # only runs on success; failure here is loud but does NOT trigger rollback
  - exec: sha256sum {{ snapshot_dir }}/* > {{ snapshot_dir }}/MANIFEST.sha256
  - check: services_healthy
    services: [atlas-api, nexus-api]
    timeout: 60s
```

Two features inside this that the field user called out as the most important:
1. **`on_failure: rollback_to: <step_id>`** — partial rollback to a known-good checkpoint, not just all-or-nothing.
2. **`parallel: true` with a `matrix:`** — Phase 0's volume-tar step took 4× longer than necessary because it was serialized. Fan-out across a list with bounded concurrency (`max_parallel: N`, default = matrix size, cap maybe 8) is non-negotiable for any real migration.

Other design points:
- **Atomicity.** Best-effort, not true ACID — document this clearly. On failure of step N: run the matching `rollback` block (or `rollback_to:` target) for completed steps in reverse order. If a rollback step itself fails, **stop and surface loudly** rather than silently continuing.
- **Preflight / postflight checks.** First-class `check:` types (not arbitrary exec): `disk_free`, `docker_running`, `services_healthy`, `http_ok`, `sql_returns`. Keeps the surface narrow and lets `inspect bundle plan` validate the *structure* of checks without running them. Arbitrary checks fall back to `exec:` with a non-zero exit gate.
- **Audit trail.** All steps in a bundle share a `bundle_id` in the audit log so `inspect logs --bundle <id>` reconstructs the whole transaction. The cheaper sibling — a `--group <name>` tag on individual `exec` calls — should ship as a precursor (delivers ~30% of the value: replay/grouping in the audit log) before the full bundle engine lands.
- **Apply gate.** `inspect bundle plan` always dry-runs (renders the resolved step list with all `{{ vars }}` substituted). `inspect bundle run --apply` is required for destructive steps. Per-step `apply: false` lets you mix dry-run probes with real ops.
- **Variables / templating.** String interpolation only (`{{ var }}`, `{{ matrix.x }}`). **No conditionals, no loops other than `matrix:`, no functions, no includes** in v1. Operators who need real logic write a shell script that calls `inspect bundle run` with different yamls. Keep the spec narrow.
- **Failure modes to design for:**
  - Partial success where rollback is impossible (e.g., `pg_dump` already wrote to disk — that's fine, it's not destructive). Mark steps `reversible: true | false`; non-reversible steps don't trigger rollback for *later* failures unless explicitly requested.
  - Operator Ctrl-C mid-bundle: prompt "rollback completed steps? [y/N]" with a default-safe answer. `--no-prompt` for CI.
  - Heartbeat (see B7) per step, plus a top-level `step 3/12: dump-pg-atlas (running, 45s)` progress line. For `parallel: true` steps, show one line per matrix entry.
  - Partial-failure inside a parallel matrix: by default, abort the bundle on first matrix-entry failure and roll back; opt-in `matrix.on_failure: continue` for steps where partial completion is acceptable.
- **Inline waits.** `wait_until_*` steps inside a bundle delegate to the B10 (`inspect watch`) engine — same predicate code path, same audit format. B9 and B10 must be designed together.

**Open questions:**
1. YAML for bundles is the right call (operator muscle memory: Ansible/Compose), even though inspect's other config is TOML. Confirm.
2. Should bundles be named/versioned and stored in a per-namespace registry (`inspect bundle list arte`), or pure file-path invocation? Start with file-path; promote to registry only if demand appears.
3. Interaction with k8s (F1, future): a bundle of mixed Docker+k8s steps is appealing but multiplies failure surface. v1 = single-medium bundles; cross-medium deferred.
4. Should `--reason` be required at the bundle level? Lean yes; bundle-level `reason:` propagates to every step's audit entry.

**Test:**
- `inspect bundle plan phase-0-snapshot.bundle.yaml` renders all steps with vars resolved, runs preflight checks, and exits 0 without touching the remote.
- `inspect bundle run` with an injected mid-step failure runs the correct `rollback_to:` target in reverse order; audit log shows a single `bundle_id` covering all entries.
- `parallel: true` with a 4-entry matrix runs concurrently (verify by timing) and respects `max_parallel`.
- Ctrl-C mid-bundle prompts and cleanly rolls back; `--no-prompt` rolls back without asking.
- `inspect bundle run --apply` is required for any step missing `apply: false`; without `--apply` the run aborts with a clear gate message.

**Next step:** write `INSPECT_v0.2.0_BUNDLE_SPEC.md` (designed alongside B10). Ship `--group <name>` audit tag in a v0.1.x patch as a cheap precursor.

---

### B10 — `inspect watch <target> --until <predicate>` — synchronous wait for a remote condition
**Source:** Field retrospective — every "did the service come back?" / "is the queue drained?" / "are migrations done?" check across Phases 1–7 of the atlas migration is currently a `for i in {1..30}; do inspect run ... | grep ...; sleep 2; done` shell loop. Also flagged as the missing primitive that bundles (B9) need internally.
**Severity:** High — promoted from "future feature" to backlog item alongside B9. The field user explicitly called out that B9 is incomplete without B10 because every bundle needs a `wait_until_healthy` step and would otherwise re-create polling glue from scratch. Also retroactively closes the streaming-progress gap (B7) for the common "did this big op finish?" case.
**Problem:** There is no synchronous primitive for "wait until a remote condition becomes true". Operators write fragile shell loops with `inspect run | grep && sleep`, which has no timeout, no audit trail, and inconsistent exit semantics.
**Fix:** Ship `inspect watch <target> --until-<kind> <predicate> [--interval <dur>] [--timeout <dur>]`. Exit 0 = predicate met; exit 124 = timeout (matching `timeout(1)` convention so callers can distinguish timeout from other errors). Composes with `&&` for sequencing without changing existing `exec` / `run` semantics.

Predicate kinds (field-validated, all four are needed):
```bash
# 1. Wait for a log line to appear (tail -F + match)
inspect watch arte/atlas-api \
  --until-log 'Started server on 0.0.0.0:8000' \
  --timeout 60s

# 2. Wait for a SQL predicate to become true
inspect watch arte/atlas-pg \
  --until-sql "SELECT count(*) = 0 FROM pg_stat_activity WHERE state = 'active' AND query LIKE 'COPY%'" \
  --interval 2s --timeout 5m

# 3. Wait for an HTTP probe + JSONPath/jq predicate
inspect watch arte/onyx-api \
  --until-http https://localhost:8080/health '$.status == "ok"' \
  --timeout 90s

# 4. Wait for an arbitrary remote command's stdout to satisfy a comparison
inspect watch arte/temporal \
  --until-cmd 'temporal workflow list --query "WorkflowType=\"embed-doc\" AND ExecutionStatus=\"Running\"" | wc -l' \
  --equals 0 --timeout 10m
```

Other design points:
- **Comparators for `--until-cmd`:** `--equals <v>`, `--matches <regex>`, `--gt <n>`, `--lt <n>`, `--changes` (any change from baseline), `--stable-for <dur>` (output unchanged for N seconds — useful for "pipeline drained").
- **Output / UX.**
  - TTY: live single-line status (`waiting on arte/atlas-api: log match (12s elapsed, last poll +0.4s)`); replace in place.
  - Non-TTY / `--verbose`: one line per poll with timestamp and the value being compared.
  - On success: print the matching value / log line / SQL row to stdout (so `$(inspect watch ...)` is useful in scripts).
  - On timeout: print the last observed value plus a clear `timeout after Ns` line on stderr.
- **Polling vs. streaming.** Prefer streaming where the underlying primitive supports it (`docker logs -f` for `--until-log`, long-poll where available). Fall back to interval polling for SQL / HTTP / cmd. Default `--interval`: 2s for cmd/sql, immediate-stream for log.
- **Safety / cost.** `--until-sql` and `--until-cmd` run repeatedly on the remote — surface the per-poll cost in `--verbose`. Cap default `--timeout` at 10m to prevent runaway watches; require explicit `--timeout 0` (or `--timeout <larger>`) to disable.
- **Audit trail.** A successful `watch` records `target`, predicate, elapsed time, and the matching value. A timeout records the last observed value and the predicate. Cheap; small log entries.
- **Bundle integration (with B9).** Bundles get a first-class step type that delegates to the watch engine — no shelling out:
  ```yaml
  - id: wait-api-up
    watch: arte/atlas-api
    until_log: 'Started server'
    timeout: 60s
  ```

**Open questions:**
1. `--until-http` predicate language — JSONPath, jq, or a small DSL (`$.status == "ok"`)? jq is the operator default but adds a runtime dep; ship a tiny built-in expression evaluator for the common cases (`==`, `!=`, `<`, `>`, `contains`).
2. Should `--until-log` accept a regex by default or require an explicit flag? Lean: literal by default, `--regex` to opt in (matches `inspect logs --match` v0.1.1 behavior — be consistent).
3. Stacked predicates (`--until-log A AND --until-cmd B`) — out of scope for v1; chain `inspect watch && inspect watch` instead.

**Test:**
- `inspect watch <ns>/<svc> --until-log 'ready' --timeout 5s` against a service that emits the line in <1s exits 0 and prints the line.
- Same against a silent service exits 124 within ~5s with `timeout after 5s` on stderr.
- `--until-cmd 'echo 0' --equals 0` exits 0 on first poll.
- `--until-sql` re-polls at the configured interval; verify with a counter table that flips after N seconds.
- Audit log entry includes target, predicate, elapsed, and matching value (or last observed on timeout).

**Next step:** spec `inspect watch` as a sibling of `inspect exec` / `inspect run` in `INSPECT_v0.2.0_WATCH_SPEC.md`. Design alongside B9 since bundles consume the same predicate engine.

---

## Future Features (not patch material)

These are larger directional bets that don't fit the "small patch" backlog
rule. Track here so they don't get lost; promote to a real spec doc when
prioritized.

### F1 — Kubernetes support (`inspect` for k8s)
**Source:** Field demand from the 15+ service migration (DB, Redis, Milvus, Neo4j, …); Docker-only is starting to feel like a constraint.
**Status:** Idea / not scoped.
**Sketch:**

- **Medium abstraction.** Today `Medium` is implicitly Docker-over-SSH. Generalize to a trait with `Docker` and `Kubernetes` implementations. Selector becomes `cluster/namespace/workload[/container]` for k8s while staying `host/service` for Docker.
- **Discovery.** Replace `docker ps` probes with `kubectl get pods -o json` (or the k8s API directly via `kube-rs`) plus Service/Endpoint/Ingress probes. Profile cache extends with workload kind, replicas, container list per pod, and label selectors.
- **Verbs that map cleanly:**
  - `logs <sel>` → `kubectl logs` (with `-c <container>` when ambiguous), `--follow`, `--tail`, `--since` all carry over. `--merged` becomes a natural fit: interleave logs across all replicas of a Deployment.
  - `exec <sel> -- <cmd>` → `kubectl exec`. `--no-tty` and the audit gate stay identical.
  - `run <sel> -- <cmd>` → `kubectl run` ephemeral pod, or `kubectl debug` against an existing pod (requires k8s ≥ 1.25).
  - `connectivity` → probe Services + Ingresses, not just host ports.
  - `why <sel>` → roll up Pod conditions, last `kubectl describe`, recent Events, container restart counts. Likely the highest-value k8s verb.
- **Verbs that need rethinking:**
  - `cp`, `edit`, `chmod`, `chown` — pods are immutable; redirect to ConfigMap/Secret edits + a `kubectl rollout restart`. Probably refuse with a chained hint in v1 rather than silently doing the wrong thing.
  - `setup` — replace SSH connectivity probe with kubeconfig context check + RBAC self-test (`kubectl auth can-i ...`).
- **Auth.** Honor the user's existing kubeconfig + current-context. No new credential surface. Optional `--context` / `--kubeconfig` flags mirror `kubectl`.
- **Safety.** Audit log entries gain `cluster` + `namespace` fields. Explicit-mode gate (P8) extends to multi-replica fan-out: confirm before `exec`-ing into all replicas of a Deployment.
- **Distribution.** Separate binary (`inspect-k8s`) or feature flag (`--features k8s`) — TBD; depends on `kube-rs` compile time and binary size.

**Open questions:**
1. Single binary or split? `kube-rs` is heavy; static-musl builds may bloat.
2. Stream multiplexing for `logs --merged` across 50 pod replicas — channel-based fan-in scales, but does `kubectl logs -f` rate-limit?
3. CRDs (Argo Rollouts, Knative, etc.) — declare out of scope for v1, or expose a generic `inspect get <gvk>/<name>` escape hatch?
4. Helm/Kustomize awareness — useful for `why` but explodes the dependency surface.

**Next step when promoted:** write `INSPECT_v0.2.0_K8S_SPEC.md` mirroring the structure of `archives/INSPECT_v0.1.1_PATCH_SPEC.md`, with one section per verb and a migration matrix from Docker selectors to k8s selectors.

---

## Shipped
*(move items here when released)*

---

## Running total: 10 / 10 — **READY TO SHIP** (B9 + B10 are load-bearing; B6/B7/B8 ride along)
