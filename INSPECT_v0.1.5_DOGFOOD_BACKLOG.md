# inspect v0.1.5 — Dogfooding / Usage Backlog

**v0.1.5 is a usage-feedback phase, not a feature-build pass.** Real machine users (the coordinator + devops sub-coords) drive inspect on real maker ops; each session appends a report — **what went WELL · what went BADLY · THE ONE feature that would have helped most.** Features get built by *evidence* (a recurring "one feature" across sessions), not by guessing. When it's solid → v2.0.

Append newest session at the top.

---

## Session 2 — 2026-07-07 · devops sub-coord, maker full-deploy (the v0.1.4-is-status-only verdict)

A single messy, multi-surface maker deploy — core / helix / mycelium / omni / secretary / beacon / atlas driven green through ImagePullBackOff, CrashLoopBackOff, kafka mTLS, ExternalSecret sync failures, ArgoCD OutOfSync, NetworkPolicy deny, and FGA/delegation gaps. **Hard telemetry from the transcript: 317 shell commands — 144 `kubectl`, 93 `argocd`, and only 5 `inspect` (3 of which were just "is inspect installed / `--version` / `--help`").** inspect was reached for twice as real ops tooling (`inspect status helix`) and both times fell short. Every one of the 144 kubectl reaches is a capability inspect v0.1.4 doesn't have. v0.1.4 is *status-only*; the deploy needed a *diagnostic + GitOps + CRD-aware ops tool*. The gaps below are ranked by how often the missing capability forced a raw-kubectl reach today.

### Verdict
inspect v0.1.4 answers exactly one question — *"is this pod healthy?"* That's the *first* question of a deploy and the tool nailed it (it revealed the `:latest`-not-in-Harbor root cause in one call). But it's the *only* question it answers. The moment "unhealthy" turned into "**why**, and what do I do" — the entire body of the work today — inspect had nothing, and the operator dropped to kubectl for the next 144 commands and stayed there. A *complete* ops tool has to carry the operator across the whole arc: **health → GitOps reconciliation state → the failure triad (describe+logs+events) → a diagnosis that names the failure class → the fix verb → re-verify.** v0.1.4 owns step one.

### G1 — ArgoCD Application-awareness (GitOps reconciliation state) ▶ #1 by frequency
> **93 `argocd` + 77 `kubectl get application` — the single most-reached surface, and inspect has zero concept of it.** Every surface's real state lived in its ArgoCD Application (Synced/OutOfSync, Healthy/Degraded, live-vs-desired diff, sync-hook jobs), not its pods. **build:** `inspect argo status <ns>` — sync+health, OutOfSync list, failing sync-hook jobs; read-only first, audited `argo sync --apply` follow-on.

### G2 — The failure triad: describe + logs --previous + events ▶ #2
> 33 `kubectl logs` (many `--previous`) + describe + events — the root-causing loop, entirely absent. `--previous` is essential (the crashed container is gone). **build:** `inspect describe <ns>/<pod>`, `inspect logs <ns>/<pod> --previous --tail N` (L7-redacted), `inspect events <ns> [--for pod]`. The atoms every diagnosis composes from.

### G3 — A `why` NEXT-hint that actually diagnoses the failure class ▶ #3
> 17 CrashLoop + 12 ImagePull moments, each hand-classified. Classes are finite + machine-recognizable (image-not-in-Harbor / missing pull-secret / DNS / NetworkPolicy-deny / ExternalSecret-unsynced / config-fail-closed). **build:** `inspect why <ns>/<pod>` — run the G2 triad, classify into a named class, chain a `hint:` to the fix. (Session-1's "unhealthy→why" widened by Session-2 evidence into a real classifier.)

### G4 — Deployment / rollout / Job-aware status ▶ #4
> 23 `kubectl get deploy` + jobs + rs; atlas "rollout mid-flight / 503". Pod status misses the workload story (replica counts, rollout progress, failing seed/migrate Jobs). **build:** roll `inspect status` up per-Deployment + list Jobs; `inspect rollout <ns>/<deploy>`.

### G5 — Fleet / multi-namespace rollup ▶ #5 (Session-1 "THE ONE", reconfirmed)
> Operator hand-wrote `for ns in core helix mycelium omni secretary beacon; do kubectl get pods -n $ns; done` — exactly the loop Session 1 predicted. **build:** `inspect status --all` (or bare) rolls up every configured namespace, one row/surface (sync/health/rollout/unhealthy-count). Pairs with G1+G4 as columns.

### G6 — CRD-awareness: ExternalSecret / Secret store ▶ #6
> 16 `get externalsecret` + 14 `get secret`. ESO-shaped blockers were a top-3 class today (harbor-pull-secret missing, helix-secrets "could not get secret data"). **build:** `inspect secrets <ns>` — ExternalSecrets w/ SecretSynced + failure reason + backing-secret existence (no values). Feeds G3's classifier.

### G7 — Read-only exec + port-forward for k8s ▶ #7
> 23 `kubectl exec` + port-forward, used read-only (curl in-pod health, check env, reach OpenFGA). inspect refuses k8s exec/port-forward entirely — right for mutations, wrong for read-only diagnosis. **build:** `inspect exec <ns>/<pod> -- <cmd>` read-only (mirrors `inspect run`), `inspect port-forward`. Keep the mutation refusal.

### G8 — CRD-awareness: Strimzi (KafkaTopic/KafkaUser) + ARC runners ▶ #8
> 5 kafkatopic/kafkauser + 16 ARC (autoscalingrunnerset/ephemeralrunner). First-class maker CRDs inspect can't see. **build:** a generic **CRD-status capability** `inspect crd <ns> <kind>` (reads any CRD's `.status.conditions`) with built-in printers for the Luminary set (Application/ExternalSecret/KafkaUser/RunnerSet) — capability-shaped (Rule 8), not one-verb-per-CRD. Generalizes G1+G6.

### G9 — OpenFGA / delegation store introspection ▶ #9 (scope Q for JP)
> 8 openfga + port-forward; secretary's real blocker was the FGA delegation-node value. **build (flagged, maybe out-of-charter):** optional `inspect fga store <ns>` — store id + model id + declared types. Reaches into Onyx identity-substrate; may belong outside inspect's server-ops charter — JP call.

### G10 — Infra reachability: a runner-mediated maker target ▶ cross-cutting
> Operator logged it: inspect has no maker target out of the box — maker is reached via `ssh shared-runner 'kubectl …'`, but inspect's k8s runtime runs kubectl *locally*. This is *why* even do-able health checks went through raw ssh+kubectl. **build:** a `--via <ssh-ns>` / runner-mediated k8s namespace type dispatching the kubectl backend through an existing inspect SSH namespace (composes the docker-SSH + k8s-kubectl runtimes that already exist). Without this, G1–G8 aren't usable against maker as it's actually reached.

**Frequency ledger:** get pods 85 · get application 77 (+93 argocd) · logs 33 · get deploy 23 · exec 23 · get externalsecret 16 · get secret 14 · ARC 16 · kafkatopic/user 5+ · port-forward 3 · events 3 · describe 2.

*(next session appends above this line — Session 1 follows below)*


## Session 1 — 2026-07-07 · coordinator, checking the maker deploy (5 new surfaces, G4)

First real use: configured inspect against maker and used it to check surface health during the G4 deploy.

**What went WELL:**
- **`-h` was genuinely agent-friendly — it was NOT the source of my delay.** `inspect add --help` had a *dedicated* explanation of `--non-interactive` ("requires every required value on the command line and errors with `missing required value` instead of prompting"), the exact k8s flag set (`--context`/`--kubeconfig`/`--namespace`), a note that there's no env-var form for add-flags, and a k8s example. It answered my exact question cold. The agent-first help investment shows.
- **Chained hints self-corrected me.** `inspect status core` → `no cached profile — run inspect setup core first`; `inspect add` → `NEXT: inspect test <ns>`. I never had to guess the next step — the tool told me.
- **`inspect status <ns>` output is exactly right for an agent:** per-pod `healthy/unhealthy` + the resolved image tag. That one line (`…/luminary/core:latest  unhealthy`) *immediately* revealed the G4 root cause — the pod is pulling a `:latest` image that doesn't exist in Harbor. Diagnosis from the tool in one call.
- Context-pinning worked as designed (never touched ambient `current-context`); direct kubeconfig over the VPN worked; `--force` idempotent re-add was clean.

**What went BADLY / friction:**
- **`add → setup → status` needs a separate discovery step.** `status` errors until you run `setup <ns>` first. It's *well-hinted* (self-correcting), but it's an extra manual step for a read that could lazily self-discover on first `status`.
- **An unhealthy pod's `status` ends `NEXT: (none)`.** When a pod is `unhealthy`, the highest-value next action is "why?" — chaining to `inspect describe <ns>/<pod>` or a `why` would turn one command into a diagnosis path. (The healthy case correctly has no next step; the *unhealthy* case is where a chained hint would earn the most.)
- *(Not inspect's fault, logged for honesty:)* the devops-skill doc had the wrong command names (`inspect ns add --type kubernetes` vs the real `inspect add --type k8s`) — fixed. The *tool* was right; our *docs* drifted.

**THE ONE feature that would have helped most:**
> **A fleet / multi-namespace rollup** — `inspect status` (no arg) or `inspect status --all` that rolls up **every configured namespace** in one call. Checking a multi-surface deploy meant running `inspect status <ns>` once per surface (core/helix/mycelium/omni/secretary/…). A coordinator watching a deploy wants *"show me every surface's health, one call, one glance."* This is the single highest-leverage gap for the ops-oversight use case.

---
*(next session appends above this line)*
