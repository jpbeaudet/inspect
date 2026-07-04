# INSPECT v0.1.4 — Research Synthesis (Phase 3 integration)

**Author:** inspect-subcoord · **Branch:** `feat/v0.1.4-program` · **Date:** 2026-07-04

This doc **integrates** the raw Phase-3 dossiers in `INSPECT_v0.1.4_RESEARCH/`
into actionable **deltas** against the surface map (`INSPECT_v0.1.4_SURFACE_MAP.md`).
It is built **one dossier per turn** (protocol: no double-dispatch, incremental
commit). At the Phase-3 synthesis turn the *confirmed* deltas fold into the
surface map and seed the Phase-4 backlog (`K<n>` items) + the CLAUDE.md
amendments.

Delta legend: **✓VALIDATES** (map is right, keep) · **+ADD** (add a
flag/behavior/item) · **⚑JP** (new/changed decision needing JP) · **⚙WAVE**
(assign to a Phase-4 wave).

Integration status: **w1 ✓ done** · w2 pending · w3 pending · synthesis pending.

---

## Batch w1 — kubectl + k9s (dossier: `kubectl_k9s.md`)

Bottom line: **all three tested recommendations (Q1 selector, Q2 backend, Q4
write set) VALIDATED.** kubectl is the idiom agents already know; its worst
footguns are exactly the class our `-h`-first + chained-hint + `failure_class`
discipline fixes; k9s's own issue tracker is evidence that a JSON verb surface
beats a TUI for our audience.

### w1-D1 — `logs` multi-container: auto-pick + hint (out-do kubectl)
**+ADD / ✓VALIDATES.** kubectl `logs <pod>` *errors* on multi-container pods
without `-c` (kubectl#371, "the first-hour gotcha"). Our `logs` keeps `-c`, but
should **default to the first container with a chained hint listing the others**
— the auto-pick kubectl refused to ship. Higher parity value than kubectl.
⚙WAVE B.

### w1-D2 — crashed-container `--previous` auto-hint (differentiator)
**+ADD.** `--previous`/`-p` is undiscoverable exactly when needed (crash-loop →
current logs empty). Keep the `--previous` flag, **and** make `why`/`status`
auto-emit *"container crashed (restartCount=N) — try `inspect logs … --previous`"*
whenever `restartCount>0`. A genuine differentiator. ⚙WAVE B (flag) + C (`why`).

### w1-D3 — container exit-code + reason surfaced in `why`/`status` data
**+ADD (new, not yet in map).** Container exit codes (137 OOMKilled / 143
SIGTERM / 139 SIGSEGV) live only in
`.status.containerStatuses[].lastState.terminated.exitCode` — agents must
jsonpath for them. `why`/`status` should surface
`lastState.terminated.{exitCode,reason}` directly in `data`. High-leverage.
⚙WAVE C.

### w1-D4 — `describe` → envelope is an improvement, not just parity
**✓VALIDATES.** kubectl `describe` has **no `-o json`** (its richest view —
events+conditions — is text-only). Reshaping describe's spec+events+conditions
into `data` is genuinely better than kubectl. Keep `describe` NEW. ⚙WAVE C.

### w1-D5 — `--select` (jaq) is strictly better than kubectl jsonpath — sell it
**✓VALIDATES.** kubectl jsonpath has no regex and breaks on quoting, forcing
`-o json | jq`. Our in-binary `--select` (jaq over the envelope) is strictly
better. Document this explicitly as a selling point in the new `kubernetes.md`
help topic. ⚙WAVE E (help).

### w1-D6 — `events`/`top` verb-owned ordering + degrade
**✓VALIDATES.** `events` newest-first *in the verb* (mirrors `audit ls`) so
agents never touch the brittle `--sort-by='{...}'` brace form (which even
shipped a wrong example, k8s#21018). `top` degrades with a CI-gate-quality
"metrics-server not installed" error. Keep both as mapped. ⚙WAVE C.

### w1-D7 — `failure_class` closes kubectl's exit-1 conflation (Q2 caveat)
**✓VALIDATES + ⚙WAVE A cost.** kubectl collapses NotFound / Forbidden /
Unreachable / TLS-expired all into exit `1`, distinction only in stderr prose.
Our `failure_class` + 12–14 transport taxonomy (SM §10) closes this real gap —
**but the shell-out backend must parse kubectl stderr** to build the class.
That stderr-classification is the one non-trivial cost of shell-out vs a typed
client; **budget it as a Wave-A task.**

### w1-D8 — Runtime-trait invariants from k9s's shell-out bugs
**✓VALIDATES + ⚙WAVE A.** k9s's issue tracker yields three hard invariants for
our kubectl backend:
1. **Probe `kubectl` on PATH like we probe `docker`** (k9s kubectl-not-found
   breaks exec). Fail loud + actionable. **Non-negotiable Wave-A.**
2. **Thread `--kubeconfig`/`--context` to EVERY shell-out**, not just discovery
   (k9s#195: `--kubeconfig` made views work but broke exec). Make it a
   Runtime-trait invariant **with a test**.
3. **Don't hardcode a shell / don't gate on unneeded RBAC.** Our
   `run`=`kubectl exec -- <cmd>` (no `/bin/bash` wrapper) already avoids the
   Alpine-no-bash trap (k9s#1428) — **VALIDATES** that choice. `auth can-i`
   self-test must probe the *actual* verbs inspect runs, not a superset
   (k9s#3583 gated exec on node-reads it didn't need) — ties to CLAUDE.md
   security-scope-narrower rule.

### w1-D9 — Q1 (2-seg selector + `-n`): VALIDATED strongly, one refinement
**✓VALIDATES.** kubectl puts *connection* (context/cluster/auth) in
**kubeconfig**, not on the command line — exactly our "context+ns in config like
the SSH host" argument. A 3-segment selector would spread cluster-addressing
into the parser + every call site — *more* surface for the k9s#195 class of
"addressing not threaded" bug. **Refinement (+ADD):** `-n` on a cluster-scoped
kind (nodes/PVs) should be **ignored-with-a-note**, not error — mirror kubectl
but *say so* in `-n` help text.

### w1-D10 — Q4 write set: VALIDATED + resolves the SM §5.3 revert open-question
**✓VALIDATES + ⚑JP.**
- `scale` = cleanest revertible write (capture prior `--replicas`, inverse =
  scale back). kubectl's `--current-replicas=N` optimistic-concurrency guard is
  worth adopting for a safer apply. ⚙WAVE D.
- **Resolves SM §5.3 open question:** `restart` (rollout restart) revert should
  be **`command_pair`, not `unsupported`** — capture `rollout history` revision
  before restart, inverse = `rollout undo --to-revision=<captured>` (dispatchable
  locally as `kubectl`). ⚙WAVE D.
- `delete pod` → `unsupported` **VALIDATED** (deletion isn't undoable;
  controller recreation is the "revert" — keep that preview wording).
- **⚑JP-Q4-add:** `rollout undo` deserves to be a **first-class safety verb**
  (fastest rollback of a bad deploy; low-risk; its own inverse is another
  `rollout undo`), not merely a revert mechanism. Surface to JP.
- **`cordon`/`drain` stay OUT of v0.1.4** (node-level, higher blast radius,
  drain genuinely destructive/hard to revert; no daily-driver signal).

### w1-D11 — new JP question: `port-forward` disposition (⚑JP-Q7)
**⚑JP.** `port-forward` is in kubectl's daily set but **unmapped** in SM §5. It
is documented-fragile (single connection, idle-drop, no reconnect) — a poor fit
for a headless *audited* verb. **Recommend refuse-with-hint for v0.1.4** (point
at raw `kubectl port-forward`); revisit as an owned verb only if field signal
demands. New question Q7 for JP.

### w1-D12 — `cp` REFUSE reinforced
**✓VALIDATES.** kubectl `cp` requires `tar` in-container (silent degrade
otherwise). Reinforces SM §5.3 `cp` REFUSE-with-hint. No change.

### w1 net-new for Phase 4
- New JP questions: **Q7** (port-forward disposition).
- New item candidates: container-exit-code surfacing in `why`/`status`
  (w1-D3); `logs` auto-pick+hint (w1-D1); crashed-container `--previous`
  auto-hint (w1-D2); `rollout undo` as first-class verb (w1-D10/⚑JP).
- Revert-contract resolution: `restart` = `command_pair` via `rollout undo`
  (closes SM §5.3).
- Wave-A hard invariants: kubectl PATH probe; kubeconfig/context threaded to
  every shell-out (tested); stderr→`failure_class` parser.

---

*w2 (stern/kail/kubectx/kubens/kubecolor) and w3 (popeye/lens/pain-points)
integration sections append on subsequent turns; then the synthesis turn folds
confirmed deltas into the surface map + drafts the Phase-4 K-item list.*
