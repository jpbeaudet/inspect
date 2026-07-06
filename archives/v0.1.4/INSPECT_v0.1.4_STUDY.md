# INSPECT v0.1.4 — Program Study (Phase 1)

**Author:** inspect-subcoord (agentic program, JP-commissioned)
**Date:** 2026-07-04
**Branch:** `feat/v0.1.4-program`
**Status:** Phase 1 of 5 — STUDY complete. Feeds Phase 2 (surface map) → Phase 3 (research) → Phase 4 (deep design) → Phase 5+ (implement in waves).

This document reconstructs, from the repo itself, (1) what `inspect` is
and how it is architected, (2) the **backlog process** that delivered
v0.1.2 / v0.1.3, and (3) the pre-existing Kubernetes design seed and its
re-sequencing into v0.1.4. It is the working substrate for the v0.1.4
Kube program: a fresh conversation can open on this doc and continue.

---

## 1. What `inspect` is (mission + philosophy)

`inspect` is an **agentic-first-class SRE CLI** in Rust — an operational
debugging tool for fleets of servers reached over SSH. Four capability
axes: **search** (logs/config across machines via a LogQL-style query
language), **diagnose** (what's running + `why`), **safely apply**
(dry-run by default, `--apply`, full audit + universal revert), and
**orchestrate** (declarative YAML bundles with rollback).

Current tag: **v0.1.3** (2026-05-10), plus an accumulating
`v0.1.3.1` patch backlog. This program builds **v0.1.4 = the Kube
release**.

### Load-bearing philosophy (governs every design decision below)

- **The shell is the integration layer. No MCP server required — and
  writing one is an anti-pattern.** Every shape an MCP tool could
  expose is already a composable verb whose contract is discoverable
  from `-h` and stable across the `0.1.x → 0.2.0` window. The design
  test for any new verb: *can an LLM with a shell tool +
  `inspect <verb> --help` learn this contract cold and chain it to the
  next action without an external adapter?* If no, the design has
  drifted. (CLAUDE.md, Operating context.)
- **Agentic callers are a first-class consumer**, co-equal with human
  operators: stable JSON envelopes (`{schema_version, summary, data,
  next, meta}`), explicit exit-code classes, redacted stdout, chained
  `hint:` / `see: inspect help <topic>` recovery trailers, in-binary
  `--select '<jq>'` projection (F19, via `jaq`).
- **Production grade only.** No stubs, no "good enough for a demo", no
  `.unwrap()` on a fallible boundary, no silent deferrals. Every commit
  leaves the tree releasable.
- **No backward compatibility until v0.2.0.** v0.1.x is the last window
  to break CLI flags, JSON schemas, config formats, audit fields,
  exit-code semantics. Migration shims are debt — fix the design.

### Exit-code contract (stable)

`0` ok · `1` no-match · `2` usage/arg error · `12–14` transport (F13)
· inner exit-code passthrough on `run`/`exec`. Never reuse a code.

---

## 2. Architecture map (where k8s must plug in)

### 2.1 Source layout (`src/`)

| Area | Modules | Role |
|---|---|---|
| CLI surface | `cli.rs` (verb enum `Command`, per-verb `*Args`, `LONG_*` help constants), `main.rs`, `commands/` | Clap definitions + dispatch entry. **~50 verbs** today (see §2.2). |
| Read verbs | `verbs/` (`status`, `health`, `logs`, `grep`, `cat`, `ls`, `find`, `ps`, `volumes`, `images`, `network`, `ports`, `watch`, `run`, `cache`, `correlation`, …) | Diagnostic + read surface. |
| Write verbs | `verbs/write/` (`chmod`, `chown`, `mkdir`, `touch`, `rm`, `edit`, `exec`, `atomic`, `lifecycle`) | Mutating surface; every one captures an F11 revert. |
| Compose | `verbs/compose/` | First-class `inspect compose` tree (F6/L8). |
| Bundles | `bundle/` | Declarative YAML orchestration (B9), matrix, per-branch rollback (L6). |
| Discovery | `discovery/probes.rs` | **Builds `docker ps` / `docker inspect` / tool-probe commands directly.** The primary docker-coupling site. |
| Dispatch/exec | `exec/` (`dispatch.rs`, `medium.rs`, `cancel.rs`, `reader/`) | Command dispatch + reauth (`dispatch_with_reauth`), SIGPIPE handling, streaming. |
| SSH | `ssh/` (`master.rs`, `precheck.rs`) | ControlMaster reuse, interactive auth retry loop, `accept-new`. |
| Profile/config | `profile/` (`schema.rs`, `cache.rs`, `runtime.rs`), `config/` | `~/.inspect/servers.toml`, cached namespace profiles, `Service` / `ServiceKind` model. |
| Query | `logql/`, `query/`, `selector/` | LogQL parser, `jaq` `--select` projection, selector grammar. |
| Safety | `safety/audit.rs` | `AuditEntry` (freeze-bound in v0.2.0), revert enum. |
| Redaction | `redact/` | 4-masker family (header/PEM/URL/env), P1 adds env-suffix. |
| Help | `help/`, `help/content/*.md` | First-class agent API. Editorial topics + search index (size cap in `help/search.rs`). |
| Format | `format/` | 10 output formats, envelope rendering, templating. |

### 2.2 Verb surface (from `cli.rs` `Command` enum)

Namespace mgmt: `add · list · remove · test · show`. Connections:
`connect · disconnect · connections · disconnect-all · ssh · keychain`.
Discovery: `setup`/`discover · profile`. Read diagnostics: `status ·
health · logs · grep · cat · ls · find · ps · volumes · images ·
network · ports · why · connectivity · recipe · search · watch · run`.
Write/lifecycle: `restart · stop · start · reload · cp · exec · edit ·
chmod · chown · mkdir · touch · rm · atomic`. Orchestration + audit:
`bundle · audit · revert · history · cache · compose · fleet`.

### 2.3 The docker-coupling seam (critical for k8s design)

**There is no runtime-abstraction trait today.** The name `Medium`
(`exec/medium.rs`) is a **`source=` label parser** (logs / file:<p> /
dir:<p> / discovery / state / volume / image / network / host:<p>) — it
is the *what to read* axis, **not** the *what runtime* axis. Docker is
assumed structurally in two places:

1. **`discovery/probes.rs`** builds literal `docker ps --format …` and
   `docker inspect` invocations, three-bucket-classifies their timeouts,
   and probes host tools (`rg jq journalctl sed grep netstat ss
   systemctl docker`).
2. **`verbs/dispatch.rs`** resolves a selector to a container name and
   builds `docker logs|exec|restart|stop|start|kill <ctr>`.

`Service` (`profile/schema.rs`) has a `ServiceKind` enum
(`Container | Systemd | HostListener`) and a `container_name` field
that is "always the value passed to `docker logs|exec|…`". The compose
layer already distinguishes service-name vs container_name (F5).

**Implication for v0.1.4:** k8s is a *new runtime medium* introduced at
the **namespace level** (a `type = "docker" | "k8s"` field on the
namespace, per the roadmap seed), which swaps the command-building layer
from `docker …` to `kubectl …` (backend decision — shell-out vs
`kube-rs` — is a Phase 4 design question). The selector grammar, the
verb surface, the JSON envelope, the audit/revert contract, and the help
system are all **runtime-agnostic** and should stay that way — k8s
reuses them. This is exactly the roadmap's "Executor trait:
`DockerExecutor` and `K8sExecutor` implement the same interface" intent,
which does not yet exist and is the central architectural build of
v0.1.4.

---

## 3. The backlog PROCESS (reconstructed from v0.1.2 / v0.1.3 / v0.1.3.1)

This is the delivery machine v0.1.4 must mirror. Reconstructed from
`archives/v0.1.2/`, `archives/v0.1.3/`, `INSPECT_v0.1.3.1_PATCH_BACKLOG.md`,
and CLAUDE.md.

### 3.1 Lifecycle: intake → itemize → deliver → release → archive

1. **Intake.** Work originates as either **field feedback** (real
   operator pain → `F<n>` items) or **pre-existing roadmap limitations**
   (`L<n>` items) or **stabilization** (`S<n>`) or **post-tag patches**
   (`P<n>`). Intake docs feed the backlog: for v0.1.3 these were
   `INSPECT_ROADMAP_TO_v01.3.md`, `INSPECT_v013_PAIN_POINT_AUDIT.md`
   (F-items), `INSPECT_v013_SECURITY_AUDIT.md` (S-items). **Do not
   conflate prefixes** — `F<n>` and `L<n>` live in different sections
   and ship in different orders.
2. **Itemize.** Each item is a numbered block with a fixed shape
   (see §3.2). The backlog is a single root-level file
   `INSPECT_v<X.Y.Z>_BACKLOG.md` that is **FROZEN in scope** once
   agreed — no mid-implementation scope creep; creep surfaces as a
   question to JP. (v0.1.3 did legitimately expand 25→31 when the
   no-silent-deferrals policy exposed six features earlier commits had
   quietly punted to a non-existent v0.1.5, plus F19 promotion — each
   expansion was justified in-doc, not silent.)
3. **Deliver.** **One commit per backlog item** (never bundled; a
   side-find becomes its own commit). Subject `<ID>: <desc>`; body =
   sub-section breakdown (modules, API additions, audit-shape changes,
   help changes, test counts); footer `Closes <ID> in <backlog>.` +
   `Co-Authored-By:`. Marker convention `F<n>`/`L<n>`/`(vX.Y.Z)` on
   every new comment/docstring throughout the window for cross-file
   traceability. All work on a feature branch — never edit `main`
   directly.
4. **The mandatory 5-surface update sweep** (per closed item — missing
   one is a *policy violation*, not a follow-up):
   1. **Source + tests** — acceptance tests in the phase file
      (`tests/phase_f_v013.rs`), named `<id>_*` (`f14_*`, `l7_*`).
      Every sub-item ≥ 1 test.
   2. **`CHANGELOG.md`** — new bullet atop the version's `Added`
      section; verbose, field-feedback-quoted; behavior/audit/exit-code
      changes explicitly flagged.
   3. **Backlog row** — mark `✅ Done`, replace Notes placeholder with a
      technical summary (modules, flags, test counts, docs touched).
   4. **`docs/MANUAL.md`** (+ `docs/RUNBOOK.md` when applicable) — new
      user-facing contracts get a section; the release-readiness gate
      enumerates which sections each item must touch.
   5. **Interactive `-h` help** — the load-bearing surface (agents learn
      the CLI from `-h` first). Every flag → clap docstring; every JSON
      field / exit code / state value → the verb's `LONG_*` constant or
      an editorial topic in `src/help/content/`.
5. **Release.** Pre-commit gate (`cargo fmt --check` → `cargo clippy
   --all-targets -- -D warnings` → `cargo test`) green on every commit;
   the deferral-scan grep clean. Then: final item shipped + smoke green
   → **Cleaning Duty** (strip `F<n>`/`L<n>`/`(vX.Y.Z)` markers to
   industry-grade prose, per `CLEANING_DUTY.md`; test names + CHANGELOG
   keep their markers as archaeology IDs) → archive sweep (closed
   planning docs → `archives/v<X.Y.Z>/`) → README freshness → CLAUDE.md
   pivot to next release → tag/build/publish (`RELEASING.md`:
   tag-and-push triggers the release workflow).

### 3.2 The per-item block shape (mirror this exactly)

From v0.1.3.1 P-items and v0.1.3 F/L-items, each item carries:

- A `### <ID> — <one-line title>` heading.
- A metadata table: **ID · Status · Priority · Source · Surfaced via**.
- **Problem** — written in the operator's voice, quoting the field user
  where useful; concrete reproduction of the failure.
- **Proposed fix** — concrete module names + the actual API/flag
  additions.
- **Acceptance** — named acceptance tests (`<id>_*`), CHANGELOG entry,
  help-surface additions, doc sections.
- **Notes / Open question** — surfaced explicitly, never silently
  resolved.

Status legend: `🟦 Open · 🟧 In progress · ✅ Done · 🟥 Bumped/Deferred
(authorized) · 🟦 Deferred (authorized)`.

### 3.3 The release-readiness gate (the tag blocker)

v0.1.3's gate is the template: **all-green-to-tag** checklist covering
(a) every item has a passing phase-file test; (b) the dead-code +
help-contract tests pass against the expanded verb surface, with **each
new flag / subcommand / JSON field / exit code enumerated inline**;
(c) MANUAL + RUNBOOK sections enumerated per item; (d) one CHANGELOG
bullet per item with behavior/schema/exit-code flags called out; (e) a
set of **end-to-end smoke tests against a real host** that reproduce the
original field scenarios (the field-validation gate, distinct from unit
tests). v0.1.4 needs the analogous gate, retargeted to a real k8s
cluster.

### 3.4 No-silent-deferrals discipline (governs the whole program)

"deferred / out of scope / postponed / punted / TODO / stub /
unimplemented" in committed prose or code is a **policy violation**
unless it either points at a *future* release (`v0.1.5+`, `v0.2.0+`) or
is an entry in the **Authorized deferrals** registry (requires JP's
explicit "ok defer X"; agents never self-authorize). Sequencing an item
later *within* the release is not deferral. A pre-commit grep enforces
this over `src/ docs/ CHANGELOG.md` + the active backlog. **v0.1.4
inherits this unchanged** — anything docker/compose/SSH that doesn't fit
k8s does not get "out of scope for v0.1.4"'d; it either isn't in scope
(a boundary, stated as such against a real future release) or it ships.

### 3.5 Working-doc location + naming convention

Active-cycle planning artifacts live at **repo root**, named
`INSPECT_v<X.Y.Z>_<KIND>.md` (`_BACKLOG`, `_ROADMAP`, `_PAIN_POINT_AUDIT`,
`_SECURITY_AUDIT`, `_JAQ_PLAN`, `SMOKE_v<X.Y.Z>.md`). At release they
move **unmodified** into `archives/v<X.Y.Z>/` with a short `README.md`
index. Therefore this program's docs are root-level `INSPECT_v0.1.4_*.md`
(this study, the coming surface map, the backlog, the smoke runbook),
archived to `archives/v0.1.4/` at tag time.

---

## 4. Pre-existing Kubernetes design seed

Kubernetes is **not greenfield** — a design sketch already exists and is
authoritative starting material.

### 4.1 The roadmap seed (`INSPECT_ROADMAP_TO_v01.3.md`, "v0.2.0 Part 2")

The original roadmap slotted k8s at v0.2.0. Its sketch (now pulled
forward to v0.1.4) specifies:

- **Namespace gains a `type` field:** `"docker"` (default) or `"k8s"`.
  k8s config carries `kubeconfig`, `context`, `namespace`.
- **Executor trait:** `DockerExecutor` and `K8sExecutor` implement the
  same interface. Selectors, verbs, JSON, audit, help stay identical.
- **Same selectors:** `staging-k8s/api` resolves like `arte/api`.
- **Same verbs reused:** `logs · grep · run · exec · status · health ·
  why · watch · ps`. Mixed fleet: `inspect fleet status` shows docker +
  k8s together; `inspect search` spans both.
- **k8s-specific command mappings:** discovery via `kubectl get
  pods/services/deployments/configmaps -o json`; `logs` via `kubectl
  logs` with `-c` for multi-container pods; `run` = read-only `kubectl
  exec`; `exec --apply` = writing `kubectl exec` (audited); `--merged`
  fans `kubectl logs -f` across all replicas of a Deployment; `why` =
  Pod conditions + Events + restart counts + dependency probing.
- **Immutability guardrail:** `edit`/`cp` on pods **refuse with a hint**
  ("edit the ConfigMap/Secret and `kubectl rollout restart`"). Pods are
  immutable.
- **`setup`:** kubeconfig context check + `kubectl auth can-i` RBAC
  self-test.
- **Auth:** inherit kubeconfig; `--context` / `--kubeconfig` flags. **No
  new credential surface.**
- **Backend decision:** `kube-rs` (type-safe, no kubectl dependency) vs
  shell out to `kubectl` (simpler, universal, agent-legible). "Can ship
  with `kubectl` backend first." **Phase 4 will decide** — note the
  Dependency Policy (prefer native; a heavy generated-client crate is a
  real decision) and the shell-is-the-integration-layer philosophy both
  bear on this.
- **Per-user policies** (deferred from v0.1.3): `allow`/`deny` verb
  lists per namespace, `require_reason`, RBAC-aware.
- **No-change guarantee for docker users:** `type = "docker"` default;
  existing configs/selectors/aliases/recipes/bundles all unchanged. k8s
  is purely additive.

### 4.2 The v0.1.3-backlog scope statement for v0.1.4 (authoritative)

The v0.1.3 backlog's closing "Next step" paragraph explicitly charters
v0.1.4 (this program):

> open `INSPECT_v0.1.4_BACKLOG.md` covering the **Kubernetes release** —
> k8s medium implementation, k8s-aware selectors
> (`<ctx>/<namespace>/<workload>`), kubectl-equivalent read verbs
> (`logs`, `describe`, `events`, `top`), kubectl-equivalent write verbs
> scoped conservatively (`scale`, `restart`, `delete pod` with audit),
> and the bundle-engine integration so cross-medium k8s+docker bundles
> become possible.

Note the selector-grammar detail here (`<ctx>/<namespace>/<workload>`,
a three-segment form) refines the roadmap's two-segment
`staging-k8s/api` — reconciling the selector shape is a Phase 2 map
question.

### 4.3 Existing kube *mentions* in the tree (all forward-looking notes)

`grep -ril kube` hits are help-content examples, roadmap/backlog
forward-notes, `discovery/probes.rs` (docker probe, mentions k8s only in
comments), and CLAUDE.md's scope statement. **No k8s code exists.** The
tree is clean for a from-scratch runtime medium.

### 4.4 Scope boundaries already fixed (NOT v0.1.4)

- **Cross-medium bundles** (docker + k8s steps in one bundle) are
  **v0.2.0+** per the v0.1.3 backlog "Not part of the contract" list —
  *however* the v0.1.4 charter paragraph (§4.2) says bundle-engine
  integration should make cross-medium bundles *possible*. This is a
  **reconciliation item for Phase 2/4** (build the seam in v0.1.4 vs
  ship cross-medium composition in v0.1.4). Surface to JP, do not
  silently pick.
- **TUI write actions, alias defaults, themes/plugins** — unrelated,
  v0.2.0+ / permanent boundaries.

---

## 5. Divergence note (roadmap vs bible — resolved)

The v0.1.3-era `INSPECT_ROADMAP_TO_v01.3.md` sequenced **v0.1.4 =
pre-stabilization cleanup (S1–S7)** and **v0.2.0 = Kubernetes**. The
current authoritative CLAUDE.md bible **re-sequenced** this:

> v0.1.4 = Kubernetes only. v0.1.5 = stabilization sweep. v0.2.0 =
> contract freeze.

The v0.1.3 backlog tail (§4.2) confirms the re-sequencing and moves the
S1–S7 stabilization sweep to `INSPECT_v0.1.5_BACKLOG.md`. **The bible +
backlog-tail win.** v0.1.4 is Kubernetes; the roadmap doc's v0.1.4/v0.2.0
sections are superseded on sequencing but their *content* (S1–S7 list →
v0.1.5; k8s sketch → v0.1.4) remains the source material. Phase 4 amends
CLAUDE.md and, if useful, renames/updates the roadmap doc to the
`v0.1.3 → v0.1.4 (k8s) → v0.1.5 (stabilization) → v0.2.0 (contract)`
sequence (the backlog tail explicitly requests this).

---

## 6. Open threads carried into Phase 2 (surface map)

1. **Selector grammar reconciliation** — roadmap `staging-k8s/api`
   (2-seg, context-as-namespace) vs backlog-tail
   `<ctx>/<namespace>/<workload>` (3-seg). Which is the v0.1.4 shape,
   and how does it coexist with docker's existing `<ns>/<svc>` and the
   `source=` label grammar?
2. **Backend decision framing** — shell-out `kubectl` vs `kube-rs`,
   weighed against Dependency Policy + shell-is-integration-layer +
   binary-size. (Decision lands in Phase 4, but Phase 2 must map which
   verbs each backend option cleanly serves.)
3. **Verb-by-verb mapping** — which existing verbs reuse verbatim, which
   need k8s command-building, which are new (`describe`, `events`,
   `top`, `scale`, `rollout restart`, `delete pod`), which refuse
   (`edit`/`cp` on pods).
4. **Runtime-abstraction shape** — introduce the executor trait /
   `type` field where, touching which modules (`discovery/probes.rs`,
   `verbs/dispatch.rs`, `profile/schema.rs`, `config/`, `exec/`).
5. **Cross-medium bundle seam** — build-the-seam vs ship-composition
   (§4.4). JP decision.
6. **k8s auth model** — kubeconfig inheritance vs inspect's SSH/keychain
   credential surface; the "no new credential surface" guarantee.
7. **What the JSON envelope + revert contract look like for k8s** —
   `scale`/`restart`/`delete pod` reverts (F11 `revert.kind`), and
   whether pod deletion is even revertible (likely `unsupported` with a
   manual-hint, like CLI-only inverses).

---

## 7. Process compliance for this program

- Branch: `feat/v0.1.4-program` (all program work; workers commit
  locally + frequently, never >15min uncommitted; sub-coord PR-posts
  completed waves; **JP/root merges — never the sub-coord**).
- Every wave: `export PATH=$HOME/.cargo/bin:$PATH` then
  `cargo check → clippy --all-targets -D warnings → test` before the
  wave closes; deferral-scan grep clean.
- Each shipped item runs the 5-surface sweep (§3.1.4).
- git-coord claim `8f118fd393f8` heartbeat every turn; tessaract every
  turn.
- Save-point report ends every turn.

---

*Phase 1 complete. Next: Phase 2 — MAP the full k8s verb + surface set,
committed as `INSPECT_v0.1.4_SURFACE_MAP.md`.*
