# Inspect CLI — v0.1.3 Patch Backlog

**Rule:** Ship when 7+ items accumulated OR one critical issue found.
**Source:** Known limitations from pre-v0.2.0 roadmap + retrospective items surfaced during v0.1.2 bundle implementation + field feedback from two independent v0.1.2 users (Keycloak / Vault debug sessions on multi-container hosts).
**Status:** **OPEN — committed to ship all 14 items.** v0.1.3 is the last "break things freely" release on the docker-only surface; **v0.1.4 is dedicated to Kubernetes** (medium = k8s, kubectl-equivalent verbs, k8s-aware selectors). The stabilization sweep that was previously planned for v0.1.4 shifts to a v0.1.5 stabilization release before the v0.2.0 contract. Because v0.1.4 will be heads-down on k8s, **anything docker / compose / SSH related that doesn't ship in v0.1.3 will not be touched again until v0.1.5 at the earliest** — that's the reason to clear the entire v0.1.3 backlog rather than slipping items.
**Contract:** No backward compatibility. Break whatever needs breaking. After v0.2.0 the CLI surface, JSON schema, and config formats are frozen.

| Item | Status | Notes |
|---|---|---|
| F1 — `inspect status` returns 0 services after `--force` discovery (regression) | ⬜ Open | **critical regression**, ship-blocker; **independently confirmed by 2nd field user** on a 37-container Keycloak host |
| F2 — `docker inspect` batched-timeout warning noise during setup | ⬜ Open | small, cosmetic but erodes trust on first run; 2nd field user hit the 10s default warning every setup |
| F3 — `inspect help <command>` not a `--help` synonym | ⬜ Open | small, ergonomics; carry-over from v0.1.2 backlog |
| F4 — `inspect why` compose-aware deep-diagnostic bundle (logs + effective Cmd + port reality) | ⬜ Open | **load-bearing field request**; turns 15-minute manual triage into 30 seconds |
| F5 — Container-name vs compose-service-name uniform resolution | ⬜ Open | small/medium; both `arte/onyx-vault` and `arte/luminary-onyx-onyx-vault-1` should resolve, or the error must point at the canonical form |
| F6 — First-class `inspect compose` verbs | ⬜ Open | medium-large; replaces ~80% of field `inspect run -- 'docker compose …'` usage; scoped read-only + restart in v0.1.3 |
| F7 — Selector / output ergonomic papercuts (4 small fixes bundled) | ⬜ Open | small; pre-setup error hint, `arte:` shorthand suggestion, `inspect ports --port`, `--quiet` trailer suppression |
| L4 — Password auth + session TTL + `ssh add-key` helper | ⬜ Open | small, unblocks legacy servers; bundles the key-migration helper |
| L2 — OS keychain integration (opt-in, cross-session only) | ⬜ Open | small, one crate (`keyring`); default stays ssh-agent / per-session |
| L5 — Audit log rotation / retention | ⬜ Open | small, maintenance hygiene |
| L7 — Header / PEM / URL credential redaction | ⬜ Open | medium, security hardening |
| L3 — Parameterized aliases | ⬜ Open | medium, parser change |
| L6 — Per-branch rollback in bundle matrix | ⬜ Open | medium, architectural — touches bundle executor |
| L1 — TUI mode (`inspect tui`) | ⬜ Open | largest, ships last |

**Implementation order:** F1 → F2 → F3 → F4 → F5 → F7 → F6 → L4 → L2 → L5 → L7 → L3 → L6 → L1. Field-reported regressions first (F1 is a ship-blocker on its own under the "one critical issue" rule). F4 is the load-bearing field request and ships ahead of all L-items because it amplifies every existing diagnostic verb. F5 + F7 are small ergonomic wins that ride along with F4 (same code paths). F6 (compose verbs) is the larger surface and lands after the diagnostic improvements so compose-aware `why` already exists when compose verbs ship. Then the planned L-items in their existing order. TUI last when everything underneath is stable.

---

## Backlog (14 items)

### F1 — `inspect status <ns>` returns 0 services after `--force` discovery (regression)
**Source:** v0.1.2 field feedback (first reported on a 38-container host; **independently re-confirmed by 2nd field user** debugging a Keycloak deployment on a 37-container host — `inspect setup --force` showed the full inventory, then `inspect status arte` immediately returned `0 services`. `inspect health 'arte/onyx-*'` worked fine on the same data, which made `status` "feel like dead weight").
**Severity:** **Critical** (ship-blocker on its own — qualifies v0.1.3 for release under the "one critical issue" clause). Two-of-two independent field users hit this on the same code path within the first minute of use; the status verb is the first thing operators run after `connect`, and returning 0 services on a healthy 30+-container host destroys trust in the tool.
**Problem:** `inspect status arte` reports `0 services` even after `inspect status arte --force` (which is supposed to bypass the discovery cache and re-scan from scratch). The discovery probe clearly *did* run during v0.1.2 setup (the rest of the verbs work — `logs`, `run`, `search` all see services), so the regression is in either:
  (a) the cache key used by `status` vs the cache key written by `--force` discovery (status reads a stale / wrong-keyed entry), or
  (b) the post-discovery aggregation that builds the `status` view (services normalization landed in v0.1.2 to fix #1 / #3 phantom selectors — likely the same code path now drops everything when normalization can't resolve a row).
**Fix:**
- **Reproduce first.** Add `tests/phase_f_v013.rs::status_after_force_discovery_returns_services` covering: cold cache → `inspect status <ns> --force` → assert `services_count > 0` against a mock medium with N≥10 containers. This test must fail today before any code change.
- **Bisect** between v0.1.1 (where status worked) and v0.1.2 (where it doesn't) on the discovery / status code paths only — the v0.1.2 services-normalization commit is the prime suspect.
- **Fix at the layer the bisect points at**, not above:
  - If (a) — make `--force` invalidate **and rewrite** the same cache entry that `status` reads. Single source of truth for the cache key (`src/discovery/engine.rs`). Add a debug-assert that the key written by discovery equals the key read by status.
  - If (b) — the normalization step must never drop a row silently. Unresolvable rows fall through with `service: <raw_name>, normalized: false` and are still counted in `status`. Log a single warning summarizing how many rows fell through, never per-row spam.
- `inspect status <ns> --force --debug` prints the cache path being read, entry count before/after force, and any rows dropped during normalization — so the next regression of this shape is diagnosed in one command.
- Audit-log the `--force` invocation (`verb=status.force`) so we can see in the field how often operators reach for it.

**Test:**
- `tests/phase_f_v013.rs::status_after_force_discovery_returns_services` — passes after the fix, fails before.
- Regression guard: same test against a 1-container, 10-container, and 50-container mock medium; all return the expected count.
- Normalization fall-through: a container whose name does not match the v0.1.2 normalization rules still appears in `status` output with `normalized: false` and contributes to the total count.
- `--force --debug` output includes cache path, pre/post entry count, and any drop summary.
- v0.1.2 phantom-selector fix is **not regressed** (re-run `tests/phase_b_v011.rs` and the v0.1.2 services-normalization tests).

---

### F2 — `docker inspect` batched-timeout warning noise during setup
**Source:** v0.1.2 field feedback (independently re-reported by 2nd field user — "appeared as a warning every single `setup` run" on a 37-container Keycloak host with the 10s default).
**Severity:** Low (cosmetic, but it is the *first* output a new user sees on `inspect setup` / first discovery — and a scary warning on first run erodes trust before the tool has done anything useful). Two-of-two field users hitting it on first run elevates it from "rare papercut" to "guaranteed first-impression bug".
**Problem:** During setup / first discovery, the batched `docker inspect` probe emits a timeout warning that (a) appears even on healthy hosts, (b) does not actually indicate a failure (discovery succeeds, services are populated, the warning is just noise), and (c) uses a fixed 10s default that is too aggressive for hosts with 30+ containers. Today's behavior trains users to ignore warnings, which is exactly the wrong reflex.
**Fix:**
- **Decide what the warning means** before changing anything. Audit `src/discovery/probes.rs` (the docker-inspect batch path) and classify the timeout into one of three buckets:
  1. **Genuine failure** — batch returned partial / no data. Surface as an `error` with a concrete chained hint, never as a `warning`.
  2. **Slow but successful** — batch took longer than the soft threshold but returned complete data. **Demote to debug-level only**, visible under `--debug` / `RUST_LOG=debug`. Default output stays clean.
  3. **Per-container timeout in a parallel batch** — surface once at the end of discovery as `warning: docker inspect timed out for N/M containers; rerun with --force or check daemon load` (single line, with counts), never per-container.
- **Scale the timeout with inventory size**, as the 2nd field user explicitly suggested. New formula: `timeout = max(10s, 250ms * container_count)` — i.e. 10s floor for tiny hosts, ~10s for 40 containers, ~25s for 100. Cap at 60s to keep failure cases bounded. Configurable via `discovery.docker_inspect_timeout = "30s"` in `~/.inspect/config.toml` for operators who want to override.
- Add an integration test that runs first discovery against a healthy mock medium and asserts **no `warning:` lines** on stderr. That test is the contract going forward.
- Document the three-bucket classification + the scaling formula in `docs/RUNBOOK.md` so the next probe author follows the same rule.

**Test:**
- Healthy mock medium with 37 containers, first discovery: stderr contains zero `warning:` lines (`tests/phase_f_v013.rs::no_spurious_docker_inspect_warning_at_field_scale`).
- Inject a single slow container (sleep 10s in the inspect path) under the scaled threshold for a 37-container host: still zero warnings, debug log shows the slow entry.
- Inject a genuine failure (kill the docker socket mid-batch): error surfaces with a chained hint, exit code non-zero, **not** a warning.
- Inject 3 of 50 per-container timeouts: exactly one summary warning line at end of discovery, format matches the spec above.
- Config override `discovery.docker_inspect_timeout = "5s"` is honored and bypasses the scaling formula.

---

### F3 — `inspect help <command>` as `--help` synonym
**Source:** v0.1.2 field feedback (still open from prior backlogs — carried forward because operators keep hitting it).
**Severity:** Low (ergonomics), but it is one of the first reflexes users have (`git help log`, `cargo help build`, `kubectl help get` all work) and its absence is jarring.
**Problem:** `inspect help <command>` does not behave as a synonym for `inspect <command> --help`. Today it either errors or drops back to the top-level help, depending on the verb. Operators who type `inspect help logs` expect the `inspect logs --help` page.
**Fix:**
- In `src/commands/help.rs` (and the top-level dispatcher in `src/main.rs` / `src/cli.rs`), when the parsed argv is `inspect help <token>`:
  - If `<token>` is a known verb → dispatch to that verb's `--help` rendering exactly as `inspect <token> --help` would (same output, byte-for-byte; share the rendering function).
  - If `<token>` is a known topic (`ssh`, `bundle`, `selectors`, etc.) → render the topic page.
  - If `<token>` is unknown → exit 2 with `error: unknown command or topic: <token>` and a chained hint pointing at `inspect help` (top-level list). Never silently fall back to the top-level help — that is exactly the bug.
- Same treatment for the bare `inspect help` form (already works; keep it).
- Add a `tests/help_contract.rs` case asserting `inspect help <verb>` and `inspect <verb> --help` produce **identical stdout** for every registered verb. This is the contract that prevents future drift.
- No change to the JSON help snapshot (`tests/help_json_snapshot.rs`) other than adding the synonym path to its coverage.

**Test:**
- For every verb in the registry: `inspect help <verb>` stdout == `inspect <verb> --help` stdout (byte-for-byte).
- `inspect help selectors` renders the selectors topic page (matches existing topic rendering).
- `inspect help nonsense-verb` exits 2 with the unknown-command error and a hint to `inspect help`. Does **not** print top-level help on stdout.
- Bare `inspect help` is unchanged.

---

### F4 — `inspect why` compose-aware deep-diagnostic bundle
**Source:** v0.1.2 field feedback (2nd field user, "the one load-bearing feature request"). Real-world session: Vault failing with `bind: address already in use`, `inspect why arte/onyx-vault` correctly identified the service as down but stopped at "down — likely root cause" and pointed the operator at `inspect logs`. The operator then spent 15 minutes manually assembling logs + effective `Cmd` + port reality across `inspect run`, `inspect logs`, `inspect ports` to find a duplicate listener bug (docker-entrypoint injecting `-dev-listen-address` even with `-config`). With this bundle, the same diagnosis would have been ~30 seconds.
**Severity:** **High value, medium effort.** This is the single highest-leverage change in v0.1.3 because it amplifies the value of every existing diagnostic verb without adding a new top-level surface. It is the verb operators reach for first when something is broken, and today it stops one level too shallow.
**Problem:** `inspect why <selector>` reports up/down + a single likely-root-cause line + a `NEXT:` hint. That is correct but shallow for any unhealthy/exited container — the operator now manually runs three more commands (`logs`, `run -- 'docker inspect'`, `ports`) to reconstruct the actual failure context. The information is already on the host; the verb just isn't gathering it.
**Fix:** For any selector that resolves to an **unhealthy, exited, or restart-looping** container, `inspect why` automatically attaches three diagnostic artifacts inline, in this exact order, under the existing `DATA` block:

1. **Recent logs** — last 20 lines of `docker logs <container>` (configurable via `--log-tail <n>`, capped at 200). Streamed through the existing redaction pipeline (L7 once shipped). Skipped silently if logs are empty.
2. **Effective `Cmd` + `Entrypoint`** — the `Cmd` and `Entrypoint` fields from `docker inspect`, **plus** the resolved entrypoint script if it is a known `docker-entrypoint.sh`-style wrapper (read the first 50 lines of the script to surface flag injection like Vault's `-dev-listen-address`). Display as: `effective command: <entrypoint> <cmd>` and, if a wrapper is detected, `wrapper injects: -dev-listen-address=...` on a separate line.
3. **Port reality vs declared** — for each port the container declares (`HostConfig.PortBindings` + `Config.ExposedPorts`), check three things on the host: (a) is the host port free or bound? (b) is the container port bound inside the container's netns? (c) does the declared config bind the same port more than once? Render as a small table:
   ```
   port    host        container       declared
   8200    free        bound (twice!)  config + entrypoint -dev-listen
   8201    bound→pid…  free            config
   ```
   The "bound (twice!)" detection is the headline diagnostic — it short-circuits "is it the network, the config, or the entrypoint?" for the dominant class of port-conflict failures on shared dev hosts.

Design points:
- New flag `--no-bundle` to suppress the three artifacts and restore today's terse output (for agents that already drive the deeper queries themselves).
- New flag `--log-tail <n>` (default 20, max 200).
- For **healthy** services, `why` output is unchanged (no bundle, no extra round-trips, no perf regression on the happy path).
- The bundle never fires more than 4 extra remote commands (logs + inspect + entrypoint cat + port probe). Hard cap, audited.
- All three artifacts go under `DATA` with explicit subsection headers (`logs:`, `effective_command:`, `ports:`) so the existing `SUMMARY` / `DATA` / `NEXT` discipline holds.
- `--json` output adds three structured fields: `recent_logs: []`, `effective_command: { entrypoint, cmd, wrapper_injects }`, `port_reality: [{ port, host, container, declared_by }]`.
- The `NEXT:` block becomes smarter: if "bound twice" is detected, the suggestion is `inspect run <ns>/<svc> -- 'cat /entrypoint.sh'` or the equivalent for the detected wrapper. If logs contain `address already in use`, suggestion is `inspect ports <ns> --port <p>` (uses the F7 structured filter once that lands).
- Compose-aware naming: works for both `arte/onyx-vault` (compose service) and `arte/luminary-onyx-onyx-vault-1` (docker container) — relies on F5's uniform resolution.
- Implementation lives in `src/commands/why.rs`; the three artifact gatherers go under `src/commands/why/bundle/{logs.rs,command.rs,ports.rs}` so each is unit-testable in isolation against a mock docker-inspect / log fixture.

**Test:**
- Mock medium with a Vault-style container: exited (1), restart_count=4, logs containing `bind: address already in use`, entrypoint script that injects `-dev-listen-address=0.0.0.0:8200`, declared port 8200 in compose config — `inspect why arte/onyx-vault` output includes all three sections, the "bound (twice!)" detection fires, and `NEXT:` suggests inspecting the entrypoint.
- Healthy service: `inspect why arte/pulse` output is byte-for-byte identical to v0.1.2 (no bundle, no extra commands fired — assert remote-command counter is unchanged).
- `--no-bundle` on an unhealthy service produces v0.1.2-style terse output.
- `--log-tail 50` returns 50 lines; `--log-tail 500` is clamped to 200 with a one-line notice.
- `--json` output schema includes the three new fields with non-null values for unhealthy services and empty defaults for healthy ones.
- Hard cap test: an unhealthy service produces ≤ 4 extra remote commands (counted via mock medium's command log).
- Resolution test: `inspect why arte/luminary-onyx-onyx-vault-1` and `inspect why arte/onyx-vault` both produce the same bundle (depends on F5).

---

### F5 — Container-name vs compose-service-name uniform resolution
**Source:** v0.1.2 field feedback (2nd field user). The operator tried `arte/luminary-onyx-onyx-vault-1` (the docker container name from `docker ps`), got an error, then `arte/onyx-vault` (the compose service name) worked. Both forms appear in the discovered inventory, so the failure is surprising.
**Severity:** Small/medium (every compose user hits this once; trust cost on first encounter).
**Problem:** A single container has at least two valid identifiers in the discovered inventory: the docker-assigned container name (`<project>-<service>-<index>`, e.g. `luminary-onyx-onyx-vault-1`) and the compose service name (`onyx-vault`). The selector resolver currently accepts only one of them depending on the verb / code path, and the error on the rejected form is generic ("no targets" or "invalid selector") with no hint at the canonical form.
**Fix:**
- **Single resolution pass** in `src/selector/`: every container selector resolves through one function that tries, in order: (1) exact match against compose `service` name, (2) exact match against docker container name, (3) glob match against either. The function returns the canonical form (compose service name, when available) plus a list of aliases.
- All verbs (`why`, `logs`, `run`, `ports`, `cat`, `exec`, `health`) use that function. No verb-local resolution paths.
- When the rejected form is a known docker container name but the canonical is the compose service: error becomes `error: 'arte/luminary-onyx-onyx-vault-1' is the docker container name; the canonical selector is 'arte/onyx-vault' (try that, or use the docker name with --by-container)`.
- New flag `--by-container` (and config option `selector.prefer = "container" | "service"`, default `"service"`) for operators who explicitly want docker-container-name semantics (e.g. when two compose services map to the same image and they want the specific instance).
- `inspect status <ns>` and `inspect health <selector>` JSON output gains an `aliases: ["luminary-onyx-onyx-vault-1"]` field per service so agents discover the equivalence without trial-and-error.

**Test:**
- `inspect why arte/onyx-vault` and `inspect why arte/luminary-onyx-onyx-vault-1` both succeed and target the same container.
- Without the disambiguation flag, the docker-container form prints the canonical-form hint as part of stderr but still resolves (warning, not error). With `selector.prefer = "service"` (default), this warning fires; with `selector.prefer = "container"`, it does not.
- Two compose services running the same image: `--by-container` resolves to the specific docker instance; default service-name resolution returns the compose service.
- `inspect status arte --json` output includes the `aliases` field for every service that has a docker-container-name distinct from its compose-service name.
- Glob form: `arte/onyx-*` and `arte/luminary-onyx-onyx-*-1` both work; the result set is identical when each compose service has exactly one container.

---

### F6 — First-class `inspect compose` verbs
**Source:** v0.1.2 field feedback (2nd field user, "no obvious `inspect compose` integration … first-class compose verbs would replace 80% of my `run` usage"). Field operator was repeatedly running `inspect run arte -- 'cd /opt/luminary-onyx && sudo docker compose …'` for ps / logs / restart / config. Compose is the dominant deployment shape on the field servers Inspect targets today.
**Severity:** Medium-large (new verb surface, but each sub-verb is a thin wrapper around an existing remote `docker compose` invocation — risk is in scoping, not in implementation complexity).
**Problem:** Compose projects are a first-class deployment unit on the hosts Inspect targets, but Inspect treats them as opaque collections of containers. To inspect a compose project's effective config, restart a single service, or view aggregated compose logs, operators drop back to `inspect run <ns> -- 'cd <project_dir> && sudo docker compose …'` — losing structured output, audit trail, redaction, and selector grammar.
**Fix:** Ship a small, **read-mostly** `inspect compose` subcommand surface in v0.1.3. Write verbs are limited to `restart` (the safest, most-needed action). `up` / `down` / `pull` are deferred to **v0.1.5+** pending a compose-write design review (v0.1.4 is the k8s release and will not touch compose).

Sub-verbs in v0.1.3:
- `inspect compose ls <ns>` — list compose projects discovered on the namespace, with `name`, `working_dir`, `service_count`, `running_count`, `compose_file`. Replaces `docker compose ls`.
- `inspect compose ps <ns>/<project>` — per-service status table for one project (state, ports, image, uptime). Replaces `docker compose ps`.
- `inspect compose config <ns>/<project>` — effective merged compose config (resolved variables, profiles applied). Replaces `docker compose config`. Streamed through redaction.
- `inspect compose logs <ns>/<project>[/<service>]` — aggregated logs for a project (or one service inside it). Wraps `docker compose logs` with the existing `--tail` / `--follow` / `--since` flags from `inspect logs`.
- `inspect compose restart <ns>/<project>/<service>` — restart a single service. **Audited** like any write verb (`verb=compose.restart`), respects `--dry-run`. Aborts if more than one service is targeted unless `--all` is passed (defensive default).

Design points:
- Compose project discovery extends `src/discovery/probes.rs` to find compose projects via `docker compose ls --format json`. Cached alongside the existing container inventory; surfaces in `inspect status <ns>` as a new `compose_projects:` line.
- Project paths resolve against the discovered `working_dir`; operators never type the path. Selector form is `<ns>/<project>` for the project and `<ns>/<project>/<service>` for a service inside it. The existing `<ns>/<service>` form continues to work because F5's resolver tries compose-service first.
- All sub-verbs share the existing `SUMMARY` / `DATA` / `NEXT` output discipline and `--json` schema.
- **Out of scope for v0.1.3:** `up`, `down`, `pull`, `build`, `exec`. Those introduce compose-state-mutation semantics that need their own design pass; deferred to the **v0.1.5 backlog** with a placeholder (v0.1.4 is k8s-only).
- Audit log: every `compose restart` writes a structured entry with `project`, `service`, `compose_file_hash` (so the post-mortem can verify the file didn't change between audit and rerun).
- Help: `inspect help compose` lists the sub-verbs and explicitly notes which compose actions are intentionally not yet exposed and why.

**Test:**
- Mock medium with two compose projects: `inspect compose ls arte` lists both with correct service counts.
- `inspect compose ps arte/luminary-onyx` returns the per-service table; `--json` schema matches the spec.
- `inspect compose config arte/luminary-onyx` produces the merged YAML and runs through the redaction pipeline (L7-aware once that lands; today the existing env-var masker applies).
- `inspect compose logs arte/luminary-onyx` aggregates; `inspect compose logs arte/luminary-onyx/onyx-vault` narrows to one service.
- `inspect compose restart arte/luminary-onyx/onyx-vault` writes an audit entry, restarts only that service, and exits 0.
- `inspect compose restart arte/luminary-onyx` (no service) exits 2 with "specify a service or pass --all".
- `inspect compose restart arte/luminary-onyx --all --dry-run` lists every service that *would* restart without doing it.
- `inspect status arte` output includes `compose_projects: 2` (or whatever the count is); `--json` includes the structured project list.
- `inspect compose up`, `inspect compose down`, `inspect compose pull` exit 2 with "intentionally not implemented in v0.1.3 — see `inspect help compose`".

---

### F7 — Selector / output ergonomic papercuts (4 small fixes bundled)
**Source:** v0.1.2 field feedback (2nd field user, "minor papercuts" section). Bundled because each fix is < 30 LOC and they all touch error formatting / selector parsing / output trimming.
**Severity:** Low individually; collectively they remove four obvious "the tool should have just told me" moments from a fresh user's first hour.
**Problem + Fix (one bullet each):**

1. **Pre-setup verb error points at `inspect profile` instead of `inspect setup <ns>`.** Today, `inspect logs arte/onyx-vault` before discovery has run errors with `servers tried: arte / services available: (none)` and a hint pointing at `inspect profile`. The operator wanted "run `inspect setup arte` first". Fix: detect the "namespace known, services empty" case in `src/commands/logs.rs` (and every other read verb) and emit the chained hint `→ run 'inspect setup <ns>' to discover services on this namespace`. The `inspect profile` hint stays for the genuinely-misconfigured-namespace case (namespace not in `servers.toml`).

2. **`arte:` shorthand for host-file paths is rejected with a generic "invalid selector character ':'" error.** The correct form is `arte/_:/path` (the `_` indicates "host, not container"). Fix: in the selector parser (`src/selector/parse.rs`), specifically detect the `<ns>:<absolute_path>` shape and emit `error: 'arte:/path' looks like a host-path selector — did you mean 'arte/_:/path'? (the '_' selector targets the host filesystem)`. Pure error-message change, no parser-grammar change.

3. **`inspect ports arte | grep 8200` ergonomics.** Operators want a structured filter, not a grep. Fix: add `--port <n>` (single port) and `--port-range <lo-hi>` to `inspect ports`, filtering the table server-side. `inspect ports arte --port 8200` returns only rows where host or container port matches 8200. JSON output respects the same filter.

4. **`inspect logs --tail` collides with the user's own `| tail` because the `SUMMARY` block is at the end.** Fix: add a global `--quiet` flag that suppresses the trailing `SUMMARY` and `NEXT:` blocks (keeps `DATA` only), making output safe to pipe into `tail`, `head`, `grep -A`, etc. without worrying about trailer corruption. Documented in `docs/MANUAL.md` under "piping output". `--quiet` is mutually exclusive with `--json` (json output is already trailer-free).

**Test:**
- Pre-setup: `inspect logs arte/onyx-vault` before any discovery for `arte` exits 2 with the chained hint pointing at `inspect setup arte`. Same for `inspect run`, `inspect why`, `inspect cat`.
- `inspect cat arte:/etc/hosts` exits 2 with the `arte/_:/etc/hosts` suggestion. `inspect cat arte/_:/etc/hosts` succeeds.
- `inspect ports arte --port 8200` returns only matching rows; `--port-range 8000-9000` returns the range; `--port 8200 --json` schema is unchanged minus the filter.
- `inspect logs arte/onyx-vault --tail 50 --quiet | tail -10` prints exactly 10 log lines with no `SUMMARY` / `NEXT` text appended.
- `--quiet --json` exits 2 with "mutually exclusive" error (json is already quiet).

---

### L4 — Password authentication + extended session TTL + `ssh add-key` helper
**Source:** Roadmap "Remaining work" / known limitation; key-only auth blocks integration with shared bastions and legacy boxes that still require password auth. Refined during v0.1.3 backlog review: password auth is only acceptable if (a) the session TTL is long enough that passwords are not re-prompted within a working session, and (b) there is a one-command path off password auth onto keys.
**Severity:** Medium (blocks adoption on legacy infrastructure).
**Problem:** `inspect connect` only supports key-based SSH auth. Legacy servers, locked-down bastions, and password-only managed appliances cannot be onboarded today. Operators must shell out to plain `ssh` and lose the audit trail / ControlMaster / discovery cache. Even when password auth is available, re-prompting every ~4h within a long working session would make the feature unusable.
**Fix:**
- New per-server config field `auth = "password"` in `~/.inspect/servers.toml`. Default remains `"key"`.
- Password sources, in order: `password_env = "VAR_NAME"` → interactive prompt at `inspect connect`. Never stored on disk.
- **Session TTL.** When `auth = "password"`, the OpenSSH ControlMaster `ControlPersist` defaults to `12h` (was effectively the agent / system default, often 4h). Configurable per-server via `session_ttl = "24h"`. Cap at 24h so a forgotten laptop doesn't hold a live remote session indefinitely; document the cap. Key-auth servers keep the existing default unless explicitly overridden — only password auth gets the longer default, because re-prompting passwords is the painful case.
- Password entered once at `inspect connect`; every subsequent `inspect <verb>` reuses the ControlMaster socket without re-prompting until TTL expires or the user runs `inspect disconnect`.
- **`inspect ssh add-key <ns>` helper** (new verb). Wraps `ssh-copy-id`: generates a key pair under `~/.ssh/inspect_<ns>_ed25519` if none specified, copies the public key to the remote `authorized_keys` over the current (password) session, then offers to flip the server's `auth = "password"` config to `auth = "key"` with `key_path = "..."`. One command from "password-only legacy box" to "first-class key-auth namespace".
  - `--key <path>` to use an existing key instead of generating one.
  - `--no-rewrite-config` to skip the auth-flip prompt (just install the key).
  - Audit-logged like any other write: `verb=ssh.add-key, target=<ns>`.
- One-time warning printed at first password connect: `warning: password auth is less secure than key-based. Run 'inspect ssh add-key <ns>' to migrate.`
- Max 3 failed password attempts then abort with a clear chained hint (point at `inspect help ssh`).
- Update `inspect connectivity` to surface password-auth status, current session TTL, and remaining session lifetime in its triage output.

```toml
[legacy-box]
host = "legacy.internal"
user = "admin"
auth = "password"
password_env = "LEGACY_BOX_PASS"   # optional; falls back to prompt
session_ttl = "12h"                # optional; default 12h for password, capped at 24h
```

**Test:**
- `inspect connect legacy-box` with `LEGACY_BOX_PASS` set succeeds without a prompt; unset, prompts once and connects.
- After connect, `inspect logs legacy-box/<svc>` reuses the session without re-prompting.
- ControlMaster persists for the configured TTL; verify by checking the socket exists 5 minutes after connect and a fresh command does not re-prompt.
- `session_ttl = "48h"` is rejected with a clear error explaining the 24h cap.
- `inspect ssh add-key legacy-box` over a live password session: generates a key, installs it, prompts to flip config; after flip, `inspect connect legacy-box` succeeds with key auth and never prompts.
- 3 wrong passwords aborts cleanly with non-zero exit and a hint.
- `inspect connect` against a `auth = "key"` server is unchanged (regression guard).
- `inspect connectivity legacy-box` shows `auth: password`, `session_ttl: 12h`, `expires_in: 11h47m`.

---

### L2 — OS keychain integration (opt-in, cross-session only)
**Source:** Roadmap "Remaining work" / known limitation. Refined during v0.1.3 backlog review: keychain is **only useful for cross-session persistence**. Within a single shell session, ssh-agent + ControlMaster already gives "enter passphrase once and reuse for the rest of the session" — that is the default, expected behavior and most users want exactly that (passphrase gone on logout, re-entered next session). Keychain is the opt-in for the smaller group who want passphrases to survive reboot.
**Severity:** Low (opt-in convenience for a subset of users; not on the default path).
**Problem:** A subset of operators want SSH passphrases to persist across shell sessions and reboots without leaving them in env vars, `.envrc` files, or shell history. There is no OS-native option today.
**Default behavior is unchanged and remains the recommended path:** ssh-agent holds the passphrase for the life of the shell session; logout / reboot clears it; next session prompts once again. This is what most users want and the docs should say so explicitly.
**Fix:**
- Add the `keyring` crate (single dep, cross-platform). Backends: macOS Keychain, GNOME Keyring (Secret Service), KDE Wallet, Windows Credential Manager (via WSL2).
- **Opt-in only.** No automatic keychain use. New explicit flag: `inspect connect <ns> --save-passphrase` — prompts once, stores in OS keychain under service `inspect-cli`, account `<namespace>`. Without the flag, behavior is exactly as in v0.1.2 (ssh-agent / per-session prompt).
- Subsequent `inspect connect <ns>` auto-retrieves silently **only if the namespace was previously saved with `--save-passphrase`**.
- New management verbs:
  - `inspect keychain list` — show stored namespaces (no values).
  - `inspect keychain remove <ns>` — delete entry.
  - `inspect keychain test` — verify the OS backend is reachable; exit 0/non-zero with a clear hint when keychain is unavailable.
- New credential-resolution order: socket → user ControlMaster → ssh-agent → **OS keychain (only if `--save-passphrase` was previously used for this ns)** → env var → prompt.
- Headless / CI: skip keychain silently if backend unavailable, fall back to env var or prompt. No hard dep on a running keychain daemon.
- **Inspect never writes passphrases to its own files**, ever. Keychain is the one persistent path, and only when explicitly opted into.
- Docs (`docs/MANUAL.md`) get a short "credential lifetime" section that spells out the three options: (1) **default** — ssh-agent, one prompt per shell session; (2) **`--save-passphrase`** — OS keychain, persists across sessions and reboots; (3) **env var** — for CI / scripted use only.

**Test:**
- Default path (no `--save-passphrase`): `inspect connect arte` behaves identically to v0.1.2 — agent if loaded, otherwise prompt; nothing written to keychain.
- `inspect connect arte --save-passphrase` stores; subsequent connects in a fresh shell don't prompt.
- `inspect keychain list` shows `arte`; `inspect keychain remove arte` deletes; subsequent connect prompts again.
- Headless container with no keychain backend: `inspect connect --save-passphrase` warns once and falls back to per-session prompt; `inspect keychain test` exits non-zero with a hint.
- Order test: a stored keychain entry for `arte` is consulted only because `arte` was previously saved; a fresh ns `bravo` ignores the keychain entirely (no implicit cross-namespace lookups).

---

### L5 — Audit log rotation + retention policy
**Source:** v0.1.2 bundle implementation retrospective.
**Severity:** Low (no scale issue today, but will bite long-running installations and bundle-heavy workflows).
**Problem:** `~/.inspect/audit/` grows unbounded. Monthly JSONL files are manageable today, but a team running 50 bundle ops per day will accumulate years of entries plus orphaned snapshot directories. There is no rotation, no retention policy, no cleanup verb, and no snapshot GC.
**Fix:**
- New verb `inspect audit gc` with two retention modes:
  - `--keep <duration>` (e.g. `90d`, `4w`, `12h`) — delete audit entries older than N.
  - `--keep <N>` — keep the last N entries per namespace.
  - `--dry-run` — preview what would be deleted (counts + total bytes freed).
- Snapshot directory gets the same treatment in the same pass: any snapshot dir under `~/.inspect/snapshots/` not referenced by a retained audit entry is an orphan and gets cleaned (subject to `--dry-run`).
- New config option in `~/.inspect/config.toml`:
  ```toml
  [audit]
  retention = "90d"     # or "100" for entry count; unset = no automatic GC
  ```
  When set, GC runs lightweight on every `--apply` invocation (just checks the oldest file's mtime; no full scan unless rotation is needed).
- `inspect audit gc --json` for machine consumers (counts, freed bytes, deleted snapshot ids).
- Clear, loud output on what was deleted and what was kept; never silently delete on an unrecognized retention value.

**Test:**
- Backfill `~/.inspect/audit/` with synthetic entries dated >100d in the past; `inspect audit gc --keep 90d --dry-run` lists them; `inspect audit gc --keep 90d` deletes them and the orphaned snapshot dirs.
- `audit.retention = "90d"` in config: an `--apply` invocation triggers GC; second invocation within the same minute does not re-scan (cheap-path guard).
- `--json` output includes `deleted_entries`, `deleted_snapshots`, `freed_bytes`.
- Snapshot referenced by a retained audit entry is **never** deleted.

---

### L7 — Header / PEM / URL credential redaction in stdout
**Source:** v0.1.2 bundle implementation retrospective.
**Severity:** Medium (security — agent workflows pipe stdout into LLM context windows; one leaked Bearer token can be catastrophic).
**Problem:** The current secret masker (`src/redact.rs`) is line-oriented and only catches `KEY=value` env-var-style strings. It misses:
1. HTTP `Authorization: Bearer <token>` headers in `curl -v` output.
2. PEM private key blocks in file-content dumps (`cat /etc/ssl/private/...`).
3. Credentials embedded in URLs (`postgres://user:pass@host/db`, `redis://:pass@host:6379`).
4. Cookies / `Set-Cookie` headers carrying session tokens.
**Fix:** Add three additional masking patterns running in sequence alongside the existing env-var masker. Each is a separate module under `src/redact/` so future patterns plug in cleanly:

1. **Header masker** (`src/redact/headers.rs`):
   - Match (case-insensitive) `Authorization:`, `X-API-Key:`, `X-Auth-Token:`, `Cookie:`, `Set-Cookie:`, `Proxy-Authorization:`. Mask the value portion: `Authorization: Bearer ****`.
2. **PEM masker** (`src/redact/pem.rs`):
   - Match `-----BEGIN (RSA |EC |DSA |OPENSSH |ENCRYPTED )?PRIVATE KEY-----` through the matching `-----END ... PRIVATE KEY-----`. Replace the entire block (including delimiters) with `[REDACTED PEM PRIVATE KEY (N lines)]`.
3. **URL credential masker** (`src/redact/url_creds.rs`):
   - Match `<scheme>://<user>:<pass>@<host>` in connection strings (postgres, mysql, redis, mongodb, amqp, http, https, etc.). Mask the password portion only: `postgres://user:****@host/db`.

- All maskers run on every `inspect run`, `inspect exec`, `inspect logs`, `inspect search`, `inspect bundle run` stdout/stderr line.
- `--show-secrets` bypasses **all** maskers (extends existing behavior — single flag, single bypass).
- Maskers are streaming-safe (no buffering of full output); PEM masker uses a small line-window state machine.
- Per-masker counter exposed in `--json`: `redactions: { env: 2, header: 1, pem: 0, url: 1 }` so consumers can audit what was masked.

**Test:**
- `inspect run <ns>/<svc> -- 'curl -v https://api.example.com/'` masks the `Authorization:` header.
- `inspect run <ns>/<svc> -- 'cat /etc/ssl/private/test.pem'` replaces the PEM block.
- `inspect run <ns>/<svc> -- 'echo postgres://u:p@h/db'` masks the password.
- `--show-secrets` returns the unmasked output verbatim across all three patterns.
- `--json` output's `redactions` counters match the patterns triggered.
- Streaming test: a slow `curl -v` heartbeat does not break the header mask across chunk boundaries.

---

### L3 — Parameterized aliases
**Source:** Roadmap "Remaining work" / known limitation; agent + recipe ergonomics.
**Severity:** Medium (parser change + alias storage format change).
**Problem:** Aliases today are static strings. A recipe that wants "logs for *any* service on arte" has to either define one alias per service or fall back to writing the full LogQL each time. Agents can't compose aliases programmatically.
**Fix:** Add `$param` placeholders in alias bodies, supplied at call site as `@name(key=val,key2=val2)`. Aliases may chain other aliases.

```bash
# Define
inspect alias add svc-logs '{server="arte", service="$svc", source="logs"}'

# Use
inspect search '@svc-logs(svc=pulse) |= "error"'

# Chain
inspect alias add prod-pulse '@svc-logs(svc=pulse) |= "$pat"'
inspect search '@prod-pulse(pat=ERROR)'

# Agent discovery
inspect alias show svc-logs --json
# → {"name":"svc-logs","parameters":["svc"],"type":"logql","body":"..."}
```

Design points:
- `$param` syntax in alias body. Tokenizer recognizes `$<ident>` outside of string-literal quoting.
- Call site syntax: `@name(k=v,k=v)`. Bare `@name` still works for parameterless aliases (back-compat with v0.1.2 within the v0.1.x series).
- Missing required param → clear error listing the alias name and required params: `error: alias 'svc-logs' requires param 'svc' (call as @svc-logs(svc=...))`.
- Chain depth cap: 5. Beyond that → error at expansion time with the chain shown.
- Circular reference detection at definition time (`alias add`) — refuse to write the alias and explain the cycle.
- `inspect alias show <name> --json` includes a `parameters: []` list for agent discovery.
- **No defaults in v0.1.3.** `${svc:-pulse}` syntax is deferred to v0.2.0 so the parser stays minimal.
- Storage: `aliases.toml` schema gains an optional `parameters = []` field for cached param list (rebuilt on `alias add`); old aliases without the field continue to work.

**Test:**
- `alias add svc-logs '{service="$svc"}'` then `inspect search '@svc-logs(svc=pulse)'` resolves to `{service="pulse"}`.
- Missing param → exit 2 with the required-param error format.
- Chained aliases work to depth 5; depth 6 errors with the chain printed.
- `alias add a '@b'; alias add b '@a'` — second `alias add` errors with "circular reference: a → b → a".
- `inspect alias show <name> --json` includes the resolved `parameters` array.
- Bare `@name` (no parens) on a parameterless alias is unchanged from v0.1.2.

---

### L6 — Per-branch rollback tracking in bundle matrix steps
**Source:** v0.1.2 bundle implementation retrospective.
**Severity:** Medium (architectural — affects correctness of parallel rollback in the bundle engine that just shipped).
**Problem:** Bundle matrix steps (`parallel: true` with `matrix:`) currently execute as all-or-nothing per step. If 4 of 6 volume tars succeed and 2 fail, rollback today undoes the entire step — including the 4 successful branches whose output may already have been used downstream. The correct behavior: rollback only the branches that actually succeeded; leave the failed ones alone (nothing to undo).
**Fix:**
- Track per-branch completion status in the bundle executor (`src/bundle/exec.rs`):
  ```rust
  struct BranchResult {
      branch_id: String,        // e.g. "atlas_milvus"
      status: BranchStatus,     // Ok | Failed | Skipped
      audit_id: Option<String>, // per-branch audit entry id
  }
  ```
- On rollback, only execute the rollback block for branches with `status == Ok`. Failed branches are skipped silently (with an audit note explaining why).
- Audit log: each matrix branch gets its own audit entry under the same `bundle_id` and step id, with a `branch:` field for filtering.
- Rollback templating becomes branch-aware: `{{ matrix.<key> }}` inside a `rollback:` block resolves to **only the succeeded branches' values** when the rollback is triggered. (Inside the forward `exec:`, behavior is unchanged — full matrix.)
- New verb `inspect bundle status <bundle_id>` shows per-branch outcomes:
  ```
  bundle: atlas-phase-0-snapshot (id: 0e3a…)
    step: tar-volumes
      ✓ atlas_milvus    (12.3s)
      ✓ atlas_etcd      (4.1s)
      ✗ aware_milvus    (failed: no space left on device)
  ```
- `--json` variant emits the structured per-branch outcomes for agent consumption.
- Integration with B9's existing `on_failure: rollback_to:` — when rolling back **to** a checkpoint that includes a matrix step, all completed branches of that step are kept (rollback target is "everything after this checkpoint"); when rolling back **a** matrix step itself, only succeeded branches are reversed.

**Test:**
- Inject a mid-matrix failure (3 of 5 branches succeed, 2 fail) — rollback runs only for the 3 succeeded branches; audit log shows per-branch entries with correct `status`.
- `inspect bundle status <id>` displays the per-branch outcomes; `--json` matches.
- `rollback:` block referencing `{{ matrix.volume }}` is invoked exactly once per succeeded branch with the correct value substituted.
- A matrix step where all branches succeed and a *later* step fails (triggering rollback to a checkpoint before the matrix): rollback covers all branches as expected (regression guard for the existing path).
- `--json` audit query `inspect audit show --bundle <id>` returns the per-branch entries grouped by step.

---

### L1 — TUI mode (`inspect tui`)
**Source:** Roadmap "Remaining work" / known limitation; long-standing field request for an interactive dashboard.
**Severity:** Low (large feature, but read-only and additive — does not affect existing surface).
**Problem:** Interactive triage today is a sequence of `inspect status`, `inspect logs --follow`, `inspect why` invocations in separate terminals. Operators want a single live dashboard for at-a-glance health and drill-down. Agents do not need this; humans do.
**Fix:** Ship `inspect tui` — a read-only three-pane dashboard built on `ratatui`. Thin presentation layer over the existing verb functions (no new business logic). Read-only in v0.1.3; write actions inside the TUI are deferred to v0.2.0+.

```
┌─ Services ─────────────────┬─ Logs ──────────────────────────────────┐
│ ▶ pulse        ✓ healthy   │ [pulse]   Request received...           │
│   atlas        ✓ healthy   │ [atlas]   Query OK (42ms)               │
│   synapse      ✗ down      │ [synapse] Connection refused            │
├─────────────────────────────┼─ Detail ────────────────────────────────┤
│                             │ synapse — exited (code 137)             │
│                             │ depends_on: [pulse, atlas, redis]       │
│                             │ Suggested: inspect why arte/synapse     │
└─────────────────────────────┴─────────────────────────────────────────┘
 [q]uit [/]search [f]ollow [m]atch [enter]drill [w]hy [e]xec [?]help
```

Design points:
- **Crate:** `ratatui` + `crossterm`.
- **Left pane:** `inspect status` data, refreshed every 10s (configurable via `--refresh <dur>`).
- **Right-top pane:** `inspect logs --follow --merged` for the selected service(s); supports `/` search and `m` match-filter live.
- **Right-bottom pane:** service detail card (image, ports, depends_on, last health) + suggested `inspect why` invocation; pressing `w` runs `why` and replaces the pane content.
- **Keybindings:** `j`/`k` navigate, `Enter` drill, `/` search, `f` toggle follow, `m` set match filter, `w` run why, `r` force refresh, `?` help overlay, `q` quit. **No mouse.** **No custom layouts.** Fixed three-pane layout.
- **Selector targeting:** `inspect tui [<namespace>|<selector>]` — defaults to all configured namespaces; narrows to a namespace or selector when given.
- **Read-only:** any keypress that would cause a write action (`e`, `x`, etc. in v0.2.0+) shows `read-only in v0.1.3` in the status bar and exits the keypress.
- **Crash safety:** restore terminal state on panic via `crossterm::terminal::disable_raw_mode` in a panic hook; never leave the user's terminal in a broken state.
- **CI:** smoke test runs `inspect tui --selftest` which initializes the layout, ticks the refresh once, dumps the rendered frame to stdout as text, and exits 0.
- **Scope discipline:** no themes, no plugins, no custom layouts, no per-cell styling configuration in v0.1.3. The TUI is a window onto existing commands, nothing more.

**Test:**
- `inspect tui --selftest` exits 0 within 2s and prints a layout snapshot to stdout.
- Snapshot test: rendered frame for a fixed mock dataset matches a golden file in `tests/golden/tui/`.
- Panic hook test: forcibly panic inside the render loop, verify terminal mode is restored (`stty -a` snapshot returns to cooked mode).
- Keybinding test: each documented key produces the documented effect against a mock data backend (no real SSH).
- `inspect tui` against an unreachable namespace shows the namespace as `unreachable` in the left pane and surfaces the underlying connectivity error in the detail pane (no crash).

---

## Out of scope for v0.1.3
*(explicitly deferred — do not let scope creep drag these in)*

- **TUI write actions.** Read-only in v0.1.3; `e` / `x` / `apply` from inside TUI is v0.2.0+.
- **Compose write verbs beyond `restart`.** `inspect compose up` / `down` / `pull` / `build` / `exec` are intentionally deferred (see F6). They need a compose-state-mutation design pass and land in **v0.1.5 at earliest** — v0.1.4 is the Kubernetes release and will not touch compose.
- **Alias defaults / fallback values** (`${svc:-pulse}`). Parser stays minimal in v0.1.3; defaults land in v0.2.0.
- **Cross-medium bundles** (Docker + k8s steps in one bundle). v0.2.0 introduces k8s; cross-medium is post-v0.2.0.
- **CLI surface renames / config schema freeze.** Previously planned for v0.1.4; now the job of **v0.1.5** (the stabilization sweep), because v0.1.4 is reserved for Kubernetes. Resist any rename in v0.1.3 unless it is fixing an outright bug.
- **kubectl / k8s anything.** **v0.1.4** (was v0.2.0 — accelerated). The entire v0.1.4 release is dedicated to introducing the k8s medium, k8s-aware selectors, and the kubectl-equivalent verb surface.
- **Themes, plugins, custom layouts in TUI.** Permanent no for v0.1.x.

---

## Shipped
*(move items here when released)*

---

## Running total: 0 / 14 — **OPEN, committed to ship the full backlog**

**Why ship the entire backlog, not just F1:** v0.1.4 is now dedicated to Kubernetes. That means the docker / compose / SSH surface — every L-item and every F-item in this backlog — gets no further attention until **v0.1.5 at the earliest**. Slipping any item out of v0.1.3 effectively pushes it past two intervening releases (v0.1.4 k8s + v0.1.5 stabilization) into v0.2.0+ territory. The docker-host install base is the entire current user base of the tool, so leaving their backlog half-shipped while spending a release on k8s would be the wrong call. Ship all 14.

**Note on critical-issue rule:** F1 (status returns 0 services after `--force`) is a regression on a verb that runs in the first 30 seconds of every session, **independently confirmed by two field users** on 37- and 38-container hosts. It qualifies v0.1.3 for release on its own under the "one critical issue" clause — but the commitment now is full-backlog. F4 (compose-aware `why`) is the highest-leverage item and the answer to the dominant field complaint ("structured verbs stop one level too shallow"); it amplifies the value of every other diagnostic verb without expanding the surface. F5 lands with F4 (shared resolver path). F6 (compose verbs) is the largest surface in the release; its absence is the second-most-cited field gap, and given the v0.1.4 k8s diversion, **F6 cannot slip** without leaving compose users without a structured surface for two full release cycles.

**Release readiness gate (all must be green to tag v0.1.3):**
- All 14 items have a passing test (or test bundle) in their respective phase file.
- `tests/phase_f_v013.rs` covers F1–F7 end-to-end.
- `tests/no_dead_code.rs` and `tests/help_contract.rs` pass against the expanded verb surface (F6 adds the `compose` subcommand tree; F7 adds `--quiet` globally and `--port` / `--port-range` to `ports`).
- `docs/MANUAL.md` updated for: compose verbs (F6), `arte/_:` host-path selector (F7.2), `--quiet` piping section (F7.4), `inspect why` deep-bundle output and `--no-bundle` / `--log-tail` flags (F4).
- `docs/RUNBOOK.md` updated for: F2 three-bucket warning classification + scaling formula, F4 deep-bundle internals.
- `CHANGELOG.md` entry per item (14 bullets minimum).
- One end-to-end smoke test against a real multi-container host reproduces the second field user's Vault-style scenario and confirms F4 produces the deep-bundle output (this is the field-validation gate, not a unit test).

**Next step after L1 lands:** open `INSPECT_v0.1.4_BACKLOG.md` covering the **Kubernetes release** — k8s medium implementation, k8s-aware selectors (`<ctx>/<namespace>/<workload>`), kubectl-equivalent read verbs (`logs`, `describe`, `events`, `top`), kubectl-equivalent write verbs scoped conservatively (`scale`, `restart`, `delete pod` with audit), and the bundle-engine integration so cross-medium k8s+docker bundles become possible. The S1–S7 stabilization sweep that was previously slated for v0.1.4 (CLI surface audit, config freeze, JSON schema freeze, help audit, README rewrite, dead code + dependency audit, security audit) shifts to **`INSPECT_v0.1.5_BACKLOG.md`** — the last release before the v0.2.0 contract; v0.1.5 ships **no new features**, only stabilization. Update `INSPECT_ROADMAP_TO_v01.3.md` (or rename it) to reflect the v0.1.3 → v0.1.4 (k8s) → v0.1.5 (stabilization) → v0.2.0 (contract) sequence.
