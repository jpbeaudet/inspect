# INSPECT v0.1.4 — Phase 3 Research: kubectl + k9s

**Worker:** inspect-w1 · **Date:** 2026-07-04 · **Branch:** `feat/v0.1.4-program`
**Inputs:** `INSPECT_v0.1.4_SURFACE_MAP.md`, `INSPECT_v0.1.4_STUDY.md`
**Assignment:** Deep research on kubectl + k9s; map each finding to our mapped
surfaces (VALIDATES / CHALLENGES / ADD-A-FLAG); stress-test recommendations
Q1 (2-seg selector + `-n` parity), Q2 (kubectl shell-out backend), Q4
(conservative write set).

The bottom line up front: **research strongly VALIDATES all three
recommendations.** kubectl is the idiom agents already know, its `-o json`
surface is the exact machine-readable contract our envelope wraps, and its
worst footguns (silent multi-container failure, `--previous` discoverability,
jsonpath quoting traps, non-namespaced `-A` surprises) are precisely the
class of thing our `-h`-first + chained-hint discipline exists to fix. k9s is
a *human* TUI whose own issue tracker is a catalogue of why an
agent-legible JSON verb surface is the better shape for our audience. Details
+ the handful of real challenges below.

---

## PART 1 — kubectl

### 1(a) Core verb/command surface an SRE drives daily

The daily surface, with the flags that matter to an agentic caller:

| Verb | Agent-critical flags | Notes |
|---|---|---|
| `get <kind> [name]` | `-o json` / `-o jsonpath='{…}'` / `-o wide` / `-o name`; `-n <ns>` / `-A`/`--all-namespaces`; `--context`; `--sort-by='{.field}'`; `-l`/`--selector`; `-w`/`--watch` | The read workhorse. `-o json` is the machine contract. |
| `describe <kind> <name>` | `-n`, `--context` | Human-readable deep dump: spec + conditions + **Events** appended at the bottom. **No `-o json`** — describe is text-only, a real gotcha (see 1c). |
| `logs <pod>` | `-c`/`--container`; `-p`/`--previous`; `-f`/`--follow`; `--tail=N`; `--since=1h` / `--since-time`; `--timestamps`; `--all-containers=true`; `-l` (select pods) | The single most footgun-laden read verb (see 1c). |
| `exec <pod>` | `-c`; `-i`/`-t` (interactive TTY); `-- <cmd>` | The `--` separator is load-bearing; without it flags leak to kubectl. |
| `scale <kind>/<name>` | `--replicas=N`; `--current-replicas=N` (guard); `-l` | Cleanest revertible write. |
| `rollout restart <kind>/<name>` | — | Rolling recreation; no `--replicas`. |
| `rollout undo <kind>/<name>` | `--to-revision=N` | The inverse of a bad deploy; pairs with `rollout history`. |
| `rollout status <kind>/<name>` | `-w` (default watches), `--timeout` | **Blocks until rollout completes or times out.** Exit non-zero on failed/timed-out rollout. |
| `rollout history <kind>/<name>` | `--revision=N` | Enumerates revisions for `undo --to-revision`. |
| `delete <kind> <name>` | `--grace-period`, `--force`, `--wait`, `-l`, `--field-selector` | Deleting a **pod** owned by a controller → controller recreates. Deleting the **controller** → gone. |
| `top pods\|nodes` | `-n`, `-A`, `--containers`, `--sort-by=cpu\|memory` | **Requires metrics-server**; errors hard if absent. |
| `events` | `--for <kind>/<name>`; `-A`; `--types` | Newer dedicated verb; older idiom is `get events --sort-by='.lastTimestamp'`. |
| `wait` | `--for=condition=Ready` / `--for=delete` / `--for=jsonpath=…`; `--timeout=90s` | Block-until-condition; the clean primitive our `watch` should map onto. |
| `port-forward <pod\|svc>` | `<local>:<remote>`; `--address` | Long-lived, single-connection, **flaky on idle** (see 1c). |
| `cp <pod>:<src> <dst>` | `-c` | **Requires `tar` in the container**; silently degraded otherwise. |

### 1(b) Output shapes + conventions agents/scripts rely on

- **`-o json`** is the canonical machine contract: the full object/list as
  the API returns it. **`-o jsonpath='{.items[*].metadata.name}'`** projects.
  **`-o name`** yields `kind/name` refs. **`-o go-template`** for complex
  shaping. Scripts overwhelmingly build on `-o json | jq` because jsonpath
  is weaker than jq (see below).
- **`--sort-by` takes a JSONPath**, and it must be the brace form
  `--sort-by='{.metadata.name}'` / `--sort-by='.lastTimestamp'`. The kubectl
  help itself historically shipped a **wrong example** (`ObjectMeta.Name`),
  a documented bug — [kubernetes/kubernetes#21018](https://github.com/kubernetes/kubernetes/issues/21018).
- **Exit codes:** `kubectl` returns `0` on success, `1` on most errors
  (not-found, forbidden, invalid), and its own non-zero for `rollout status`
  timeout / `wait` timeout. There is **no rich exit-code taxonomy** — a
  not-found, an RBAC-forbidden, and an unreachable-API-server can all surface
  as exit `1` with the distinction *only in the stderr text*. This is a gap
  our `failure_class` + 12–14 transport discipline directly improves on.
- **Container exit codes** (a different axis agents conflate) are surfaced in
  pod status, e.g. `137` (OOMKilled / SIGKILL), `143` (SIGTERM), `139`
  (SIGSEGV) — [cast.ai/blog/kubernetes-exit-codes](https://cast.ai/blog/kubernetes-exit-codes/).
  These live in `.status.containerStatuses[].lastState.terminated.exitCode`,
  reachable only via `-o jsonpath` / `-o json`, never in `logs`.
- **Error message shapes** are prose, e.g. `Error from server (NotFound):
  pods "x" not found`, `Error from server (Forbidden): … cannot get resource
  …`, `error: unable to upgrade connection: container not found`. Parseable
  by the `(Reason)` in parens but **not structured** — again, exactly what
  our envelope normalizes.

### 1(c) Known PAIN POINTS / footguns (practitioner-cited)

1. **`kubectl logs` on a multi-container pod fails without `-c`.** With >1
   container, `logs <pod>` errors `error: a container name must be specified
   for pod <p>, choose one of: [a b c]` — or (with `--all-containers`)
   silently interleaves. Long-standing feature request to auto-pick:
   [kubernetes/kubectl#371](https://github.com/kubernetes/kubectl/issues/371).
   Practitioner writeups treat this as *the* first-hour gotcha —
   [Why kubectl logs Doesn't Work with Multi-Container Pods](https://medium.com/@ugur.mendi96/why-kubectl-logs-doesnt-work-with-multi-container-pods-and-how-to-fix-it-9f4f4fc5c0d9),
   [SigNoz](https://signoz.io/blog/kubectl-logs/).
2. **`--previous` / `-p` is undiscoverable exactly when you need it.** After a
   crash-loop, current logs are empty/useless; you must *know* to add `-p` to
   read the dead container's logs. Practitioners repeatedly rediscover this —
   [oneuptime: kubectl logs --previous after crashes](https://oneuptime.com/blog/post/2026-02-09-kubectl-logs-previous-container/view).
3. **`describe` has no `-o json`.** The richest human view (events, conditions)
   is text-only; scripts must fall back to `get -o json` + manual event
   correlation. Our `describe`-into-envelope reshaping (SURFACE_MAP §5.2) is a
   genuine improvement, not just parity.
4. **Namespaced-vs-not surprises.** `get <kind>` shows only the *current*
   namespace unless `-A`; but some kinds (nodes, PVs, namespaces themselves)
   are cluster-scoped and `-n` is silently ignored — a persistent confusion.
   Kubernetes' own pitfalls post lists forgetting namespace scope among the
   seven — [kubernetes.io/blog: 7 pitfalls](https://kubernetes.io/blog/2025/10/20/seven-kubernetes-pitfalls-and-how-to-avoid/).
5. **jsonpath quoting + capability traps.** `--sort-by`/`-o jsonpath` require
   the `{…}` brace form and break on Windows quoting; kubectl jsonpath has
   **no regex**, so anything nontrivial forces `-o json | jq` —
   [kubectl jsonpath support](https://kubernetes.io/docs/reference/kubectl/jsonpath/),
   [kubectl#1535](https://github.com/kubernetes/kubectl/issues/1535),
   [website#9606](https://github.com/kubernetes/website/issues/9606).
6. **`exec` missing `--`.** `kubectl exec pod ls -la` mis-parses; `kubectl
   exec pod -- ls -la` is required. `container not found` / `unable to upgrade
   connection` errors are opaque.
7. **`top` silently depends on metrics-server.** No metrics-server → `error:
   Metrics API not available`. A cluster-config dependency masquerading as a
   CLI bug.
8. **`port-forward` is fragile:** single connection, dies on idle/pod-restart,
   no auto-reconnect — a recurring ops complaint.
9. **`delete pod` is not "undo".** Deleting a controller-owned pod triggers
   recreation (looks like a restart); deleting the controller is permanent and
   irreversible. The mental-model mismatch is a real footgun.
10. **Manual `kubectl` triage doesn't scale** — practitioners reach for
    dashboards/platforms precisely because chaining raw kubectl across pods is
    cumbersome ([360cloudplatforms: K8s pain points](https://360cloudplatforms.com/blog/dont-drown-solutions-for-common-kubernetes-pain-points)).
    inspect's `why` deep-bundle is the answer to this class.

---

## PART 2 — k9s

k9s (`derailed/k9s`) is a **full-screen terminal UI** for live cluster
navigation — a human-operator tool, keyboard-driven, *not* a scriptable/
agent surface. That framing itself is the key finding for us (see mapping).

### 2(a) Navigation + core surface

- **Command bar / resource views.** Type `:pods`, `:deploy`, `:svc`, `:ns`,
  `:events`, `:xray deploy`, etc. to switch resource views; aliases (`:po`,
  `:dp`). `:ctx` switches kubeconfig context, `:ns` scopes namespace. Numeric
  `0`–`9` fast-switch namespaces.
- **Selection actions** (single-key on the highlighted row): `l` logs, `s`
  shell/exec into a pod, `d` describe, `y` YAML, `e` edit, `Ctrl-D` delete,
  `Ctrl-K` kill, `Enter` drill-in (deploy→pods→containers).
- **Scale / restart:** `s`-cale prompt on a deployment; restart via delete or
  the rollout menu depending on version.
- **Log view:** live tail with `0`/`1`/`2`… for time windows, `/` in-view
  filter, wrap/timestamp toggles, previous-container toggle.
- **Filters:** `/` fuzzy-filters the current table; `/!` inverse; label filter
  `-l`. This is the human analogue of our `--select`/`grep`.
- **Events + describe** are surfaced as views (`d` / `:events`), not JSON —
  read-optimized for eyes, not pipes.
- Repo: [github.com/derailed/k9s](https://github.com/derailed/k9s).

### 2(b) Output shapes

k9s emits **rendered TUI frames**, not machine output. It shells out to
kubectl/client-go under the hood for `logs`/`exec`. There is **no stable
JSON/stdout contract** — its "output" is the screen. Any agent trying to
consume k9s scrapes a terminal, which is exactly the anti-pattern our
authorized-deferral of the L1 TUI (CLAUDE.md) already rejects. k9s is the
worked example of "a dashboard is strictly worse than a JSON-emitting verb
for an agent."

### 2(c) Known PAIN POINTS / footguns (from k9s's own issue tracker)

1. **Shell-into-pod broke for locked-down RBAC (`0.50.12`).** k9s started
   requiring **node read access** to detect the pod's OS before exec'ing;
   high-security clusters that allow pod exec but *not* node reads now fail
   with `Shell exec failed: no os information available`. Previously it
   assumed Linux/`/bin/sh` —
   [derailed/k9s#3583](https://github.com/derailed/k9s/issues/3583).
   **Direct lesson for our shell-out backend:** don't add RBAC preconditions
   the actual operation doesn't need; assume the minimal path and degrade with
   a hint.
2. **Alpine / no-bash containers.** Pressing `s` tries `/bin/bash`; Alpine has
   only `/bin/sh`, so exec exits 1 and bounces you back to the list —
   [derailed/k9s#1428](https://github.com/derailed/k9s/issues/1428).
   Lesson: shell path must be discovered/parameterized, never hardcoded.
3. **`Shell exec failed` regressions across versions/OS** — recurring:
   [#1424](https://github.com/derailed/k9s/issues/1424) (v0.25.18),
   [#1787](https://github.com/derailed/k9s/issues/1787) (Windows 0.26.x),
   [#979](https://github.com/derailed/k9s/issues/979) ("container must be
   running").
4. **`--kubeconfig` breaks exec.** Starting k9s with an explicit
   `--kubeconfig` makes most views work but **shelling into pods fails** —
   [derailed/k9s#195](https://github.com/derailed/k9s/issues/195). Lesson: the
   kubeconfig path must be threaded to *every* shell-out, not just the initial
   client — directly relevant to our `kubeconfig`/`context` config fields.
5. **kubectl-not-on-PATH → exec fails** with `executable file not found`. k9s
   shells out to `kubectl` for exec and can't find it. **This is the single
   most important operational note for our Q2 kubectl-shell-out decision:** the
   backend is only as reliable as the `kubectl` binary probe. We already probe
   `docker` in `discovery/probes.rs`; k8s must probe `kubectl` the same way and
   fail with a CI-gate-quality hint.
6. **Terminal-state corruption after exit** — exiting an exec'd shell
   (`Ctrl-D`) can leave a blank/unusable pane —
   [warp#1705](https://github.com/warpdotdev/Warp/issues/1705). A TUI-lifecycle
   problem our headless verb surface simply doesn't have.

---

## PART 3 — Mapping to OUR surfaces (VALIDATES / CHALLENGES / ADD-A-FLAG)

### 3.1 Per-surface mapping

| Finding | Verdict | Our surface | Action |
|---|---|---|---|
| Multi-container `logs` fails without `-c` (kubectl#371) | **VALIDATES** | `logs` REUSE+ with `-c`/`--container` (SM §4, §5.1) | Keep `-c`. **ADD-A-FLAG confirm:** default to first container **with a chained hint listing the others** — do the auto-pick kubectl refused to. Higher parity value than kubectl. |
| `--previous` undiscoverable on crash-loop | **VALIDATES** | `logs` `--previous` (new flag, SM §5.1) | Keep. Make `why`/`status` surface a **"container crashed — try `logs … --previous`" hint** automatically when restartCount>0. This is a differentiator. |
| `describe` has no `-o json` | **VALIDATES** (improvement, not just parity) | `describe` NEW into envelope (SM §5.2) | Keep. Reshape describe's spec+events+conditions into `data`. Genuinely better than kubectl. |
| `top` needs metrics-server | **VALIDATES** | `top` NEW, "degrade with clear hint if absent" (SM §5.2) | Keep exactly as mapped — CI-gate-quality "metrics-server not installed" error. |
| jsonpath weak / no regex → `-o json \| jq` | **VALIDATES** | `--select` (jaq) over the envelope | Our jaq `--select` is *strictly better* than kubectl jsonpath. Selling point; document as such in `kubernetes.md`. |
| `--sort-by` brace-form trap + wrong-example bug | **VALIDATES** | `events` newest-first, `audit ls`-style ordering (SM §5.2) | Keep. We fix ordering *in the verb* so agents never touch `--sort-by`. |
| kubectl exit `1` conflates not-found / forbidden / unreachable | **VALIDATES** | `failure_class` + k8s transport taxonomy (SM §2, §10) | Strongly validates the SM §10 model. This is a real gap we close. |
| kubectl `exec` needs `--`; opaque `container not found` | **VALIDATES** | `run`/`exec` REUSE+ | We own arg assembly; agent never writes `--`. Keep. Map `container not found` → clear hint. |
| container exit codes (137/143/139) only in pod status | **ADD-A-FLAG** | `why` / `status` | `why` should **surface `lastState.terminated.exitCode` + reason (OOMKilled/Error)** in `data` — agents currently must jsonpath for it. High-leverage add. |
| k9s node-read-access shell regression (#3583) | **VALIDATES (as anti-pattern)** | `exec`/`run` RBAC | Don't gate exec on unneeded reads; `auth can-i` self-test (SM §5.4) should test the *actual* verbs, not a superset. Ties to CLAUDE.md security-scope-narrower rule. |
| k9s hardcoded `/bin/bash` fails on Alpine (#1428) | **ADD-A-FLAG** | `run`/`exec`/`cat`/`ls` shell path | Don't hardcode a shell; for in-pod commands prefer direct `exec -- <cmd>` (no shell wrapper) or probe `/bin/sh`. Our `run`=`kubectl exec -- <cmd>` (SM §5.1) already avoids this — **VALIDATES** that choice. |
| k9s `--kubeconfig` not threaded to exec (#195) | **VALIDATES** | `kubeconfig`/`context` config fields (SM §3) | Thread `--kubeconfig`/`--context` to **every** shell-out, not just discovery. Make it a Runtime-trait invariant + a test. |
| k9s kubectl-not-on-PATH exec failure | **VALIDATES** | kubectl backend probe (SM §9, Wave A) | Probe `kubectl` like `docker`; fail loud + actionable. Non-negotiable Wave-A item. |
| `port-forward` fragile/idle-drop | **CHALLENGES (scope)** | `port-forward` in SM table 1(a) daily set, **not** in SM verb map | Flagged below — port-forward is not in our §5 verb map. Decide: refuse-with-hint, or a thin passthrough. Its fragility argues *against* owning a long-lived tunnel in v0.1.4. |
| `delete pod` ≠ undo; controller recreates | **VALIDATES** | `delete pod` NEW, `Revert::unsupported` w/ "controller recreates is the revert" preview (SM §5.3, §6) | Exactly right. Keep the preview wording. |
| Manual kubectl triage doesn't scale | **VALIDATES** | `why` k8s deep-bundle (SM §5.1, "highest-leverage") | Confirms `why` as the flagship. Prioritize in Wave C. |
| k9s = TUI, no machine contract | **VALIDATES** | L1 TUI authorized-deferral (CLAUDE.md) | Reconfirms: JSON verbs > dashboard for agents. No action; evidence for the deferral. |

### 3.2 Stress-test of the three JP recommendations

**Q1 — 2-segment selector `<inspect-ns>/<workload>` + kubectl-parity `-n`
override.  → VALIDATED, strongly.**
- kubectl's own model puts *connection* (context, cluster, auth) in
  **kubeconfig**, not on the command line — exactly our "context+ns live in
  config like the SSH host does" argument (SM §4). The parity is real, not
  rhetorical.
- Agents already reach for `-n`/`--namespace` reflexively; giving them the
  same flag is muscle-memory parity. The k9s `--kubeconfig`-not-threaded bug
  (#195) is a *warning*, not a contradiction: it says "whatever addressing you
  choose, thread it to every shell-out." Our config-based context does that in
  one place; a 3-segment selector would spread cluster-addressing into the
  parser and every call site — **more** surface for the #195 class of bug.
- **No contradiction found.** One refinement: because `get` silently ignores
  `-n` on cluster-scoped kinds (nodes/PVs), our `-n` override should be
  **ignored-with-a-note** (not error) for cluster-scoped reads, mirroring
  kubectl but *saying so* — a small ADD to the `-n` help text.

**Q2 — kubectl shell-out backend for v0.1.4 (Runtime trait keeps kube-rs
swap mechanical).  → VALIDATED.**
- Every practitioner artifact above is written in kubectl's idiom; shelling
  out keeps inspect's errors/outputs legible in the exact language agents
  already know — the shell-is-the-integration-layer thesis, confirmed.
- The single hard dependency this introduces — **`kubectl` must be on PATH** —
  is real (k9s issue: kubectl-not-found breaks exec). Mitigation is already
  mapped (SM §9 "probe like docker"). This is a *known, boundable* cost, not a
  surprise. kube-rs would trade it for implementing kubeconfig/exec-cred/OIDC
  auth ourselves — strictly more surface for v0.1.4.
- **Caveat (not a contradiction):** kubectl's **weak exit-code taxonomy** and
  **prose errors** mean the shell-out backend must **parse stderr** to build
  `failure_class` (NotFound / Forbidden / Unreachable / TLS-expired). That
  parsing is a real Wave-A task the SM already anticipates (§10). Flagging it
  so Phase 4 budgets for stderr-classification, which is the one non-trivial
  cost of shell-out over a typed client.

**Q4 — conservative write set `scale` + `rollout restart` + `delete pod` +
in-pod `exec --apply`.  → VALIDATED, with one add-candidate.**
- `scale` is the cleanest revertible write (capture prior `--replicas`,
  inverse = scale back) — kubectl even offers `--current-replicas` as an
  optimistic-concurrency guard we could adopt for a safer apply.
- `rollout restart` has no clean inverse; **`rollout undo --to-revision=N`
  paired with `rollout history` is the real revert primitive** (Part 1a). This
  answers the SM §5.3 open question directly: capture `rollout history`
  revision before restart → `command_pair` inverse = `rollout undo
  --to-revision=<captured>`. That is dispatchable locally (it's `kubectl`, runs
  against the context, not on a remote), so it qualifies as `command_pair`, not
  `unsupported`. **Recommend command_pair over unsupported for `restart`.**
- `delete pod` → `unsupported` is correct (deletion isn't undoable; recreation
  is the controller's job) — VALIDATED.
- **ADD-candidate for JP (Q4):** `rollout undo` is a *first-class safety verb*,
  not just a revert mechanism — an operator's fastest rollback of a bad deploy.
  It's low-risk (moves to an existing prior revision), audit-clean, and its own
  inverse is another `rollout undo`. Worth surfacing to JP as a Q4 addition
  alongside the conservative set. **`cordon`/`drain` should stay out of
  v0.1.4** — node-level, higher blast radius, and drain is genuinely
  destructive/hard to revert; no practitioner signal that they're daily-driver
  enough to justify the risk in the first k8s release.

### 3.3 Contradictions / flags for Phase 4

1. **`port-forward` is unmapped.** It's in kubectl's daily set but absent from
   SM §5. Its documented fragility (idle-drop, single-connection, no
   reconnect) argues it's a *poor fit* for a headless audited verb in v0.1.4.
   **Flag to JP:** refuse-with-hint (point at raw `kubectl port-forward`) vs a
   thin non-owned passthrough. Recommend refuse-with-hint for v0.1.4.
2. **`cp` requires `tar` in-container** and SM §5.3 already leans REFUSE — the
   `tar`-dependency footgun reinforces REFUSE. No contradiction; corroborated.
3. **`describe`/`top`/`events` are `kubectl`-only concepts with no `kube-rs`
   one-liner.** If Q2 ever flips to kube-rs, these three need bespoke
   reimplementation. Not a v0.1.4 problem (we're shell-out) but worth a note in
   the Runtime-trait design so the swap-cost is visible.
4. **Container exit-code surfacing gap** (137/143/139 only via jsonpath) is not
   yet an explicit SM item — recommend Phase 4 add it to `why`/`status` data.

---

## Sources

**kubectl**
- [kubernetes/kubectl#371 — multi-container logs without -c](https://github.com/kubernetes/kubectl/issues/371)
- [Why kubectl logs Doesn't Work with Multi-Container Pods (Medium)](https://medium.com/@ugur.mendi96/why-kubectl-logs-doesnt-work-with-multi-container-pods-and-how-to-fix-it-9f4f4fc5c0d9)
- [SigNoz — kubectl logs guide](https://signoz.io/blog/kubectl-logs/)
- [oneuptime — kubectl logs --previous after crashes](https://oneuptime.com/blog/post/2026-02-09-kubectl-logs-previous-container/view)
- [kubernetes.io blog — 7 Kubernetes pitfalls (2025-10-20)](https://kubernetes.io/blog/2025/10/20/seven-kubernetes-pitfalls-and-how-to-avoid/)
- [kubernetes/kubernetes#21018 — --sort-by wrong example](https://github.com/kubernetes/kubernetes/issues/21018)
- [kubernetes.io — JSONPath support reference](https://kubernetes.io/docs/reference/kubectl/jsonpath/)
- [kubernetes/kubectl#1535 — jsonpath queries not handled](https://github.com/kubernetes/kubectl/issues/1535)
- [kubernetes/website#9606 — jsonpath invalid on Windows](https://github.com/kubernetes/website/issues/9606)
- [cast.ai — Kubernetes exit codes 137/139/143](https://cast.ai/blog/kubernetes-exit-codes/)
- [360cloudplatforms — Kubernetes pain points](https://360cloudplatforms.com/blog/dont-drown-solutions-for-common-kubernetes-pain-points)
- [kubernetes.io — kubectl logs reference](https://kubernetes.io/docs/reference/kubectl/generated/kubectl_logs/)
- [Tasrie IT — kubectl rollout status 2026](https://tasrieit.com/blog/kubectl-rollout-status-deployment-2026)

**k9s**
- [github.com/derailed/k9s](https://github.com/derailed/k9s)
- [derailed/k9s#3583 — shell needs node read access (0.50.12)](https://github.com/derailed/k9s/issues/3583)
- [derailed/k9s#1428 — shell into Alpine container](https://github.com/derailed/k9s/issues/1428)
- [derailed/k9s#1424 — "Shell exec failed" v0.25.18](https://github.com/derailed/k9s/issues/1424)
- [derailed/k9s#1787 — "Shell exec failed" Windows 0.26.x](https://github.com/derailed/k9s/issues/1787)
- [derailed/k9s#195 — shell fails with --kubeconfig](https://github.com/derailed/k9s/issues/195)
- [derailed/k9s#979 — shell "container must be running" v0.24.1](https://github.com/derailed/k9s/issues/979)
- [warpdotdev/Warp#1705 — unable to return to k9s after exec](https://github.com/warpdotdev/Warp/issues/1705)

*Phase 3 worker output (w1). Feeds Phase 4 design + backlog itemization.*
