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

| Wave | Theme | Items | Gates |
|---|---|---|---|
| **A** | Foundation: runtime abstraction, config, backend probe, failure taxonomy, context-pinning | K1–K5 | Blocks all others. |
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

## 6. Release-readiness gate (skeleton — filled as waves complete)

Mirrors the v0.1.3 all-green-to-tag gate. To be expanded per item as Waves B–E
are specified. Fixed structure:
- Every K-item has a passing `k<n>_*` test in `tests/phase_k_v014.rs`.
- `tests/no_dead_code.rs` + `tests/help_contract.rs` pass against the expanded
  verb surface (each new verb/flag/JSON-field/exit-class/`failure_class` value
  enumerated inline, v0.1.3-gate style).
- The **docker regression suite stays 100% green** (K1 is a behavior-preserving
  refactor) — the additive-purity gate.
- MANUAL + RUNBOOK sections enumerated per item; one CHANGELOG bullet per item.
- **Field-validation gate:** end-to-end smoke (`SMOKE_v0.1.4.md`, K25) against a
  **real k8s cluster** reproducing the P1–P8 scenarios (wrong-context immunity,
  RBAC-forbidden hint, CrashLoop `--previous` auto-hint, `--merged` replica
  tagging, `top` metrics-absent degrade, `events` ordering, `scale`/`rollout`
  revert round-trip, distroless no-shell class).

---

*Wave A specified. Next save-point turns: Wave B (K6–K9), Wave C (K10–K14),
Wave D (K15–K20), Wave E (K21–K25), each fully specified; then the CLAUDE.md
amendment (Deliverable 2) folding the §3 bible-interaction flags; save-point
marked DESIGN-COMPLETE.*
