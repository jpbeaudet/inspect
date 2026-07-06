# INSPECT v0.1.4 — Implementation Plan (Kubernetes)

**Author:** inspect-subcoord (agentic program, JP-commissioned) · **Branch:** `feat/v0.1.4-program` · **Date:** 2026-07-04
**Status:** Phase 4 DEEP DESIGN — in progress. Skeleton + Wave A specified; Waves B–E + release-readiness gate + CLAUDE.md amendment land across subsequent save-point turns.
**Inputs:** `INSPECT_v0.1.4_STUDY.md` (process reconstruction), `INSPECT_v0.1.4_SURFACE_MAP.md` (the surface), `INSPECT_v0.1.4_RESEARCH_SYNTHESIS.md` (validated deltas w1–w3 / P1–P8), raw dossiers under `INSPECT_v0.1.4_RESEARCH/`.

v0.1.4 is the **Kubernetes release** — the entire release introduces the k8s
runtime medium, k8s-aware addressing, and the kubectl-equivalent verb surface,
purely additive (docker users see zero change). This plan mirrors the v0.1.3
backlog process reconstructed in Phase 1: numbered items in a fixed block shape,
grouped into implementation **waves**, each item carrying acceptance tests + the
mandatory 5-surface update sweep, gated by a release-readiness checklist.

**Design quality bar (non-negotiable, from the charter + bible):**
- **Agentic-first — the shell is the integration layer.** Every verb learnable
  cold from `inspect <verb> --help`, chainable with no adapter. No MCP wrapper.
- **No verb invented without a mapped surface + a research-backed pain point or
  parity need.** Every K-item cites its surface-map row and its research delta
  (w-Dn / Pn). Items with neither do not exist.
- **Production grade only; no silent deferrals** (CLAUDE.md). Anything genuinely
  later is a *stated boundary against a real future release*, never "out of
  scope for v0.1.4."

---

## 1. Process (mirrors the reconstructed v0.1.3 machine)

**Item prefix `K<n>`** — Kubernetes medium. (Distinct from `F`/`L`/`S`/`P`;
lowercase `k<n>_*` for test names, per the `f14_*`/`l7_*` convention.)

**Status legend:** `🟦 Open · 🟧 In progress · ✅ Done · 🟥 Bumped/Boundary · ⏸ Proposed (JP-gated)`.

**Per-item block shape** (every K-item):
- `### K<n> — <title>` + metadata table (ID · Status · Priority · Source · Depends-on).
- **Problem** — the operator/agent pain, in-voice, with the research citation.
- **Design** — concrete modules touched + the actual API / flag / envelope additions.
- **Acceptance** — named tests (`k<n>_*` in `tests/phase_k_v014.rs`), the CHANGELOG
  entry, the help-surface additions, the doc sections.
- **5-surface sweep** — the checklist (source+tests / CHANGELOG / backlog row /
  MANUAL+RUNBOOK / `-h` help) that every closing commit must satisfy.
- **Research refs** — the `w-Dn` / `Pn` deltas the item implements.

**Delivery discipline:** one commit per K-item (never bundled); subject
`K<n>: <desc>`; body = sub-section breakdown; footer
`Closes K<n> in INSPECT_v0.1.4_BACKLOG.md.` + `Co-Authored-By:`. Marker
convention `K<n>` / `(v0.1.4)` on every new comment/docstring through the window,
stripped at Cleaning Duty. Every wave closes only on
`export PATH=$HOME/.cargo/bin:$PATH; cargo check → clippy --all-targets -D warnings → test`
green + the deferral-scan grep clean + the 5-surface sweep per item.

*(Note: the working file for itemized delivery is `INSPECT_v0.1.4_BACKLOG.md`
per the v0.1.3-tail naming; this IMPLEMENTATION_PLAN is the design synthesis that
seeds it. Both are root-level, archived to `archives/v0.1.4/` at tag. During
implementation the two may merge into the single backlog file — decided at Wave-A
kickoff to avoid dual-maintenance drift.)*

---

## 2. JP-ratification gate (Q1–Q8) — drafted against recommendations, marked ⏸ Proposed

Per the CLAUDE.md status discipline (Proposed until the decision is settled),
every item that depends on a JP-gated question is marked **⏸ Proposed** and
carries its Q-ref. Implementation of a ⏸ item does not begin until JP ratifies.
The plan is drafted against the **recommended** answers so ratification is a
yes/no, not a redesign.

| Q | Decision | Recommendation (from research) | Gates items |
|---|---|---|---|
| Q1 | Selector grammar | **2-seg `<inspect-ns>/<workload>` + `-n`/`-A`**; context+ns in config; pin `--context` always (w1-D9, w2-D5, w3-P1) | K1,K2,K5,K7,K8 |
| Q2 | Backend | **kubectl shell-out**; Runtime trait keeps kube-rs swap mechanical (w1-D7, w3-D11) | K1,K3,K4 |
| Q3 | Cross-medium bundles | **Build the seam in v0.1.4; mixed-composition v0.2.0** (w3-D9) | K23 |
| Q4 | Write set | **scale + rollout-restart + delete-pod + exec --apply**, `rollout undo` as first-class verb, delete-pod/scale-to-0 **outage guard** (w1-D10, P4) | K15–K19 |
| Q5 | Resource breadth | **pods/deploy/svc/events/top + `describe` for common kinds; `search` covers the rest; secrets always redacted** | K10–K14 |
| Q6 | `connect` for k8s | **N/A-with-safety-note** (kubeconfig stateless, no sticky-context footgun) (w2-D7, w3-Q6) | K6 |
| Q7 | `port-forward` | **Refuse-with-hint in v0.1.4** (documented-fragile) (w1-D11) | K20 |
| Q8 | Field base mid-migration? | **Ask JP** — bears on whether mixed-composition (Q3) moves up (w3-D9) | (informational; may reprioritize K23) |

---

## 3. Bible-interaction flags (design choices that extend/touch CLAUDE.md)

These drive the Deliverable-2 CLAUDE.md amendment (next turn). Each is
**additive** — none contradicts an existing invariant.

1. **New runtime axis, orthogonal to `Medium`.** `Medium` (the `source=` label
   parser) stays unchanged; k8s is a **runtime** axis (docker vs k8s) introduced
   at the namespace level. The bible's selector/medium model is *extended*, not
   altered. **Flag:** document the runtime-vs-medium distinction so future
   readers don't conflate them.
2. **`AuditEntry` extension (no schema break).** k8s writes add
   `context` / `k8s_namespace` as `Option<T>` + `skip_serializing_if` — exactly
   the bible's audit-schema rule (freeze-bound v0.2.0). `failure_class` (already
   `Option<String>`, F13) gains new *values* (`rbac_forbidden`,
   `no_shell_in_container`, `metrics_unavailable`, k8s transport classes) — a
   value-space extension, not a field add. **Flag:** record the new value family.
3. **Exit-code policy for k8s transport.** The bible fixes `12–14` as SSH-F13
   transport codes and says "don't reuse a code for a new meaning." k8s transport
   is *semantically parallel* (unreachable / auth-expired / stale) but a
   *different medium*. **Design decision (flag for JP in the amendment):** reuse
   the `12–14` class where the *semantic* matches (unreachable=12, auth=13,
   protocol/other=14) since the meaning is "transport failure, class X" not
   "SSH failure" — OR allocate a parallel k8s band. Recommend **reuse by
   semantic class** (an agent branches on the *class*, and `failure_class`
   carries the medium-specific detail); the exit code stays a coarse class, the
   JSON `failure_class` the fine detail. Ratify in the amendment.
4. **Dependency Policy.** kubectl shell-out = **no new crate** (probe `kubectl`
   like `docker` — Dependency-Policy-clean). A future `kube-rs` swap is an
   explicit ADR/decision, not a silent add. **Flag:** record the kubectl-first
   decision + the swap-is-an-ADR note.
5. **Anti-footgun write property.** Every k8s write echoes the resolved
   context+namespace+workload in preview/prompt/audit/`meta` (w3-P1). **Flag:**
   add this as a k8s write-verb contract alongside the F11 revert contract.

---

## 4. Wave overview (A → E)

**STATUS (2026-07-05): WAVE A COMPLETE ✅** — K1–K5 all landed, live-tested
against the maker cluster, wave-close gate green (clippy -D warnings clean;
full suite 31 suites / 0 failed / 1394 tests). Findings WA-1/WA-2/WA-5 fixed;
WA-3/WA-4/WA-6 tracked to K6. Next: Wave B (K6–K9).

**PROGRAM BUILD STATUS (2026-07-06, autopilot Phase 5).** 23 of 25 K-items
landed + committed on `feat/v0.1.4-program`, every one live-verified against
the real maker/hub clusters; clippy + help_contract green throughout.
- **Wave A ✅** K1 Runtime trait · K2 config · K3 kubectl probe · K4 failure
  taxonomy · K5 context-pinning.
- **Wave B ✅** K6 discovery (setup/test/auth-can-i/metrics) · K7 status/ps/
  health · K8 logs · K9 cat/ls/grep/run.
- **Wave C ✅** K10 why (dogfooded a real degraded hub pod) · K11 describe ·
  K12 events · K13 top · K14 ports/network/volumes/images.
- **Wave D ✅ (code)** K15 scale · K16 restart · K17 rollout · K18 delete-pod ·
  K19 exec --apply · K20 refuse mappings · WD-2 local revert executor. Every
  write: dry-run default + resolved-target echo + F11 revert capture + audit
  (context/k8s_namespace). ALL dry-run paths live-verified; no mutation
  performed.
- **Wave E:** K21 fleet mixed rollup ✅ · K24 help topic ✅ · K25 SMOKE
  runbook ✅.
- All findings WA-1..7 resolved (WA-3/WA-4 per JP-2026-07-05).

**Two remaining decisions (surfaced to JP):**
1. **K22 `search` + K23 bundle seam — DEFERRED to the v0.1.5 usage-validated
   pool (JP-2026-07-06).** They are deep integrations into SSH-coupled engines
   (the LogQL per-medium readers; the bundle step executor + preflight/
   postflight, all `runner.run`-over-SSH). Research w3-D9 is the *hypothesis*
   that there is no v0.1.4 field demand (single-medium pains dominate;
   cross-medium bundle *composition* is already bounded to v0.2.0). **v0.1.5 is
   a USAGE-FEEDBACK / dogfooding phase, not a build-list**: once v0.1.4 ships,
   devops sub-coords USE inspect on real maker work and each reports at
   session-end (what-went-well / what-went-bad / *the ONE feature that would
   have helped most*). Those reports ARE the v0.1.5 backlog (like the v0.1.3
   backlog). K22/K23 sit in that pool and get **built only if real usage names
   search/bundle as "the one feature needed"** — evidence decides, not a
   pre-commitment. After v0.1.5 usage shows the medium is solid → bump to 2.0 /
   drop the experimental flag. The practical k8s CLI (every read + write verb,
   fleet, help, smoke) ships complete in v0.1.4 without them.
2. **WD-1 mutating round-trip** (apply + local revert) is the one test that
   requires a real maker mutation (throwaway deployment in `inspect-livetest`).
   All write DRY-RUN paths are verified; the mutating cycle is scripted in
   `SMOKE_v0.1.4.md` P9. Needs explicit authorization to execute (autopilot
   guardrail forbids initiating maker mutations on a generic nudge).

| Wave | Theme | Items | Gates |
|---|---|---|---|
| **A** ✅ | Foundation: runtime abstraction, config, backend probe, failure taxonomy, context-pinning | K1–K5 | **DONE** — blocks-all satisfied. |
| **B** | Discovery + read core (daily driver) | K6–K9 | Needs A. |
| **C** | k8s-native reads (`why`/`describe`/`events`/`top` + resource mappings) | K10–K14 | Needs A,B. |
| **D** | Conservative audited writes | K15–K20 | Needs A,B; F11 revert contract. |
| **E** | Integration + polish (`fleet`/`search`/bundle seam/help/smoke) | K21–K25 | Needs A–D. |

Full K-item map (skeleton; Waves B–E expand on later turns):

- **A** K1 Runtime executor trait + docker refactor · K2 Namespace `type`/kubeconfig config + conditional validate · K3 kubectl backend probe + preflight · K4 k8s failure-class taxonomy + stderr classifier + exit-code policy · K5 Context-pinning invariant + resolved-target echo (anti-footgun).
- **B** K6 k8s `setup`/`test` (inventory + `auth can-i` + metrics-server probe) · K7 `status`/`ps`/`health` · K8 `logs` (`-c` auto-pick, `--previous`, `--merged` tagging + heartbeat, `-A`) · K9 `cat`/`ls`/`grep`/`run` (+ no-shell detection).
- **C** K10 `why` (Popeye model) · K11 `describe` · K12 `events` · K13 `top` · K14 `ports`/`network`/`volumes`/`images`.
- **D** K15 `scale` · K16 `restart` (rollout) · K17 `rollout undo` · K18 `delete pod` (outage guard) · K19 `exec --apply` · K20 REFUSE mappings + `port-forward`.
- **E** K21 `fleet` mixed rollup · K22 `search` across mediums · K23 bundle runtime-aware seam · K24 `kubernetes.md` help topic + `LONG_*` sweep · K25 smoke runbook (`SMOKE_v0.1.4.md`).

---

## 5. WAVE A — Foundation (K1–K5) — fully specified

Wave A builds the runtime abstraction everything else rides on. It ships **no
user-visible k8s verb** on its own — it is the seam, the config, the backend
probe, the failure taxonomy, and the context-pinning invariant. Its acceptance
is structural (trait wired, docker still green, k8s namespace parses + validates
+ probes) plus a minimal end-to-end "k8s namespace resolves and reports backend
readiness" path.

### K1 — Runtime executor trait + docker refactor behind it

| Field | Value |
|---|---|
| **ID** | K1 |
| **Status** | ⏸ Proposed (Q2) |
| **Priority** | HIGH (foundational — blocks every other K-item) |
| **Source** | Surface Map §2 (the docker-coupling seam); roadmap "Executor trait" |
| **Depends-on** | — (first item) |

**Problem.** Today the docker runtime is assumed structurally:
`discovery/probes.rs` builds `docker ps`/`docker inspect` and
`verbs/dispatch.rs` builds `docker logs|exec|restart|…` inline. There is no
abstraction to swap in `kubectl`. Without a runtime trait, every verb would need
a docker-vs-k8s `if` — unmaintainable and exactly the kind of drift the codebase
avoids.

**Design.**
- Introduce a **`Runtime` trait** (working name; `src/exec/runtime.rs` or
  `src/runtime/`) abstracting the command-building + inventory + transport
  concerns the surface map §2 table enumerates: `inventory()`,
  `resolve_target(selector)`, `build_read_exec(target, cmd)`,
  `build_write_exec(...)`, `build_logs(...)`, `build_lifecycle(...)`,
  `classify_failure(stderr, code)` (K4 fills this).
- **Refactor the existing docker paths behind a `DockerRuntime` impl** — a pure
  refactor, **zero behavior change for docker**; the existing docker test suite
  is the regression gate (must stay 100% green).
- Add a `K8sRuntime` **stub impl** in K1 (methods present, wired, returning
  "not-yet-implemented for this verb" only where a later K-item fills them) — but
  per no-silent-stubs, K1's k8s impl provides only what K1 owns (command
  assembly skeleton + the trait wiring); it does **not** ship half-verbs. Verbs
  land in Waves B–D, each fully.
- Runtime selection: a namespace's `type` (K2) selects the impl at dispatch.

**Acceptance.**
- `k1_docker_runtime_parity_*` — the docker suite runs unchanged through the
  trait (behavior-preserving refactor; existing docker tests are the gate).
- `k1_runtime_selected_by_namespace_type` — a `type="k8s"` namespace routes to
  `K8sRuntime`, `type="docker"`/absent to `DockerRuntime`.
- `k1_runtime_trait_object_safe` — the trait is object-safe / dispatchable.
- CHANGELOG: "Added — internal Runtime abstraction (docker refactored behind it;
  no behavior change); k8s runtime introduced (verbs land in subsequent items)."
- Help: no user-facing surface yet (internal). `docs/RUNBOOK.md` gets a "Runtime
  abstraction" internals section (the docker-vs-k8s dispatch contract).
- **5-surface sweep:** source+tests ✓ / CHANGELOG ✓ / backlog row ✓ / RUNBOOK
  internals ✓ / `-h` n-a (internal) — noted explicitly.

**Research refs.** Surface Map §2; w1-D8 (Runtime-trait invariants); the whole
"same verb, new medium" thesis.

---

### K2 — Namespace `type` / kubeconfig config + type-conditional validation

| Field | Value |
|---|---|
| **ID** | K2 |
| **Status** | ⏸ Proposed (Q1) |
| **Priority** | HIGH (foundational — declares a k8s namespace) |
| **Source** | Surface Map §3; roadmap config seed |
| **Depends-on** | — (parallel-safe with K1; K1 consumes `type`) |

**Problem.** `NamespaceConfig` (`src/config/namespace.rs`) is the docker-over-SSH
shape (`host`/`user`/`auth`/…) and `validate()` **requires `host` + `user`**. A
k8s namespace has neither; it has a kubeconfig context. There is no way to
declare a k8s namespace today.

**Design.**
- Add to `NamespaceConfig`: `type: Option<String>` (`"docker"` default when
  absent | `"k8s"`), `kubeconfig: Option<String>`, `context: Option<String>`,
  `namespace: Option<String>` — all `Option`, all
  `#[serde(skip_serializing_if = "Option::is_none")]`.
- **`validate()` becomes type-conditional:** docker keeps the `host`+`user`
  requirement; k8s requires **neither** (needs a resolvable context, checked at
  `setup`/`test` in K6). SSH-only fields on a k8s namespace are inert; `show`
  renders them N/A rather than erroring.
- `add` (interactive) supports `type=k8s` with kubeconfig/context/namespace
  prompts. **Config `schema_version` bumps** (bible: config changes are versioned
  pre-freeze) — flagged as a config-schema change in CHANGELOG.

**Acceptance.**
- `k2_k8s_namespace_parses_without_host_user`,
  `k2_docker_namespace_still_requires_host_user`,
  `k2_type_defaults_to_docker_when_absent`,
  `k2_show_renders_ssh_fields_na_for_k8s`,
  `k2_schema_version_bumped`.
- CHANGELOG: config-schema-change flag (new `type`/`kubeconfig`/`context`/
  `namespace` fields; `validate()` now type-conditional).
- Help/docs: `inspect add --help` + `docs/MANUAL.md` "Kubernetes namespaces"
  config section (the `type="k8s"` stanza worked example, SM §3).
- **5-surface sweep** all five.

**Research refs.** SM §3; w2-D5 / w3-P1 (context+ns in config = the anti-footgun
model). ⏸ on Q1 ratification.

---

### K3 — kubectl backend probe + preflight

| Field | Value |
|---|---|
| **ID** | K3 |
| **Status** | ⏸ Proposed (Q2) |
| **Priority** | HIGH (every k8s verb depends on a working kubectl) |
| **Source** | SM §9; w1-D8(1) k9s kubectl-not-found breaks exec |
| **Depends-on** | K1, K2 |

**Problem.** The kubectl shell-out backend is only as reliable as the `kubectl`
binary being on PATH — k9s's most-cited operational failure is
`executable file not found` when kubectl is absent. inspect probes `docker`
today (`discovery/probes.rs`); k8s must probe `kubectl` the same way and fail
**loud, specific, actionable** (bible CI-gate-quality rule), never with a raw
OCI/exec error.

**Design.**
- Add a `kubectl` presence + version probe to the tool-probe layer (alongside
  the `docker`/`rg`/`jq`/… probes in `discovery/probes.rs` /
  `profile/schema.rs`'s tool table). Record `kubectl` availability + version in
  the cached profile.
- On a k8s verb when kubectl is absent: a four-question error — **what** (kubectl
  not found on PATH), **where** (the PATH searched), **why** (k8s namespaces
  require kubectl in the shell-out backend), **fix** (install kubectl / add to
  PATH), exit as a usage/preflight class.
- Minimum kubectl version gate (documented floor; warn-or-fail on older).

**Acceptance.**
- `k3_kubectl_probe_detects_presence_and_version`,
  `k3_k8s_verb_fails_loud_when_kubectl_absent` (asserts the four-question
  message), `k3_kubectl_version_floor_enforced`,
  `k3_docker_namespace_unaffected_by_kubectl_absence`.
- CHANGELOG entry. Help: `inspect help kubernetes` (K24 topic) preflight
  paragraph; the error message itself is the load-bearing help surface.
- **5-surface sweep** all five.

**Research refs.** w1-D8(1); SM §9 "probe like docker"; bible CI-gate-quality.

---

### K4 — k8s failure-class taxonomy + stderr classifier + exit-code policy

| Field | Value |
|---|---|
| **ID** | K4 |
| **Status** | ⏸ Proposed (Q2) |
| **Priority** | HIGH (cross-cutting; every k8s verb reports through it) |
| **Source** | SM §10; w1-D7, w3-P2/P5/P6 |
| **Depends-on** | K1 |

**Problem.** kubectl collapses NotFound / Forbidden / Unreachable / TLS-expired
all into exit `1`, with the distinction only in stderr prose (w1-D7). Agents
can't branch. inspect's `failure_class` + exit-code discipline exists to fix
exactly this — but the shell-out backend must **parse kubectl stderr** into a
class. This is the one non-trivial cost of shell-out over a typed client, and it
is a foundational, cross-cutting item.

**Design.**
- A `classify_failure(stderr, exit_code) -> FailureClass` for the k8s runtime
  (fills K1's trait method). Classes (extending the existing `failure_class`
  `Option<String>` value space — no schema change):
  - `rbac_forbidden` (w3-P2) — from `Error from server (Forbidden)`; the hint
    embeds the literal `kubectl auth can-i <verb> <resource> -n <ns>`.
  - `not_found` — `(NotFound)`.
  - `no_shell_in_container` (w3-P5) — distroless/minimal exec failure
    (`no such file or directory` on the shell/coreutil); hint → ephemeral-debug.
  - `metrics_unavailable` (w3-P6) — `Metrics API not available`; absent-vs-
    warming distinction; install hint.
  - **k8s transport classes** — API-server unreachable / kubeconfig missing /
    token-or-cert-expired / TLS — mapped onto the exit-code policy (§3-flag-3:
    reuse the 12–14 semantic class band; `failure_class` carries the k8s detail).
- Every class → a CI-gate-quality hint (what/where/why/fix) and a distinct
  `failure_class` string an agent branches on.

**Acceptance.**
- `k4_classify_forbidden_to_rbac_with_cani_hint`,
  `k4_classify_notfound`, `k4_classify_no_shell_in_container`,
  `k4_classify_metrics_unavailable_absent_vs_warming`,
  `k4_classify_transport_unreachable_maps_exit_class`,
  `k4_unknown_stderr_falls_back_safely` (no misclassification of an
  unrecognized error into a wrong branch).
- CHANGELOG: new `failure_class` value family (flagged as an additive
  value-space extension, not a schema change).
- Help: `inspect help kubernetes` + `inspect help safety` failure-class table;
  each verb's `LONG_*` references the classes it can emit.
- **5-surface sweep** all five.

**Research refs.** w1-D7; w3-P2/P5/P6; SM §10; bible F13 exit-code discipline +
CI-gate-quality rule.

---

### K5 — Context-pinning invariant + resolved-target echo (anti-footgun)

| Field | Value |
|---|---|
| **ID** | K5 |
| **Status** | ⏸ Proposed (Q1) |
| **Priority** | HIGH (safety-critical; the #1 kubectl horror class) |
| **Source** | w2-D5, w3-P1 (wrong-context/namespace destruction) |
| **Depends-on** | K1, K2 |

**Problem.** Wrong-context/namespace destruction is **THE #1 kubectl horror
class** (w3-P1): a stale global `current-context` deploys to prod instead of dev.
kubectx/kubens mutate global state that leaks across terminals (w2-D5). inspect's
structural win is that it *doesn't* carry sticky global state — but only if the
runtime **pins the context explicitly on every kubectl call** and **never reads
or mutates the ambient `current-context`**.

**Design.**
- **Invariant:** the k8s runtime passes `--context <ctx>` (and `--kubeconfig`
  when configured) on **every** kubectl invocation, resolved from the namespace
  config — never relying on / touching the ambient `current-context`. Enforced as
  a Runtime-trait contract with a test that greps assembled commands (mirrors
  the SSH `accept-new` / `StrictHostKeyChecking` invariant tests).
- **Resolved-target echo:** the resolved `{context, k8s_namespace, workload}` is
  carried in the JSON envelope `meta` for every k8s verb, and — for **writes** —
  echoed in the dry-run preview and the confirmation prompt ("will scale
  `deploy/api` in namespace `prod` on context `prod-eks`") and recorded in the
  `AuditEntry` (new `context`/`k8s_namespace` `Option` fields, §3-flag-2). This
  is the anti-footgun property: inspect does the "confirm which cluster" mitigation
  *for* the operator, always.

**Acceptance.**
- `k5_every_kubectl_command_carries_explicit_context` (command-assembly grep,
  the invariant test), `k5_ambient_current_context_never_read_or_mutated`,
  `k5_meta_carries_resolved_context_namespace_workload`,
  `k5_write_preview_and_prompt_echo_resolved_target`,
  `k5_audit_entry_records_context_and_namespace`.
- CHANGELOG: headline anti-footgun property (writes echo resolved
  cluster/namespace); audit-schema additive fields flagged.
- Help: `inspect help kubernetes` "Why inspect can't hit the wrong cluster"
  section (the safety property as a selling point); write verbs' `LONG_*` note
  the resolved-target echo.
- MANUAL: "Kubernetes addressing + the no-wrong-cluster guarantee" section.
- **5-surface sweep** all five.

**Research refs.** w2-D5, w3-P1/D2; the config-per-(context,namespace)
immunity-to-context-pong advantage. ⏸ on Q1.

---

## 5B. WAVE B — Discovery + read core (K6–K9) — fully specified

Wave B is the **daily driver**: it makes a k8s namespace discoverable and lands
the read verbs an operator/agent hits in the first minutes of every session.
Everything here rides K1 (runtime), K2 (config), K3 (probe), K4 (failure
classes), K5 (context pinning). No k8s write ships in Wave B.

### K6 — k8s `setup` / `test` (inventory + `auth can-i` self-test + metrics-server probe)

| Field | Value |
|---|---|
| **ID** | K6 |
| **Status** | ⏸ Proposed (Q1, Q6) |
| **Priority** | HIGH (nothing reads until the namespace is discovered) |
| **Source** | SM §5.4, §7; w3-P2 (`auth can-i` pre-empts RBAC), w3-P6 (metrics probe) |
| **Depends-on** | K1, K2, K3, K4, K5 |

**Problem.** A docker namespace is discovered via `docker ps`/`inspect`; a k8s
namespace has no equivalent path. Worse, an operator only learns mid-task that
they lack RBAC for a verb (w3-P2) or that the cluster has no metrics-server
(w3-P6). Discovery must front-load those answers.

**Design.**
- k8s `setup`/`discover`: `kubectl get pods,services,deployments,configmaps -o
  json` (one API round-trip — the docker per-object `inspect` timeout-batching is
  not needed, SM §7) → the cached profile `Service` model (pod →
  container-equivalent; Deployment/ReplicaSet → grouping; Service → ports).
- **`auth can-i` self-test** — run `kubectl auth can-i <verb> <resource>` for the
  verbs inspect will use (the *actual* set, not a superset — w1-D8(3) / bible
  security-scope-narrower) and record the gaps, so `setup` reports missing perms
  *before* a mid-task forbidden.
- **metrics-server probe** — record availability (+ warming state) so K13 `top`
  can pre-answer "this cluster has no metrics."
- **`test`**: `kubectl config get-contexts` + API reachability + the `auth can-i`
  matrix + kubectl version floor (K3).
- **Q6 resolution:** `connect`/`disconnect`/`connections` on a k8s namespace emit
  a clear **"N/A for k8s — kubeconfig is stateless; the target is resolved
  per-verb from config, so there is no sticky-context footgun"** note (a stated
  safety property, w2-D7/w3-Q6), not a silent no-op.

**Acceptance.**
- `k6_setup_inventories_pods_svc_deploy_cm`,
  `k6_setup_records_auth_cani_gaps`,
  `k6_setup_probes_metrics_server_presence`,
  `k6_test_reports_context_reachability_and_rbac_matrix`,
  `k6_connect_is_na_for_k8s_with_safety_note`.
- CHANGELOG entries (k8s discovery; `connect` N/A note).
- Help: `inspect setup --help` k8s behavior; `inspect help kubernetes` discovery
  + RBAC-self-test + metrics-probe paragraphs; `connect`'s `LONG_*` N/A note.
- MANUAL/RUNBOOK: "Kubernetes discovery" (MANUAL) + the inventory/`auth can-i`/
  metrics internals (RUNBOOK).
- **5-surface sweep** all five.

**Research refs.** SM §5.4/§7; w3-P2/P6; w2-D7; w1-D8(3). ⏸ Q1, Q6.

---

### K7 — `status` / `ps` / `health` (k8s)

| Field | Value |
|---|---|
| **ID** | K7 |
| **Status** | ⏸ Proposed (Q1) |
| **Priority** | HIGH (the first verb of every session) |
| **Source** | SM §5.1 (REUSE); w3-D1 (severity rollup) |
| **Depends-on** | K1–K6 |

**Problem.** `status`/`ps`/`health` are the "what's running and is it healthy"
rollup. They must present pods/deployments through the *same* health model docker
uses, so an agent that knows `inspect status arte` drives `inspect status
staging-k8s` identically.

**Design.**
- `status` — `kubectl get pods,deploy -o json` → the existing health rollup: pod
  phase + readiness + **restart counts** feed the same `state`/`summary`
  discriminators. Worst-severity **rollup** in `summary` (w3-D1 severity model).
- `ps` — lists pods (the container-equivalent) with the existing columns mapped
  (name, ready, status, restarts, age).
- `health` — pod readiness/liveness probe status from pod conditions; a **missing
  probe** is itself a reported finding (w3-D1 probe-presence).
- All emit the standard envelope; `meta` carries the resolved context+namespace
  (K5). `-n`/`-A` honored (w2-D9).

**Acceptance.**
- `k7_status_rollup_maps_pod_phase_readiness_restarts`,
  `k7_status_worst_severity_rollup_in_summary`,
  `k7_ps_lists_pods_with_mapped_columns`,
  `k7_health_reports_probe_status_and_missing_probe`,
  `k7_meta_carries_resolved_context` (K5 wiring),
  `k7_dash_n_and_dash_A_scope_reads`.
- CHANGELOG; help (`status`/`ps`/`health` `LONG_*` k8s notes + `-n`/`-A`); MANUAL
  k8s status section. **5-surface sweep** all five.

**Research refs.** SM §5.1; w3-D1; w2-D9. ⏸ Q1.

---

### K8 — `logs` (`-c` auto-pick+hint · `--previous` · `--merged` tagging + heartbeat · `-A`)

| Field | Value |
|---|---|
| **ID** | K8 |
| **Status** | ⏸ Proposed (Q1) |
| **Priority** | HIGH (top-3 field pain; the most footgun-laden read verb) |
| **Source** | SM §5.1; w1-D1/D2, w2-D1/D2/D3/D4, w3-P3/P8 |
| **Depends-on** | K1–K6 |

**Problem.** `kubectl logs` is the single most footgun-laden read verb: it errors
on multi-container pods without `-c` (w1-D1), hides crash logs behind an
undiscoverable `--previous` (w1-D2), can't fan out across replicas (the reason
stern/kail exist — w2-D1), silently picks the wrong pod of a rolling deployment
(w3-P8), and stalls silently on long tails (w2-D4). This is where inspect most
visibly out-does kubectl.

**Design.**
- **`-c`/`--container`** with **auto-pick of the first/app container + a chained
  hint listing the others + their restart counts** on a multi-container pod —
  do the auto-pick kubectl refused (w1-D1, w3-P3). Never error like kubectl.
- **`--previous`** for the last terminated container; `-h` states explicitly
  "shows the **last terminated** container only" to avoid stern's ambiguity
  (w2-D3). `logs`/`why` **auto-hint** "container X restarted N times — add
  `--previous`" when `restartCount>0` (w1-D2, w3-P3).
- **`--merged`** fans `kubectl logs -f` across all replicas of a
  Deployment/ReplicaSet (w2-D1); **every merged line is tagged with its source
  `{pod, container, revision, ts}`** in the `--json` stream (w2-D2, w3-P8) so
  agents can tell replicas apart and spot the "old pod, old image" trap.
- **Non-`--merged` deploy addressing** states which pod was selected and how many
  replicas exist ("showing 1 of 3 replicas: pod `api-abc` revision 7; use
  `--merged` for all") — never silently picks (w3-P8).
- **Heartbeat / stream-health guard** on long `-f`/`--merged` tails: a documented
  max-idle → a `failure_class` (or a heartbeat marker) rather than stern's silent
  stall (w2-D4). Composes with the existing F16 streaming + SIGINT-forwarding.
- **`--tail` / `--since` / `-A`** kubectl-parity flags.

**Acceptance.**
- `k8_multicontainer_autopicks_first_with_hint_listing_others`,
  `k8_previous_reads_last_terminated_and_help_disambiguates`,
  `k8_crashloop_auto_hints_previous_when_restartcount_positive`,
  `k8_merged_tags_each_line_with_pod_container_revision`,
  `k8_nonmerged_deploy_states_selected_pod_and_replica_count`,
  `k8_long_tail_emits_heartbeat_or_failure_class_not_silent_stall`,
  `k8_tail_since_A_flags_parity`.
- CHANGELOG (behavior notes: auto-pick, merged tagging, heartbeat).
- Help: `LONG_LOGS` k8s section (the `-c`/`--previous`/`--merged` semantics +
  the disambiguation + the replica-selection statement); MANUAL "Kubernetes logs"
  section; RUNBOOK merged-fan-out + heartbeat internals.
- **5-surface sweep** all five.

**Research refs.** SM §5.1; w1-D1/D2; w2-D1/D2/D3/D4; w3-P3/P8. ⏸ Q1.

---

### K9 — `cat` / `ls` / `grep` / `run` (+ no-shell detection)

| Field | Value |
|---|---|
| **ID** | K9 |
| **Status** | ⏸ Proposed (Q1) |
| **Priority** | MEDIUM-HIGH (in-pod read + exec surface) |
| **Source** | SM §5.1 (REUSE); w3-P5 (distroless no-shell) |
| **Depends-on** | K1–K6 |

**Problem.** `cat`/`ls`/`grep`/`run` map to `kubectl exec <pod> -- <cmd>`. But
they assume a shell / coreutils in the container — on distroless/minimal images
`kubectl exec -- cat/ls/sh` **fails with a raw OCI error** (w3-P5), the kind of
opaque failure that burns agent turns.

**Design.**
- `run` = read-only `kubectl exec <pod> [-c <ctr>] -- <cmd>` (no `/bin/bash`
  wrapper — avoids the k9s Alpine-no-bash trap, w1-D8(3)); `cat`/`ls`/`grep`
  build their commands the same way (`-- cat <path>` etc.).
- **No-shell detection:** classify the distroless/minimal exec failure into
  `failure_class = "no_shell_in_container"` (K4) with a hint pointing at
  ephemeral-container debug — **not** a raw OCI error (w3-P5). A first-class
  ephemeral-debug verb is a stated **v0.1.5+ boundary** (K-map C4), not a silent
  gap; v0.1.4 ships detection + hint.
- `-c`/`--container` for multi-container pods (same auto-pick+hint as K8).

**Acceptance.**
- `k9_run_execs_without_shell_wrapper`,
  `k9_cat_ls_grep_build_exec_dash_dash_commands`,
  `k9_distroless_no_shell_maps_failure_class_with_hint`,
  `k9_container_flag_selects_and_autopicks_with_hint`.
- CHANGELOG; help (`run`/`cat`/`ls`/`grep` `LONG_*` k8s notes + the no-shell
  class); MANUAL in-pod-read section. **5-surface sweep** all five.

**Research refs.** SM §5.1; w3-P5; w1-D8(3). ⏸ Q1.

---

## 5C. WAVE C — k8s-native reads (K10–K14) — fully specified

Wave C lands the diagnostics that have no docker analogue — the verbs that make
inspect *better* than raw kubectl for "what's wrong here." `why` is the flagship.

### K10 — `why` (Popeye content model + exit-reason table + severity rollup + `--previous` auto-hint)

| Field | Value |
|---|---|
| **ID** | K10 |
| **Status** | ⏸ Proposed (Q1, Q5) |
| **Priority** | HIGH (the flagship diagnostic; highest-leverage) |
| **Source** | SM §5.1; w1-D3, w3-D1/P3 (Popeye reference), Lens-Prism signal |
| **Depends-on** | K1–K8 |

**Problem.** Manual kubectl triage across pods/events/status doesn't scale
(w1-D10 refs); the market is moving to AI "why did this fail + how to fix"
copilots (Lens Prism). inspect's shell-native answer is a JSON `why` bundle +
chained `hint:`. Popeye is the reference design for *what belongs* in that
answer.

**Design.** `why <ns>/<workload>` assembles the k8s deep-diagnostic bundle into
the envelope `data`, with a worst-severity **rollup** in `summary`/`state` (w3-D1
severity model, Popeye OK/Info/Warn/Error). Bundle contents:
- Pod conditions + phase; **per-container `restartCount` + `lastState.terminated.
  {exitCode,reason}`** table (137 OOMKilled / 143 / 139) — the jsonpath everyone
  copy-pastes, done for the operator (w1-D3, w3-P3).
- **Events** for the object (K12), newest-first, joined in.
- **Probe presence + status** (readiness/liveness) — a *missing* probe is a
  finding (w3-D1).
- **Resource requests/limits presence** and **image-tag hygiene**
  (`:latest`/no-digest) — Popeye classes (w3-D1).
- **Dependency probing** (Service→Endpoint reachability, K14).
- **`--previous` auto-hint** when any container `restartCount>0` (w1-D2/w3-P3).
- Chained `hint:` to the next action (e.g. `inspect logs … --previous`,
  `inspect describe …`, the `auth can-i` command on RBAC gaps).

**Acceptance.**
- `k10_why_bundles_conditions_events_restarts_probes`,
  `k10_why_surfaces_exit_code_and_reason_table`,
  `k10_why_reports_missing_probe_and_missing_limits`,
  `k10_why_flags_latest_image_tag`,
  `k10_why_worst_severity_rollup_in_summary`,
  `k10_why_auto_hints_previous_on_restart`,
  `k10_why_chains_next_action_hint`.
- CHANGELOG; help (`LONG_WHY` k8s deep-bundle section); MANUAL "Kubernetes why"
  section; RUNBOOK bundle internals. **5-surface sweep** all five.

**Research refs.** SM §5.1; w1-D3; w3-D1/P3; Lens-Prism (w3-A.2). ⏸ Q1, Q5.

---

### K11 — `describe`

| Field | Value |
|---|---|
| **ID** | K11 |
| **Status** | ⏸ Proposed (Q1, Q5) |
| **Priority** | MEDIUM-HIGH |
| **Source** | SM §5.2 (NEW); w1-D4 |
| **Depends-on** | K1–K6 |

**Problem.** kubectl `describe` — the richest human view (spec + conditions +
events) — has **no `-o json`** (w1-D4); scripts/agents must fall back to `get -o
json` + manual event correlation. Reshaping describe into the envelope is a
genuine improvement, not just parity.

**Design.** `describe <ns>/<workload>` → `kubectl get <obj> -o json` (+ events)
reshaped into `data` (spec + conditions + events + status), envelope-standard,
`--select`-projectable. Covers the common kinds (pods/deploy/svc + the Q5 set);
`search` covers arbitrary kinds. **Secrets always redacted** through the
redaction family (Q5 mandate).

**Acceptance.**
- `k11_describe_reshapes_spec_conditions_events_into_data`,
  `k11_describe_json_is_envelope_and_selectable`,
  `k11_describe_redacts_secret_values`,
  `k11_describe_common_kinds_pods_deploy_svc`.
- CHANGELOG; help (`describe` `LONG_*` — note the "better than kubectl: JSON +
  `--select`" selling point); MANUAL. **5-surface sweep** all five.

**Research refs.** SM §5.2; w1-D4/D5. ⏸ Q1, Q5.

---

### K12 — `events` (newest-first · auto-scoped · fed to `why`)

| Field | Value |
|---|---|
| **ID** | K12 |
| **Status** | ⏸ Proposed (Q1) |
| **Priority** | MEDIUM-HIGH |
| **Source** | SM §5.2 (NEW); w3-P7 |
| **Depends-on** | K1–K6 |

**Problem.** `kubectl get events` is **not chronologically ordered by default**
(w3-P7, the #1 events complaint) — it needs a brittle `--sort-by='{...}'`
incantation (which even shipped a wrong example, k8s#21018). Events also expire
(~1h) and are hard to correlate.

**Design.** `events <ns> [/<workload>]` — **always newest-first** (documented in
`LONG_EVENTS`, same discipline as `LONG_AUDIT_LS`), **auto-scoped to the object**
(does the field-selector for the operator), fed into `why` (K10). Hint when the
~1h retention window may have expired. Standard envelope.

**Acceptance.**
- `k12_events_always_newest_first`,
  `k12_events_auto_scope_to_object`,
  `k12_events_feed_into_why`,
  `k12_events_hint_on_retention_expiry`.
- CHANGELOG; help (`LONG_EVENTS` ORDERING section, `audit ls` style); MANUAL.
  **5-surface sweep** all five.

**Research refs.** SM §5.2; w3-P7. ⏸ Q1.

---

### K13 — `top` (degrade → `metrics_unavailable`)

| Field | Value |
|---|---|
| **ID** | K13 |
| **Status** | ⏸ Proposed (Q1) |
| **Priority** | MEDIUM |
| **Source** | SM §5.2 (NEW); w3-P6 |
| **Depends-on** | K1–K6 (metrics probe from K6) |

**Problem.** `kubectl top` requires metrics-server, **absent by default on many
clusters (EKS)** (w3-P6); its error looks like a kubectl bug, not a missing
component, and it fails transiently for ~1min after install.

**Design.** `top <ns> [/<workload>]` → `kubectl top pods/nodes`. On absence,
degrade with `failure_class = "metrics_unavailable"` (K4), **distinguishing
absent vs just-started** (`retry ~60s`), giving the install command — never a raw
kubectl error. Uses the K6 metrics-server probe to pre-answer. `--sort-by`
cpu/memory (verb-owned, so agents never touch the brace form).

**Acceptance.**
- `k13_top_reports_cpu_mem_when_metrics_available`,
  `k13_top_degrades_metrics_unavailable_with_install_hint`,
  `k13_top_distinguishes_absent_vs_warming`,
  `k13_top_sort_by_cpu_memory`.
- CHANGELOG; help (`top` `LONG_*` + the degrade class); MANUAL. **5-surface
  sweep** all five.

**Research refs.** SM §5.2; w3-P6. ⏸ Q1.

---

### K14 — `ports` / `network` / `volumes` / `images` (k8s mappings)

| Field | Value |
|---|---|
| **ID** | K14 |
| **Status** | ⏸ Proposed (Q1) |
| **Priority** | MEDIUM |
| **Source** | SM §5.1 (REUSE+) |
| **Depends-on** | K1–K6 |

**Problem.** These four inventory verbs are docker-shaped today; k8s has
different backing objects (Services/Endpoints/NetworkPolicies, PVCs, pod-spec
images) that must map into the same verb contracts.

**Design.**
- `ports` — Service ports + container ports from `kubectl get svc`/pod spec
  (declarative), plus in-pod `ss` where exec is allowed.
- `network` — Services + Endpoints + NetworkPolicies (declarative) rather than
  docker networks; feeds `why`'s dependency probing (K10) and `connectivity`.
- `volumes` — PVCs / mounted volumes from pod spec + `kubectl get pvc`.
- `images` — container images from pod specs (`kubectl get pods -o jsonpath`).
- All envelope-standard; `-n`/`-A`; `meta` context (K5).

**Acceptance.**
- `k14_ports_maps_service_and_container_ports`,
  `k14_network_lists_services_endpoints_netpol`,
  `k14_volumes_maps_pvc_and_mounts`,
  `k14_images_from_pod_specs`.
- CHANGELOG; help (each verb `LONG_*` k8s notes); MANUAL. **5-surface sweep**
  all five.

**Research refs.** SM §5.1. ⏸ Q1.

---

## 5D. WAVE D — Conservative audited writes (K15–K20) — fully specified

Wave D lands the k8s write surface — dry-run-by-default, `--apply`-gated,
F11-revert-captured, audited, and **every write echoes the resolved
context+namespace+workload** (K5 anti-footgun, w3-P1). The set is deliberately
narrow (Q4); the community explicitly warns against the ops we exclude
(delete-pod-as-restart, scale-cycling) and endorses the ops we include (w3-P4).
Immutable-pod operations REFUSE with hints.

### K15 — `scale`

| Field | Value |
|---|---|
| **ID** | K15 |
| **Status** | ⏸ Proposed (Q1, Q4) |
| **Priority** | HIGH (cleanest revertible write; the write-surface template) |
| **Source** | SM §5.3; w1-D10, w3-P4 |
| **Depends-on** | K1–K6, F11 revert contract |

**Problem.** Scaling a workload is a daily write; done wrong (scale-to-0 then up)
it causes an outage window (w3-P4). It must be revertible and outage-aware.

**Design.** `scale <ns>/<workload> --replicas=N` → `kubectl scale deploy/<w>
--replicas=N`. **Revert = `command_pair`:** capture prior replica count →
inverse `kubectl scale --replicas=<prior>` (dispatchable locally against the
context — a valid `command_pair`, not `unsupported`). Adopt kubectl's
`--current-replicas=N` **optimistic-concurrency guard** for a safer apply.
Dry-run preview + confirmation echo the resolved target (K5). **Outage guard:**
if the op would drop ready replicas below threshold (e.g. `--replicas=0`),
escalate the confirmation and warn about the outage window (w3-P4/D5).

**Acceptance.**
- `k15_scale_applies_and_audits`,
  `k15_scale_revert_command_pair_restores_prior_replicas`,
  `k15_scale_dryrun_and_prompt_echo_resolved_target`,
  `k15_scale_current_replicas_guard`,
  `k15_scale_to_zero_escalates_confirmation_with_outage_warning`.
- CHANGELOG (new write verb + revert kind); help (`LONG_*` incl. revert +
  outage guard); MANUAL "Kubernetes writes" + RUNBOOK revert-capture. **5-surface
  sweep** all five.

**Research refs.** SM §5.3; w1-D10; w3-P4/D5. ⏸ Q1, Q4.

---

### K16 — `restart` (rollout restart + `rollout undo` revert)

| Field | Value |
|---|---|
| **ID** | K16 |
| **Status** | ⏸ Proposed (Q1, Q4) |
| **Priority** | HIGH (the correct "restart" idiom) |
| **Source** | SM §5.3; w1-D10, w3-P4 |
| **Depends-on** | K1–K6, K17, F11 |

**Problem.** Operators reach for `delete pod` to "restart," causing an outage;
the community-correct idiom is `rollout restart` (rolling, zero-downtime)
(w3-P4). inspect's `restart` on a k8s namespace must map to that, not delete-pod.

**Design.** `restart <ns>/<workload>` → `kubectl rollout restart deploy/<w>`.
**Revert = `command_pair`** (resolves SM §5.3 open Q): capture the current
`rollout history` revision **before** restart → inverse `kubectl rollout undo
--to-revision=<captured>` (w1-D10, w3-P4). Dispatchable locally, so it's a valid
`command_pair`, not `unsupported`. Preview/prompt/audit echo the resolved target
(K5). Composes with K17.

**Acceptance.**
- `k16_restart_maps_to_rollout_restart_not_delete_pod`,
  `k16_restart_revert_captures_revision_then_undo`,
  `k16_restart_dryrun_and_prompt_echo_resolved_target`,
  `k16_restart_audit_records_revision_for_revert`.
- CHANGELOG (behavior: `restart`=rollout restart on k8s; revert kind); help
  (`LONG_*`); MANUAL/RUNBOOK. **5-surface sweep** all five.

**Research refs.** SM §5.3; w1-D10; w3-P4. ⏸ Q1, Q4.

---

### K17 — `rollout undo` (first-class safety verb)

| Field | Value |
|---|---|
| **ID** | K17 |
| **Status** | ⏸ Proposed (Q1, Q4-add) |
| **Priority** | HIGH (fastest rollback of a bad deploy) |
| **Source** | w1-D10 (Q4 add-candidate); w3-P4 (community safety net) |
| **Depends-on** | K1–K6, F11 |

**Problem.** `rollout undo` is the community's documented safety net for a bad
deploy (w3-P4) — an operator's fastest rollback — but it's not in the original
conservative set. Research surfaced it as a first-class add (w1-D10): low-risk
(moves to an existing prior revision), audit-clean, and its own inverse is
another `rollout undo`.

**Design.** `rollout undo <ns>/<workload> [--to-revision=N]` → `kubectl rollout
undo`. **Revert = `command_pair`:** capture the pre-undo revision → inverse is
`rollout undo --to-revision=<pre-undo>`. `rollout status`/`rollout history`
surfaced as read sub-forms (or folded into `why`/`describe` — Phase-4 shape note:
keep `rollout` a small subcommand tree `undo`/`status`/`history` for kubectl
parity). Preview/prompt/audit echo resolved target (K5). **JP add-gate:** this
verb exists only if JP ratifies the Q4-add.

**Acceptance.**
- `k17_rollout_undo_moves_to_prior_or_named_revision`,
  `k17_rollout_undo_revert_returns_to_pre_undo_revision`,
  `k17_rollout_status_and_history_read_forms`,
  `k17_rollout_undo_dryrun_and_prompt_echo_resolved_target`.
- CHANGELOG (new verb); help (`LONG_ROLLOUT`); MANUAL/RUNBOOK. **5-surface
  sweep** all five.

**Research refs.** w1-D10; w3-P4. ⏸ Q1 + **Q4-add ratification required**.

---

### K18 — `delete pod` (narrow; `unsupported` revert + outage guard)

| Field | Value |
|---|---|
| **ID** | K18 |
| **Status** | ⏸ Proposed (Q1, Q4) |
| **Priority** | MEDIUM (narrow, guarded) |
| **Source** | SM §5.3; w1-D10, w3-P4/P1 |
| **Depends-on** | K1–K6, F11 |

**Problem.** `delete pod` is powerful and *not* undoable — deleting a
controller-owned pod triggers recreation (looks like a restart); the mental-model
mismatch is a footgun (w1 pain #9). It must be narrow, guarded, and honest about
irreversibility.

**Design.** `delete pod <ns>/<pod>` → `kubectl delete pod <p>` (narrow — pods
only, not controllers). **Revert = `Unsupported`** with the preview stating "the
controller will recreate this pod; deletion itself is not undoable" (w1-D10 —
CLI/non-dispatchable inverse ⇒ `Unsupported`, per the CLAUDE.md F11 capture-site
rule). **Outage guard:** if deleting the pod would drop ready replicas below the
deployment threshold (or it's a naked pod with no controller), **escalate the
confirmation** and warn (w3-P4/D5). Preview/prompt/audit echo resolved target
(K5) — the #1-horror-class guard (w3-P1).

**Acceptance.**
- `k18_delete_pod_is_narrow_pods_only`,
  `k18_delete_pod_revert_unsupported_with_recreation_note`,
  `k18_delete_pod_below_threshold_escalates_confirmation`,
  `k18_delete_naked_pod_warns_no_controller`,
  `k18_delete_pod_prompt_echoes_resolved_target`.
- CHANGELOG (new write verb + `unsupported` revert); help (`LONG_*` — the
  irreversibility + outage guard); MANUAL/RUNBOOK. **5-surface sweep** all five.

**Research refs.** SM §5.3; w1-D10; w3-P4/P1. ⏸ Q1, Q4.

---

### K19 — `exec --apply` (audited in-pod writes)

| Field | Value |
|---|---|
| **ID** | K19 |
| **Status** | ⏸ Proposed (Q1) |
| **Priority** | MEDIUM-HIGH (the escape hatch for legit in-pod debug writes) |
| **Source** | SM §5.1 (REUSE+ write); w3-P5 |
| **Depends-on** | K1–K6, K9, F11 |

**Problem.** Legit debugging sometimes needs a writing command inside a running
pod. `run` (K9) is read-only; the writing path must be `--apply`-gated + audited,
and it must still handle the distroless no-shell case (w3-P5).

**Design.** `exec <ns>/<pod> --apply -- <cmd>` → writing `kubectl exec` under the
`--apply` gate, audited (`AuditEntry` with resolved context/namespace, K5).
**Revert:** in-pod fs mutations are ephemeral/non-dispatchable ⇒ typically
`Unsupported` with a manual-inverse preview (or `state_snapshot` where a
meaningful before-state exists — Phase-4 leaves the per-command shape to the
capture site, per the F11 capture-site-authoritative rule). No-shell detection
(K4/K9) applies. Confirmation echoes resolved target (K5).

**Acceptance.**
- `k19_exec_apply_gated_and_audited`,
  `k19_exec_apply_records_resolved_context_in_audit`,
  `k19_exec_apply_revert_shape_per_capture_site`,
  `k19_exec_apply_no_shell_class_on_distroless`.
- CHANGELOG (write path + audit fields); help (`LONG_EXEC` k8s write note);
  MANUAL/RUNBOOK. **5-surface sweep** all five.

**Research refs.** SM §5.1; w3-P5; CLAUDE.md F11 capture-site contract. ⏸ Q1.

---

### K20 — REFUSE mappings (immutable-pod ops) + `port-forward`

| Field | Value |
|---|---|
| **ID** | K20 |
| **Status** | ⏸ Proposed (Q1, Q7) |
| **Priority** | MEDIUM (safety guardrails; each a chained hint) |
| **Source** | SM §5.3; w1-D11/D12, w3-P5 |
| **Depends-on** | K1–K6 |

**Problem.** Several docker write verbs have no safe k8s analogue: pod
filesystems are ephemeral/immutable, and some kubectl ops (`port-forward`,
`cp`) are fragile or anti-pattern. Silently accepting them would produce
non-persistent or broken results. Each must REFUSE with a chained hint pointing
at the correct idiom.

**Design.** On a k8s namespace, these REFUSE with a specific hint (never a raw
error):
- `edit` → "pods are immutable; edit the ConfigMap/Secret and `inspect restart
  <deploy>`" (rollout restart).
- `cp`/`put`/`get` → refuse; hint at ConfigMap/Volume or `kubectl cp` (which
  itself needs `tar` in-container, w1-D12) for the rare legit case.
- `chmod`/`chown`/`mkdir`/`touch`/`rm` → refuse with the immutability hint
  (in-pod fs mutation is an anti-pattern; `exec --apply` (K19) remains for legit
  debugging).
- `stop`/`start` → refuse; hint at `scale --replicas=0` (the documented idiom,
  with the outage warning) (w3-P4/D5).
- **`port-forward` (Q7)** → refuse-with-hint pointing at raw `kubectl
  port-forward` (documented-fragile: single connection, idle-drop, no reconnect —
  a poor fit for a headless audited verb) (w1-D11). Revisit as an owned verb only
  on field signal.

**Acceptance.**
- `k20_edit_refuses_with_configmap_hint`,
  `k20_cp_put_get_refuse_with_hint`,
  `k20_fs_mutation_verbs_refuse_with_immutability_hint`,
  `k20_stop_start_refuse_hint_scale_zero`,
  `k20_port_forward_refuses_with_kubectl_hint`.
- CHANGELOG (REFUSE mappings); help (each verb's `LONG_*` k8s REFUSE note +
  `inspect help kubernetes` immutability section); MANUAL. **5-surface sweep**
  all five.

**Research refs.** SM §5.3; w1-D11/D12; w3-P4/P5. ⏸ Q1, Q7.

---

## 5E. WAVE E — Integration + polish (K21–K25) — fully specified

Wave E ties the medium into the cross-cutting surfaces (fleet, search, bundles),
lands the help topic that makes the whole thing agent-legible, and closes the
release with a real-cluster smoke runbook.

### K21 — `fleet` mixed docker + k8s rollup

| Field | Value |
|---|---|
| **ID** | K21 |
| **Status** | ⏸ Proposed (Q1) |
| **Priority** | MEDIUM |
| **Source** | SM §5.4, roadmap "mixed fleet"; §1 no-change-for-docker |
| **Depends-on** | K1–K7 |

**Problem.** An operator with both docker and k8s namespaces wants one rollup, not
two tools. `fleet status` must show both mediums together (roadmap first-class
requirement).

**Design.** `fleet` iterates all configured namespaces regardless of `type`,
dispatching each through its Runtime (K1), and merges the health rollups into one
envelope — docker rows and k8s rows side by side, each tagged with its medium in
`meta`. No new flags; the medium is transparent.

**Acceptance.**
- `k21_fleet_status_merges_docker_and_k8s`,
  `k21_fleet_rows_tagged_with_medium`,
  `k21_fleet_unaffected_for_all_docker_config` (docker-only fleet unchanged).
- CHANGELOG; help (`fleet` `LONG_*` mixed-medium note); MANUAL. **5-surface
  sweep** all five.

**Research refs.** SM §5.4; roadmap mixed-fleet. ⏸ Q1.

---

### K22 — `search` across mediums

| Field | Value |
|---|---|
| **ID** | K22 |
| **Status** | ⏸ Proposed (Q1, Q5) |
| **Priority** | MEDIUM |
| **Source** | SM §5.1, §8; roadmap cross-medium search |
| **Depends-on** | K1–K8 |

**Problem.** LogQL `search` (`{server=~".*", source="logs"} |= "error"`) is
runtime-agnostic by design; it must span k8s log/state mediums too, and — per Q5
— `search` is the coverage path for resource kinds `describe` doesn't enumerate.

**Design.** The LogQL executor dispatches per-namespace through the Runtime (K1),
so a `{server=~".*"}` query fans across docker and k8s namespaces. k8s log/state
mediums plug into the existing `Medium` matching (the `source=` axis is unchanged
— k8s is the *runtime* axis, orthogonal). Secrets redacted (Q5).

**Acceptance.**
- `k22_search_spans_docker_and_k8s_namespaces`,
  `k22_search_k8s_logs_medium`,
  `k22_search_redacts_secrets`.
- CHANGELOG; help (`search` cross-medium note); MANUAL. **5-surface sweep** all
  five.

**Research refs.** SM §5.1/§8. ⏸ Q1, Q5.

---

### K23 — Bundle runtime-aware seam

| Field | Value |
|---|---|
| **ID** | K23 |
| **Status** | ⏸ Proposed (Q3) |
| **Priority** | MEDIUM |
| **Source** | SM §11; v0.1.4 charter (bundle integration); w3-D9 |
| **Depends-on** | K1–K6, K15–K19 |

**Problem.** The v0.1.4 charter wants bundle integration to make cross-medium
k8s+docker bundles *possible*; the v0.1.3 backlog puts mixed *composition* at
v0.2.0. The seam (a bundle step can target a k8s namespace) is the v0.1.4 line.

**Design.** Make the bundle executor **runtime-aware**: a bundle step resolves its
target namespace's Runtime (K1) and dispatches k8s verbs with per-step audit +
F11 revert, exactly like docker steps. **Boundary (stated, not silent):** a
single bundle file *mixing* docker and k8s steps (true cross-medium composition)
is **v0.2.0+** (SM §11, w3-D9); v0.1.4 ships the seam so a k8s-only bundle works
and the mixing is a small later step. **Q8 note:** if JP's field base is
mid-migration, mixed-composition priority rises — surfaced, not decided here.

**Acceptance.**
- `k23_bundle_step_targets_k8s_namespace`,
  `k23_bundle_k8s_step_audited_and_revertible`,
  `k23_mixed_medium_composition_is_bounded_to_v020` (a test/doc-asserted
  boundary, not a silent gap).
- CHANGELOG; help (`bundle` `LONG_*` k8s-step note); MANUAL/RUNBOOK bundle-seam
  internals. **5-surface sweep** all five.

**Research refs.** SM §11; w3-D9. ⏸ Q3 (+ Q8 informational).

---

### K24 — `kubernetes.md` help topic + `LONG_*` sweep

| Field | Value |
|---|---|
| **ID** | K24 |
| **Status** | ⏸ Proposed (all Q) |
| **Priority** | HIGH (help is first-class agent API — the load-bearing surface) |
| **Source** | CLAUDE.md help-discoverability; SM §8; every wave's help debt |
| **Depends-on** | K1–K23 |

**Problem.** Agents learn the CLI from `-h` first. The k8s medium adds selectors,
flags, failure classes, immutability refusals, and the anti-footgun property —
all must be self-describing, or the shell-is-the-integration-layer thesis breaks.

**Design.** New editorial topic `src/help/content/kubernetes.md` covering: the
`type="k8s"` config stanza; the 2-seg selector + `-n`/`-A`/`--context`; the
no-wrong-cluster guarantee (K5, sell it); the failure-class table (K4); pod
immutability + REFUSE idioms (K20); `--select` > jsonpath and `--color`/`NO_COLOR`
(w1-D5, w2-D8); `top` metrics dependency; the CrashLoop `--previous` workflow.
Plus the per-verb `LONG_*` k8s additions each wave item declared. **Raise the
help-search index cap** in `src/help/search.rs` if prose pushes it over
(precedent 50→64→80 KB — never trim docs).

**Acceptance.**
- `k24_help_kubernetes_topic_exists_and_indexed`,
  `k24_every_k8s_verb_long_has_k8s_section`,
  `k24_help_search_finds_k8s_contracts`,
  `k24_help_contract_test_passes_for_expanded_surface`.
- CHANGELOG; help (the topic itself); MANUAL cross-links. **5-surface sweep** all
  five (this item *is* mostly surface 5).

**Research refs.** CLAUDE.md help discipline; SM §8; w1-D5; w2-D8. ⏸ all Q.

---

### K25 — Smoke runbook vs a real cluster (`SMOKE_v0.1.4.md`)

| Field | Value |
|---|---|
| **ID** | K25 |
| **Status** | ⏸ Proposed (all Q) |
| **Priority** | HIGH (the field-validation gate — the tag blocker) |
| **Source** | v0.1.3 smoke precedent; the P1–P8 field scenarios |
| **Depends-on** | K1–K24 |

**Problem.** Unit tests use synthetic fixtures; the v0.1.3 precedent is a real-host
smoke that reproduces the actual field scenarios. v0.1.4 needs the analogue
against a **real k8s cluster** (kind/minikube/EKS) before tag.

**Design.** `SMOKE_v0.1.4.md` — a phased runbook (P1→Pn, mirroring
`SMOKE_v0.1.3.md`) that reproduces the research pain scenarios end-to-end:
wrong-context **immunity** (K5), RBAC-forbidden **hint** (K4/K6), CrashLoop
`--previous` **auto-hint** (K8), `--merged` **replica tagging** (K8), `top`
metrics-absent **degrade** (K13), `events` **ordering** (K12), `scale`/`restart`
**revert round-trip** (K15/K16), distroless **no-shell class** (K9/K19), `delete
pod` **outage guard** (K18). All writes labelled/scoped to a smoke namespace;
cleanup idempotent (v0.1.3 smoke-scope discipline).

**Acceptance.** The runbook is the artifact; its P-phases are the acceptance.
Gate: a clean P1→Pn PASS against a real cluster (the field-validation gate in §6).
CHANGELOG note (smoke runbook added). **5-surface sweep:** doc-centric; RUNBOOK
cross-link.

**Research refs.** all of P1–P8; v0.1.3 smoke precedent. ⏸ all Q.

---

## 6. Release-readiness gate (all must be green to tag v0.1.4)

Mirrors the v0.1.3 all-green-to-tag gate, retargeted to the k8s surface.

**Test + code gates**
- All 25 K-items (K1–K25) have a passing `k<n>_*` test (or bundle) in
  `tests/phase_k_v014.rs`.
- `tests/no_dead_code.rs` + `tests/help_contract.rs` pass against the expanded
  surface, with each addition enumerated inline (v0.1.3-gate style): the new
  verbs (`describe`, `events`, `top`, `scale`, `rollout` tree, `delete pod`);
  the new flags (`-c`/`--container`, `--previous`, `--merged`, `-n`/`--namespace`,
  `-A`/`--all-namespaces`, `--context`, `--kubeconfig`, `--current-replicas`,
  `--to-revision`, `--color`); the config fields (`type`/`kubeconfig`/`context`/
  `namespace`, `schema_version` bump); the `AuditEntry` additive fields
  (`context`/`k8s_namespace`); the new `failure_class` value family
  (`rbac_forbidden`/`no_shell_in_container`/`metrics_unavailable`/k8s-transport);
  the k8s transport exit-code mapping.
- The **docker regression suite stays 100% green** — K1 is a behavior-preserving
  refactor; this is the **additive-purity gate** (docker users see zero change,
  per §1 and bible "purely additive").
- `cargo fmt --check` + `cargo clippy --all-targets -D warnings` + full `cargo
  test` green on every commit; the deferral-scan grep clean.

**Doc + help gates**
- One CHANGELOG bullet per K-item under a v0.1.4 `Added` section; behavior /
  audit-schema / exit-code / config-schema changes explicitly flagged (K2 config
  schema; K4 failure-class values; K5 audit fields; K8 logs behavior; K15/K16/K18
  revert kinds).
- `docs/MANUAL.md` sections enumerated per item (Kubernetes namespaces + config;
  addressing + the no-wrong-cluster guarantee; k8s status/logs/why/describe/
  events/top; the k8s write surface + revert; REFUSE idioms). `docs/RUNBOOK.md`
  updated for the runtime-abstraction, failure-classifier, merged-fan-out +
  heartbeat, and bundle-seam internals.
- `inspect help kubernetes` (K24) exists + indexed; every k8s verb's `LONG_*`
  carries its k8s section; help-search index cap raised if needed (never trim).

**Field-validation gate (distinct from unit tests — the real-cluster smoke)**
- `SMOKE_v0.1.4.md` (K25) P1→Pn PASS against a **real k8s cluster**, reproducing:
  wrong-context **immunity** (K5); RBAC-forbidden **four-question hint** (K4/K6);
  CrashLoop `--previous` **auto-hint** (K8); `--merged` **replica tagging** (K8);
  `top` metrics-absent **degrade** (K13); `events` **newest-first ordering**
  (K12); `scale` + `restart` **revert round-trip** (K15/K16); distroless
  **no-shell class** (K9/K19); `delete pod` **outage guard** (K18); a k8s-only
  **bundle step** audited + revertible (K23).
- **You** (Claude Code, release session) drive this smoke against a real cluster
  at release time — per the bible operating-context, any bug shipped is one you
  step on personally.

**Process gate**
- JP has ratified Q1–Q8 (all ⏸ Proposed items promoted to their final status);
  no item ships against an unratified question.
- **Full-sweep audit gate (JP-scheduled, ROOT-dispatched).** After Wave E and
  this release-readiness gate both pass with every kube verb live-tested, the
  program undergoes a **full cross-repo sweep audit** per
  `/home/jpbeaudet/luminary/docs/audits/CROSS_REPO_AUDIT_TEMPLATE.md` (including
  the sacred **S**=sovereign / **C**=capability-first / **O**=no-overfit
  dimensions), dispatched by ROOT (not the sub-coordinator). **v0.1.4 does NOT
  tag/release before that audit reaches 0-Critical / 0-High.** The
  sub-coordinator's role is to declare **PROGRAM-READY-FOR-AUDIT** once the
  preconditions hold (Waves A–E complete, every verb live-tested, all Wave-A..E
  findings closed, gates green); ROOT then dispatches the audit.
- Cleaning Duty run (strip `K<n>`/`(v0.1.4)` markers to industry-grade prose;
  test names + CHANGELOG keep theirs); archive sweep (planning docs →
  `archives/v0.1.4/`); README freshness; CLAUDE.md pivot to v0.1.5.

---

*Wave A specified. Next save-point turns: Wave B (K6–K9), Wave C (K10–K14),
Wave D (K15–K20), Wave E (K21–K25), each fully specified; then the CLAUDE.md
amendment (Deliverable 2) folding the §3 bible-interaction flags; save-point
marked DESIGN-COMPLETE.*
