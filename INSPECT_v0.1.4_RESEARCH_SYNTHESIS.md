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

Integration status: **w1 ✓ done · w2 ✓ done · w3 ✓ done · synthesis ✓ done → PHASE-3-COMPLETE**.

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

---

## Batch w2 — stern / kail / kubectx+kubens / kubecolor (dossier: `stern_kail_ctx_kubecolor.md`)

Bottom line: **VALIDATES `logs --merged`, `-c`/`--previous`, the two-surface
color discipline, and Q1 (2-seg selector + `-n`) — and hands us a real
*advantage over kubectx/kubens*.** The tools studied exist to solve exactly the
problems our map targets; their footguns tell us what to do better.

### w2-D1 — `logs --merged` fan-out: VALIDATES the whole premise
**✓VALIDATES.** stern and kail exist *only* because `kubectl logs` can't fan out
across replicas; both auto-discover a workload's pods and follow new ones.
Building `--merged` into `inspect logs` (rather than making agents shell out to
stern) is correct and on-thesis (shell-is-the-integration-layer). ⚙WAVE B.

### w2-D2 — per-line pod+container tagging in `--merged --json` (mandatory)
**+ADD.** stern's single most-loved feature is that every merged line is tagged
with its source pod/container. Our merged fan-out **must** stamp each line with
`pod` + `container` (+ `revision`/ReplicaSet) in the JSON stream — a per-line
`{pod, container, revision, ts, message}` record — or agents can't tell replicas
apart. (Reinforced by w3-P8's "wrong-pod-of-the-deployment" detective story.)
⚙WAVE B.

### w2-D3 — `--previous` is two concepts; disambiguate in `-h`
**+ADD.** stern #13/#36 show "previous" conflates (i) the *last crashed
container* (kubectl `--previous`) and (ii) *the full restart timeline*. Our
`--previous` `-h` must state explicitly: "shows the **last terminated**
container only" so we don't inherit stern's ambiguity. ⚙WAVE B (help).

### w2-D4 — reliability/heartbeat guard on long `-f` tails (edge over stern)
**+ADD.** stern #361: multi-hour tails **stall silently with no error** — the
worst failure mode for an unattended/agent caller. Our `--merged`/`-f` should
surface a **stream-health/heartbeat signal** (or a documented max-idle →
`failure_class`) rather than silently stalling. An explicit edge over stern.
⚙WAVE B/E.

### w2-D5 — Q1 VALIDATED strongly + the global-state *advantage*
**✓VALIDATES + headline safety property.** The kubectx/kubens split itself
proves practitioners treat **context** and **namespace** as *ambient state set
once*, not per-command args (nobody types `prod/api/web` every command; they
`kubectx prod` + `kubens api` then address `web`). Our 2-seg
`<inspect-ns>/<workload>` with context+ns in config is the faithful analogue;
the 3-seg form would force re-typing ambient state every selector — exactly what
kubens was built to eliminate. **AND:** kubectx/kubens' most-cited pain is that
they mutate **global `~/.kube/config` current-context**, leaking across
terminals ("context-pong," prod-damage risk); the community's fix is
`KUBECONFIG`-per-shell isolation. **Our config-per-(context,namespace) is
inherently immune** — each inspect namespace pins its own context+namespace, so
`inspect logs staging-k8s/api` is deterministic regardless of any other shell.
**Design invariant:** inspect must **pin `--context` explicitly on every kubectl
invocation** and never read/mutate the ambient `current-context`. Converges with
w1-D8(2) and w3-P1. ⚑JP Q1 → ratify as recommended. ⚙WAVE A (invariant+test).

### w2-D6 — do NOT port the `kubectx -` "previous" gesture
**✓VALIDATES (a non-add).** The `-` bounce is beloved but only makes sense for a
*stateful* switcher; inspect is stateless per invocation. Adding hidden session
state would re-introduce the global-state footgun. Don't port it; note the
reasoning in `-h` if asked.

### w2-D7 — Q6 answered: `connect` is N/A for k8s (as a safety property)
**✓VALIDATES.** kubectx/kubens confirm k8s addressing is ambient config, not a
session — there is nothing to connect. `connect`/`disconnect` should carry a
clear **"N/A for k8s (kubeconfig is stateless; target resolved per-verb from
config — no sticky-context footgun)"** note. Converges with w3-Q6. ⚑JP Q6 →
resolve as N/A-with-safety-note.

### w2-D8 — kubecolor: two-surface color discipline + don't reparse-to-colorize
**✓VALIDATES §8 + +ADD.** kubecolor's correct behavior — color for a TTY,
auto-plain when piped (`NO_COLOR`, non-tty) — is the discipline our format layer
needs; color must **vanish under `--json`/non-tty/`NO_COLOR`**. **+ADD:** honor
`NO_COLOR` + an explicit `--color=auto|always|never` (community-standard). But
kubecolor's honest admission that heuristic log-coloring is fragile (bad at
multiline, once *froze* on >65 KB lines) is a warning: **do not build a
reparse-and-colorize log layer — structure comes from the data model**
(the per-line `{pod,container,...}` records of w2-D2), not from re-coloring
rendered prose. ⚙WAVE E.

### w2-D9 — `-A`/`--all-namespaces` on k8s read verbs (kubectl parity)
**+ADD.** For the "same cluster, many namespaces" case, add an
`-A`/`--all-namespaces` flag on k8s read verbs alongside `-n` — additive
kubectl-parity, fully consistent with Q1 (not a selector change). Keeps the
multi-namespace case to a single config stanza. ⚙WAVE B.

### w2-D10 — kail object-graph selection = v0.1.5+ (stated boundary)
**Boundary (not scope creep).** kail's `--svc`/`--deploy`/`--ing` "tail
everything behind this Service" is a real ergonomic our `network`/`why` graph
could feed later — stated as a **v0.1.5+** input, not a v0.1.4 selector change.

---

## Batch w3 — popeye / lens + practitioner pain-point hunt (dossier: `popeye_lens_painpoints.md`)

Bottom line: **Popeye is the reference design for `why`; Lens shells out to
kubectl (Q2 precedent) and its cross-replica log search validates `--merged`;
and the ranked pain-hunt (P1–P8) validates the map on every axis while sharpening
the write-safety + error-message contracts.** P1 (wrong-context/namespace
destruction) is THE #1 kubectl horror class and directly validates our Q1
anti-footgun design.

### w3-D1 — `why` gets Popeye's content model + severity rollup
**+ADD.** Popeye validates *what belongs in a good "what's wrong here" answer*.
`why` should add: **probe presence** (readiness/liveness), **requests/limits
presence**, **image-tag hygiene** (`:latest`/no-digest), and the per-container
`restartCount` + `lastState.terminated.reason` table (the jsonpath everyone
copy-pastes). Popeye's OK/Info/Warn/Error grading → our envelope's `state`
worst-severity **rollup** so agents branch without parsing prose. Converges with
w1-D3. ⚙WAVE C.

### w3-D2 — headline anti-footgun: every write echoes resolved context+ns+workload
**+ADD (headline safety property, P1).** Wrong-context/namespace destruction is
the #1 kubectl horror class. Every k8s **write dry-run preview, confirmation
prompt, and audit record must echo the resolved target**: "will scale
`deploy/api` in namespace `prod` on context `prod-eks`." The `meta` block must
carry the resolved cluster/context/namespace so an agent can read back *which
cluster it just hit*. This is a **CHANGELOG + help headline** — inspect does the
"confirm which cluster before acting" mitigation *for* the operator. Converges
with w2-D5. ⚙WAVE A/D.

### w3-D3 — RBAC forbidden → four-question gate (highest-value error upgrade, P2)
**+ADD.** kubectl's forbidden message names the identity but **never the missing
binding** — the specific thing everyone complains about. Reshape it into
inspect's four-question gate: **what** verb was denied, **on which**
resource/namespace, **which identity**, **what to run** (embed the literal
`kubectl auth can-i <verb> <resource> -n <ns>` in the `hint:`). Distinct
`failure_class = "rbac_forbidden"` (not folded into transport) so agents branch
to "escalate perms" vs "retry." The `setup`/`test` `auth can-i` self-test
pre-empts it. ⚙WAVE A (failure model) + B (setup).

### w3-D4 — CrashLoop/replica log ergonomics (P3 + P8)
**+ADD.** Converges with w1-D1/D2. `logs`/`why` **auto-hint `--previous`** on
CrashLoop ("container X restarted N times; showing current — add `--previous`
for the crashed instance"), doing the container-status jsonpath *for* the
operator. On a multi-container pod with no `-c`, **don't error like kubectl** —
default to first/app container with a hint enumerating the others + restart
counts. When addressing `logs <ns>/<deploy>` without `--merged`, **state which
pod was selected and how many replicas exist** ("showing 1 of 3 replicas: pod
`api-abc` revision 7; use `--merged` for all") — never silently pick one (the P8
"wrong pod, old image" trap). ⚙WAVE B/C.

### w3-D5 — write-safety: restart-revert + delete-pod outage guard (P4)
**✓VALIDATES + ADD.** Community explicitly warns against delete-pod-as-restart /
scale-cycling and endorses `rollout restart` + `rollout undo` — validating our
Q4 set and the w1-D10 resolution (**`restart` revert = capture `rollout history`
revision → `rollout undo --to-revision=N` `command_pair`**). **+ADD outage
guard:** `delete pod` / `scale --replicas=0` must **escalate the confirmation**
when the op would drop ready replicas below the deployment threshold, and the
preview must warn about the outage window. `stop`/`start` → REFUSE-with-hint at
`scale --replicas=0` (validated). ⚙WAVE D.

### w3-D6 — distroless/no-shell exec (P5): NEW failure class + gap in REUSE map
**+ADD (real smoke-tripping gap).** `run`/`exec`/`cat`/`ls` REUSE assumes a
shell/coreutils in the container; on distroless/minimal images `kubectl exec --
cat/ls/sh` **fails** with a raw OCI error. inspect must detect this and emit
`failure_class = "no_shell_in_container"` + a hint pointing at ephemeral-container
debug (`kubectl debug`). A **first-class ephemeral-debug verb** is a stated
**v0.1.5+** boundary (not a silent gap); v0.1.4 ships the detection + hint.
⚙WAVE B (detection) + boundary note.

### w3-D7 — `top` degrade (P6)
**✓VALIDATES.** metrics-server is absent by default on many clusters (EKS). `top`
must degrade with distinct `failure_class = "metrics_unavailable"`, distinguish
**absent vs just-started** (`retry ~60s`), give the install command, and never
return a raw kubectl error. `setup`/`test` probes metrics-server and records
availability in the profile. ⚙WAVE C + B (setup).

### w3-D8 — `events` ordering (P7)
**✓VALIDATES.** The #1 events complaint is unstable ordering (`kubectl get
events` isn't chronological). `events` must **always sort newest-first**
(documented in `LONG_EVENTS`, same discipline as `LONG_AUDIT_LS`), auto-scope to
the object (do the field-selector for the operator), feed into `why`, and hint
when the ~1h retention window may have expired. A concrete "inspect fixes a named
kubectl pain" win. ⚙WAVE C.

### w3-D9 — Q3 (bundle seam) supported, with a migration caveat (⚑JP-Q8)
**✓VALIDATES the split + ⚑JP.** No pain evidence demands mixed docker+k8s
composition in one bundle for v0.1.4 — every recurring pain (P1–P8) is
single-medium. Build the **seam** now (k8s steps callable + per-step
audit/revert); defer **mixed-composition** to v0.2.0. **Caveat → new ⚑JP-Q8:**
the *migration* use case ("drain docker service, bring up k8s equivalent") is
where mixed-composition shines — **if JP's field users are mid-migration, that
raises v0.2.0's priority** (does not move it into v0.1.4). Ask JP whether the
field base is mid-migration.

### w3-D10 — cluster-wide "sanitize"/popeye verb = v0.1.5+ (stated boundary)
**Boundary.** Popeye is *cluster-wide audit*; v0.1.4 `why` is *object-scoped*. A
cluster-wide sanitize verb is a stated **v0.1.5+** candidate (Popeye the
reference design), not a silent gap.

### w3-D11 — Q2 external precedent
**✓VALIDATES.** Lens — the market-leading observability IDE — shells out to
kubectl under the hood for legibility + auth inheritance. Strong external
precedent for our kubectl-shell-out lean (Q2/§9). Lens Prism (AI "why did this
fail + how to fix") is literally the agentic `why` use case; our JSON `why` +
chained `hint:` is the shell-native version — the design bet is validated.

---

## PHASE-3 CLOSE — consolidated confirmed deltas → Phase 4 seed

All three dossiers **VALIDATE the surface map's core** (runtime-agnostic reuse,
2-seg selector + `-n`, kubectl shell-out, conservative writes). No finding
contradicts the map; the deltas are refinements + additions. Consolidated for
Phase 4:

### C1 — Resolved decisions (fold into the surface map)
- **Q1 selector — RATIFY 2-seg `<inspect-ns>/<workload>` + `-n` + `-A`** (w1-D9,
  w2-D5, w3-D2). Pin `--context` explicitly on every kubectl call; never touch
  ambient `current-context`. Anti-footgun headline.
- **Q2 backend — kubectl shell-out for v0.1.4** (w1-D7, w3-D11), Runtime trait
  keeps a kube-rs swap mechanical. One non-trivial cost: a stderr→`failure_class`
  parser (Wave A).
- **Q4 revert — `restart` = `command_pair` via captured `rollout undo
  --to-revision=N`; `delete pod` = `unsupported`; `scale` = `command_pair`**
  (w1-D10, w3-D5). Closes the surface-map §5.3 open question.
- **Q6 — `connect`/`disconnect` = N/A-for-k8s safety note** (w2-D7, w3-Q6).

### C2 — New failure-class taxonomy (Wave A)
`rbac_forbidden` (w3-D3), `no_shell_in_container` (w3-D6),
`metrics_unavailable` (w3-D7), plus the k8s transport classes (unreachable /
token-expired / TLS) from SM §10. All CI-gate-quality (what/where/why/fix).

### C3 — New JP questions/adds surfaced by research
- **⚑Q7 (w1-D11):** `port-forward` disposition → recommend refuse-with-hint v0.1.4.
- **⚑Q8 (w3-D9):** is the field user base mid-migration? (bears on whether mixed
  docker+k8s bundle composition should move up from v0.2.0).
- **Q4 add-candidate:** `rollout undo` as a first-class safety verb (w1-D10);
  delete-pod/scale-to-0 **outage guard** (escalate confirmation below
  ready-replica threshold) (w3-D5).

### C4 — Stated v0.1.5+ boundaries (not silent gaps)
Ephemeral-container debug verb (w3-D6); cluster-wide sanitize/popeye verb
(w3-D10); kail object-graph log selection (w2-D10).

### C5 — Per-wave delta assignments (seed the K-item backlog)
- **Wave A (foundation):** Runtime trait + docker refactor; `type`/kubeconfig
  config + conditional validate; **kubectl PATH probe**; **`--context` pinned on
  every shell-out (tested invariant)**; **stderr→`failure_class` parser** with
  the C2 taxonomy; resolved-target echoed in `meta`.
- **Wave B (read core):** `setup`/`test` (`auth can-i` + metrics-server probe +
  inventory); `status`/`ps`/`health`; `logs` (`-c` auto-pick+hint,
  `--previous` disambiguated, `--merged` with per-line `{pod,container,revision}`
  tagging + heartbeat guard, `-A`); `cat`/`ls`/`grep`/`run` (+ no-shell
  detection).
- **Wave C (k8s-native reads):** `why` (Popeye content model + exit-code/reason
  table + severity rollup + `--previous` auto-hint), `describe`, `events`
  (newest-first, auto-scoped, fed to `why`), `top` (degrade), `ports`/`network`/
  `volumes`/`images`.
- **Wave D (conservative writes):** `scale`, `restart`(rollout + `rollout undo`
  revert), `delete pod` (outage guard), `exec --apply`; REFUSE mappings with
  immutability hints; every write echoes resolved context+ns+workload.
- **Wave E (integration + polish):** `fleet` mixed rollup; `search` across
  mediums; bundle runtime-aware seam; `kubernetes.md` help topic (sell `--select`
  over jsonpath; `--color`/`NO_COLOR`; the anti-footgun property) + `LONG_*`
  sweep; smoke runbook against a real cluster.

*Phase 3 complete. Next: Phase 4 — DEEP DESIGN: author `INSPECT_v0.1.4_BACKLOG.md`
(K-item decomposition mirroring the reconstructed v0.1.3 process) + amend
CLAUDE.md where the design changes it; save-point marked DESIGN-COMPLETE. The
six→eight JP questions (Q1–Q8) need ratification before Phase 4 design closes.*
