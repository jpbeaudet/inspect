# inspect v0.1.5 — Dogfooding / Usage Backlog

**v0.1.5 is a usage-feedback phase, not a feature-build pass.** Real machine users (the coordinator + devops sub-coords) drive inspect on real maker ops; each session appends a report — **what went WELL · what went BADLY · THE ONE feature that would have helped most.** Features get built by *evidence* (a recurring "one feature" across sessions), not by guessing. When it's solid → v2.0.

Append newest session at the top.

---

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
