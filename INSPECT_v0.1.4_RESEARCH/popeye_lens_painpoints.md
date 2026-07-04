# INSPECT v0.1.4 — Research: Popeye / Lens + Practitioner Pain-Point Hunt

**Worker:** inspect-w3 (jp-tree) · **Date:** 2026-07-04
**Feeds:** Phase 3 research → validates/challenges `INSPECT_v0.1.4_SURFACE_MAP.md`
**Method:** WebSearch/WebFetch over official docs, GitHub issues, vendor blogs, K8s failure-story collections. Citations inline.

---

## PART A — Tool study (diagnostic tools that inform our verbs)

### A.1 Popeye — cluster resource sanitizer (informs `why`, `health`, `describe`)

**What it is.** A read-only utility that scans a *live* cluster (not YAML on disk) and reports issues with deployed resources + their configuration. It "sanitizes based on what's deployed, not what's sitting on disk." ([derailed/popeye](https://github.com/derailed/popeye), [popeyecli.io](https://popeyecli.io/))

**What it scans / surfaces (the linter classes):**
- **Misconfigurations** — port mismatches (Service targetPort ≠ container port), image concerns (`:latest` tags, no digest).
- **Dead / unused resources** — "naked" resources (pods with no controller), unused ConfigMaps/Secrets, dangling objects, orphaned PVs.
- **Resource hygiene** — missing CPU/memory **requests & limits**, over/under-utilization vs metrics.
- **Probe gaps** — absent readiness / liveness probes.
- **RBAC issues** — references to non-existent ServiceAccounts, over-broad rules.
- Anti-patterns generally (single-replica deployments, no PodDisruptionBudget, etc.).
([Ajeet Raina / Medium](https://ajeetraina.medium.com/popeye-a-kubernetes-cluster-sanitizer-1914723eb21d), [kubernetes.qa Popeye vs Polaris](https://kubernetes.qa/blog/popeye-vs-polaris/))

**Output shape (directly relevant to our JSON envelope + `state`/`failure_class` design):**
- Grouped by resource type; each finding has a **severity level** (OK / Info / Warn / Error, rendered 🟢🔵🟠🔴).
- A per-section and overall **Popeye Score** (grade A–F / 0–100) — a *rollup*, exactly the shape our `status`/`health` rollup uses.
- Machine outputs: `-o json`, `-o yaml`, `-o html`, Prometheus push, `--save` to a report dir (`POPEYE_REPORT_DIR`, `--output-file`). It is designed for both human and pipeline consumption. ([popeyecli.io](https://popeyecli.io/), [Docker Hub derailed/popeye](https://hub.docker.com/r/derailed/popeye))

**Mapping to OUR surfaces:**
- **`why` is our Popeye analogue at object scope.** Popeye validates the *content* of a good `why` bundle: for a failing workload, surface (1) missing/failed probes, (2) missing resource requests/limits, (3) port/target mismatches, (4) restart counts, (5) RBAC references, (6) events. Our §5.1 `why` map (pod conditions + events + restart counts + probe failures + dependency probing) should **explicitly add**: probe *presence* check, requests/limits presence, and image-tag hygiene — Popeye shows operators expect these in a "what's wrong here" answer.
- **Severity-graded rollup → `state` discriminator.** Popeye's OK/Info/Warn/Error grading is a proven agent-legible shape; our envelope's `state`/`summary` should carry an equivalent worst-severity rollup so an agent branches without parsing prose.
- **Scope boundary:** Popeye is *cluster-wide audit*; our v0.1.4 `why` is *object-scoped diagnostics*. A cluster-wide "sanitize" verb is **not** in the v0.1.4 map and should stay a v0.1.5+ candidate — noting it here so it is a stated boundary, not a silent gap. Popeye is the reference design when it lands.

### A.2 Lens — Kubernetes IDE (informs `logs --merged`, `describe`, `events`, `top`)

**What it is.** A desktop IDE that uses `kubectl` under the hood + the user's kubeconfig; "when you interact with resources in Lens, it runs the equivalent kubectl commands in the background." It is positioned for *observability + troubleshooting*, with kubectl reserved for *scripting/automation*. ([Lens overview / Medium](https://medium.com/k8slens/lens-kubernetes-ide-overview-how-to-simplify-kubernetes-management-e19a9aea7dae), [spacelift.io](https://spacelift.io/blog/lens-kubernetes), [Flavius Dinu](https://techblog.flaviusdinu.com/lens-kubernetes-ide-how-to-simplify-k8s-management-in-2025-185c058a13a7))

**What it surfaces that validates our map:**
- **Cross-replica log search** — "you can search for 'Error' across the logs of **all replicas in a deployment simultaneously**," with live streaming + filtering out of the box. → **Directly validates our `logs --merged` (fan-out across replicas)** as a first-class need, not a nicety. The pain it solves is exactly our §5.1 `logs` REUSE+ mapping.
- **Live resource + events + CPU/mem in one pane** — validates bundling `describe`/`events`/`top` as the daily-driver read core (our Wave B/C).
- **Lens Prism (AI copilot)** — "understand what's happening inside clusters, see why workloads failed, how to fix them." This is *literally the agentic `why` use case* — a market signal that an LLM-consumable "why did this fail + how to fix" bundle is where the puck is going. Our JSON `why` + chained `hint:` is the shell-native version of Prism; the design bet is validated.

**Backend confirmation for Q2.** Lens (and the market leader in this space) shells out to `kubectl` under the hood rather than embedding a Rust/Go client. That is a strong external precedent for our **kubectl-shell-out lean (Q2 / §9)** — the tool operators trust for observability delegates to kubectl for legibility + auth inheritance, exactly our argument.

### A.3 Adjacent diagnostic tools (brief)

- **`kubectl debug` / ephemeral containers** (stable since k8s 1.25) — the platform answer to "distroless pod has no shell." `kubectl exec -it pod -- /bin/bash` fails on distroless with `stat /bin/bash: no such file or directory`; the fix is injecting an ephemeral debug container sharing the target's namespaces. Tools like **cdebug** wrap it with ergonomic defaults (auto-set fs root to target). ([alexandre-vazquez.com](https://alexandre-vazquez.com/debugging-distroless-containers/), [k8s docs: Ephemeral Containers](https://kubernetes.io/docs/concepts/workloads/pods/ephemeral-containers/), [Chainguard/cdebug](https://dev.to/chainguard/debugging-distroless-images-with-kubectl-and-cdebug-1dm0)) → **See P5 below — this is a real pain that stress-tests our `exec`/`cat`/`ls` REUSE map.**
- **kubens/kubectx** (Ahmet Alp Balkan) — the de-facto safety layer for the wrong-context/wrong-namespace footgun. Its *existence and popularity* is the evidence for pain themes P3 + P3b. ([plural.sh switch-namespace](https://www.plural.sh/blog/kubectl-switch-namespace-guide/))

---

## PART B — Practitioner pain-point hunt (ranked by recurrence)

Ranked most-recurring first. Each: the pain, evidence, and the **design implication for inspect**.

### P1 — Wrong-context / wrong-namespace destruction (THE #1 kubectl horror class) 🔴🔴🔴

**Pain.** "The wrong context can mean deploying to production instead of dev." A context = (cluster, user, namespace); it "tells kubectl whether it's supposed to tear down your local test environment or the production database." The default-namespace variant: "If an engineer forgets which namespace is currently active, destructive operations (like `kubectl delete deployment X`) may be mistakenly executed against the wrong environment (e.g. production instead of staging)." An entire tool ecosystem (kubens/kubectx, `$KUBECONFIG` swapping, "keep dangerous environments in a separate kubeconfig") exists *only* to mitigate this.

**Evidence.** [natkr — Wrangling Kubernetes contexts](https://natkr.com/2025-11-14-kubernetes-contexts/), [K8s Failure Stories / HN](https://news.ycombinator.com/item?id=26106080), [kubernetes.recipes context management](https://kubernetes.recipes/recipes/configuration/kubectl-config-context-management/), [openillumi default-namespace footgun](https://openillumi.com/en/en-kubectl-namespace-permanent-switch/), [ahmetb/kubectx#135 — accidental context delete](https://github.com/ahmetb/kubectx/issues/135). Recurs in essentially every "kubectl best practices" and every failure-story collection.

**Implication for inspect (STRONGLY validates our design):**
1. **Our Q1 recommendation (2-segment selector, context+namespace in config) is the correct anti-footgun design.** By binding the kubeconfig **context** to a named inspect-namespace in `servers.toml` (not floating global `current-context`), inspect *eliminates the "which context am I in?" ambiguity at the root.* `inspect scale staging-k8s/api` is unambiguous in a way `kubectl scale deploy/api` is not. **This is a headline safety argument to put in the CHANGELOG + help.**
2. **Every write verb MUST echo the resolved target (context + k8s-namespace + workload) in the dry-run preview and the confirmation prompt** — "will scale `deploy/api` in namespace `prod` on context `prod-eks`." The single most-cited mitigation is "confirm which cluster+namespace before acting"; inspect should *do that for the operator*, in the audit record and the preview, always.
3. **`-n`/`--namespace` override must never silently inherit a stale default** — it overrides *the config default for that invocation only*, which is safer than kubectl's sticky `set-context --namespace`. Our §4 flag design is right; document that it is per-invocation.
4. Validates the **`failure_class` + resolved-target-in-envelope** requirement: an agent must be able to read back *which cluster it just hit* from `meta`.

### P2 — RBAC "Forbidden" opacity (auth-can-i is the fix) 🔴🔴🔴

**Pain.** "Error messages always include the authenticated identity … but **never tell you which binding is missing**." "Binding sprawl becomes opaque in large clusters." The universal remediation is `kubectl auth can-i` (and `auth can-i --list --as=...`). RBAC is "confusing for most — Roles, ClusterRoles, bindings, API groups, verbs like `watch` that don't mean what you think."

**Evidence.** [oneuptime — RBAC permission denied](https://oneuptime.com/blog/post/2026-02-09-rbac-permission-denied-serviceaccounts/view), [prodopshub — "How I finally fixed Permission Denied"](https://prodopshub.com/kubernetes-rbac/), [markaicode](https://markaicode.com/errors/kubernetes-permission-denied-fix/), [AWS EKS RBAC troubleshooting](https://repost.aws/knowledge-center/eks-troubleshoot-rbac-errors), [k8s RBAC docs](https://kubernetes.io/docs/reference/access-authn-authz/rbac/).

**Implication (validates §10 + the "CI-gate-quality error" mandate):**
- Our §10 RBAC failure class is **necessary, not optional**. The forbidden-error message must be *reshaped* into inspect's four-question gate: **what was denied** (verb), **on which resource/namespace**, **which identity**, **what to run to confirm** — literally embed the `kubectl auth can-i <verb> <resource> -n <ns>` command in the `hint:` trailer. This is the highest-value error-message upgrade in the whole k8s surface because kubectl's own message is the specific thing everyone complains about.
- Our `setup`/`test` `auth can-i` self-test (§5.4) directly pre-empts P2: run `can-i` for the verbs inspect will use, and report the gaps *before* the operator hits a mid-task forbidden. Keep it.
- `failure_class = "rbac_forbidden"` must be a distinct discriminator (not folded into generic transport), so agents branch to "escalate permissions" vs "retry."

### P3 — Multi-container / `--previous` / crashed-pod log retrieval confusion 🔴🔴

**Pain.** On a multi-container pod, bare `kubectl logs pod` errors because "Kubernetes will not know which container you want." Crashed-container logs require `-p`/`--previous`, and the *combination* `kubectl logs pod -c container --previous` is what you actually need for a CrashLoopBackOff post-mortem — but operators routinely don't know to combine them, and lose the crash logs (the previous instance is gc'd). Identifying *which* container crashed requires a gnarly `-o jsonpath='{range .status.containerStatuses[*]}...'` incantation.

**Evidence.** [oneuptime — kubectl logs --previous](https://oneuptime.com/blog/post/2026-02-09-kubectl-logs-previous-container/view), [kodekloud kubectl logs guide](https://kodekloud.com/blog/kubectl-logs/), [DigitalOcean Q&A — logs of crashed pods](https://www.digitalocean.com/community/questions/how-to-check-the-logs-of-running-and-crashed-pods-in-kubernetes), [k8s docs — determine reason for pod failure](https://kubernetes.io/docs/tasks/debug/debug-application/determine-reason-pod-failure/).

**Implication (validates §5.1 `logs` REUSE+ and raises the bar):**
- Our `logs --previous` + `-c` + `--merged` map is exactly right and **field-validated as a top-3 pain**. Go further:
  - When a pod is CrashLooping, `logs` (and `why`) should **auto-hint** "container `X` restarted N times; showing current logs — add `--previous` for the crashed instance's logs." Do the container-status jsonpath *for* the operator so they never write it.
  - On a multi-container pod with no `-c`, **do not error like kubectl** — either default to the app/first container *with a hint listing the others*, or list containers and ask. Our §4 "default to first with a hint when ambiguous" is the right call; make the hint enumerate the container names + restart counts.
  - `why` should embed the per-container `restartCount` + `lastState.terminated.reason` table (the jsonpath everyone copy-pastes) as structured JSON.

### P4 — Scale / rollout / delete-pod safety (accidental outage class) 🔴🔴

**Pain.** "Using `kubectl scale` or manual pod deletion can cause outages — this method causes a full service outage between the scale-down and scale-up steps." Consensus best practice: **prefer `kubectl rollout restart` (rolling, zero-downtime) over `delete pod` or scale-to-0/scale-up**, and "avoid manual deletion." The safety net for rollouts is `rollout history` + `rollout undo`.

**Evidence.** [spacelift — restart pods](https://spacelift.io/blog/restart-kubernetes-pods-with-kubectl), [plural.sh — rollout restart the right way](https://www.plural.sh/blog/kubectl-rollout-restart-deployment/), [CNCF — guide to restarting pods](https://www.cncf.io/blog/2025/12/01/a-guide-to-restarting-pods-in-kubernetes-using-kubectl/), [middleware.io — 5 ways to restart](https://middleware.io/blog/kubectl-restart-pod/).

**Implication (VALIDATES Q4 conservative write set + §5.3 mappings):**
- Our decision to map `restart` → `rollout restart` (**NOT** delete-pod) is field-correct — the community explicitly warns against the delete/scale approach for restarts. Keep it.
- Our `restart` revert via captured `rollout history` revision → `rollout undo` (`command_pair`) is **strongly supported** — `rollout undo` *is* the community's documented safety net. Prefer the `command_pair`(capture prior revision → `rollout undo`) option over `Unsupported` where a revision exists; this is a genuine, dispatchable inverse. (§6 / §5.3 leaves this to Phase 4 — **research says: capture the revision.**)
- `scale` revert (capture prior replica count → scale back) is clean and correct.
- `delete pod` staying **narrow + `Unsupported`-revert with the "controller recreates it" explanation in preview** is right — but the preview must warn about the outage window if the pod is the *last* ready replica. Add a guard: if deleting the pod would drop ready replicas below the deployment's threshold, escalate the confirmation.
- `stop`/`start` → **REFUSE-with-hint pointing at `scale --replicas=0`** is validated (scale-to-0 is the documented idiom, and the outage warning applies).

### P5 — `exec` into ephemeral/immutable/distroless pods (surprise class) 🔴🔴

**Pain.** `kubectl exec -it pod -- /bin/bash` fails on distroless/minimal images ("no such file or directory"). Pod filesystems are ephemeral; in-pod fs mutation doesn't survive a restart. The modern answer is `kubectl debug` + ephemeral containers, not exec.

**Evidence.** [alexandre-vazquez — debugging distroless](https://alexandre-vazquez.com/debugging-distroless-containers/), [k8s docs — ephemeral containers](https://kubernetes.io/docs/concepts/workloads/pods/ephemeral-containers/), [oneuptime — kubectl debug distroless](https://oneuptime.com/blog/post/2026-02-09-kubectl-debug-distroless-pods/view).

**Implication (stress-tests §5.3 REFUSE mappings — mostly validated, one gap):**
- Our REFUSE-with-immutability-hint for `edit`/`cp`/`chmod`/`chown`/`mkdir`/`touch`/`rm` on pods is field-correct — in-pod fs mutation is an anti-pattern and won't persist. Keep the refuse + "edit the ConfigMap/Secret and `inspect restart`" hint.
- **Gap:** our `run`/`exec`/`cat`/`ls` REUSE map assumes a shell in the container. On distroless pods, `kubectl exec -- cat/ls/sh` **fails** the same way. inspect must:
  - Detect the "no shell / no coreutils" exec failure and **not** surface a raw OCI error. Emit a `failure_class = "no_shell_in_container"` with a hint pointing at ephemeral-container debug.
  - Phase-4 question surfaced by research: should v0.1.4 offer a narrow `kubectl debug`-backed path (or at least a documented hint) for distroless? Recommend: **hint in v0.1.4, first-class ephemeral-debug verb as v0.1.5+ candidate** (stated boundary, not silent). This is a real, common failure our `cat`/`ls`/`run` map would otherwise trip on during smoke.

### P6 — `kubectl top` fails when metrics-server absent (esp. EKS) 🔴🔴

**Pain.** `kubectl top pods/nodes` → `error: Metrics API not available`. metrics-server is **not installed by default on many clusters (EKS especially)**. Also fails transiently for ~1min after install (still collecting), and on RBAC/network misconfig. Repeatedly filed as confusing because the error looks like a kubectl bug, not a missing-component condition.

**Evidence.** [kubernetes-sigs/metrics-server#1282](https://github.com/kubernetes-sigs/metrics-server/issues/1282), [#1061](https://github.com/kubernetes-sigs/metrics-server/issues/1061), [#1558](https://github.com/kubernetes-sigs/metrics-server/issues/1558), [CloudSpinx fix guide](https://medium.com/@cloudspinx/fix-error-metrics-api-not-available-in-kubernetes-aa10766e1c2f), [bobcares — EKS top not available](https://bobcares.com/blog/kubectl-top-metrics-api-not-available-eks/).

**Implication (validates §5.2 `top` degrade requirement):**
- Our "`top` requires metrics-server; degrade with a clear hint if absent — CI-gate-quality error" is **exactly right and non-optional**. The hint must: (a) name the condition (`metrics-server not installed / not ready`), (b) distinguish *absent* from *just-started* (`retry in ~60s`), (c) give the install command, (d) set a distinct `failure_class = "metrics_unavailable"` so an agent doesn't treat it as a hard failure of the workload. Do NOT let `top` return a raw kubectl error.
- `setup`/`test` should probe metrics-server availability and record it in the cached profile, so `top` can pre-answer "this cluster has no metrics."

### P7 — Events hard to correlate / not chronologically ordered 🔴

**Pain.** `kubectl get events` output is "all mixed up" — "by default, events are **not guaranteed to be in chronological order**," making it hard to see where a newly-deployed pod is failing. Requires `--sort-by='.lastTimestamp'` (or `.metadata.creationTimestamp`) + `--field-selector reason=FailedScheduling` incantations. Events also expire (~1h default retention), and correlating events↔logs↔metrics is manual.

**Evidence.** [kubernetes/kubernetes#29838 — events don't sort by last seen](https://github.com/kubernetes/kubernetes/issues/29838), [oneuptime — events filtering/sorting](https://oneuptime.com/blog/post/2026-02-09-kubectl-events-filtering-sorting/view), [plural.sh — get events guide](https://www.plural.sh/blog/kubectl-get-events-guide/).

**Implication (validates §5.2 `events` ordering-discipline):**
- Our `events` "newest-first (mirrors `audit ls` ordering discipline)" is **directly validated** — the #1 events complaint is unstable ordering. inspect must **always sort deterministically newest-first** and document it in `LONG_EVENTS` (same discipline as `LONG_AUDIT_LS`). This is a concrete "inspect fixes a named kubectl pain" win.
- `events <ns>/<workload>` should **auto-scope to the object** (do the field-selector for the operator) and **auto-correlate** into `why` — events are most useful joined to the failing workload's restart counts + probe status, which is precisely the `why` bundle. Feed events into `why`.
- Note the ~1h retention: `why`/`events` should hint when the relevant window may have expired.

### P8 — "Missing" pod logs / wrong-pod logs of a deployment (replica addressing) 🔴

**Pain.** "Deployment logs showed success but the application logs came from a **completely different pod** with an older image version" — the "which pod of the deployment am I actually looking at" problem. A rolling update leaves old+new ReplicaSets; `kubectl logs deploy/x` picks *one* pod, and operators debug the wrong one. Detective-story-class confusion.

**Evidence.** [Menotebo — Detective Story of Missing Pod Logs](https://medium.com/@menotebo/kubernetes-logging-a-detective-story-of-missing-pod-logs-0ea9f6be34a2), reinforced by the Lens "search across all replicas" feature existing to solve it.

**Implication (validates `logs --merged` + replica addressing):**
- Our `--merged` fan-out across replicas is the fix — and inspect should **label each merged log line with its source pod + which ReplicaSet/revision** so the "old pod, old image" trap is visible, not hidden. Lens surfaces per-replica; inspect's merged stream must carry the pod identity in the envelope/prefix.
- When addressing `logs <ns>/<deploy>` without `--merged`, the output must **state which pod it selected and how many replicas exist** ("showing 1 of 3 replicas: pod `api-abc` (revision 7); use `--merged` for all"). Never silently pick one.

---

## Cross-cutting stress-test of JP's open questions

**Q3 — bundle seam v0.1.4 / mixed-composition v0.2.0.** No pain evidence *demands* mixed docker+k8s composition in one file for v0.1.4 — the recurring pains (P1–P8) are all single-medium k8s diagnostics/writes. The seam (k8s steps callable + per-step audit/revert) is enough to serve every pain found. **Research supports the split**: build the seam now, defer mixed-composition to v0.2.0. Caveat surfaced for JP: the *migration* use case ("drain docker service, bring up k8s equivalent") is where mixed-composition would shine — if JP's field users are mid-migration, that raises v0.2.0's priority but does not move it into v0.1.4.

**Q4 — conservative write set.** **Strongly validated** by P4. The community explicitly warns against the exact ops we're *excluding* (raw delete-pod-as-restart, scale-cycling) and endorses the ops we're *including* (`rollout restart` + `rollout undo` as the safety net, `scale` with revertible replica capture). Recommend one addition to consider (Phase 4 / JP): the outage-guard on `delete pod` / `scale --replicas=0` (escalate confirmation when it would drop below ready-replica threshold). `cordon`/`drain` stay out of v0.1.4 (node-level, no pain evidence in scope).

**Q6 — k8s connect semantics.** P1 reframes this usefully: kubeconfig is stateless, but the *dangerous* stateful thing in kubectl-land is the **sticky current-context / current-namespace**. inspect's win is that it *doesn't* carry sticky global state — each verb resolves target from the named namespace's config. So `connect`/`disconnect` should be a clear **"N/A for k8s (kubeconfig is stateless — target is resolved per-verb from config; no sticky context footgun)"** note. That framing turns the "no-op" into a *stated safety property*, which is better than silence. Recommend the N/A-with-safety-note wording over repurposing.

---

## Summary of design-validated changes to feed back into the map

1. `why` should add: probe-presence, requests/limits-presence, image-tag hygiene, and the per-container `restartCount`/`lastState.terminated.reason` table (Popeye + P3).
2. Every k8s **write preview + audit record echoes resolved context+namespace+workload** (P1) — headline anti-footgun property.
3. RBAC forbidden → four-question gate with the exact `auth can-i` command in the hint; distinct `failure_class` (P2).
4. `logs`/`why` auto-hint `--previous` on CrashLoop; enumerate containers on ambiguity; label merged lines with pod+revision (P3, P8).
5. `restart` revert = capture `rollout history` revision → `rollout undo` `command_pair` (not `Unsupported`) (P4).
6. Detect distroless/no-shell exec failure → `failure_class="no_shell_in_container"` + ephemeral-debug hint; ephemeral-debug verb is a stated v0.1.5+ boundary (P5).
7. `top` degrade with distinct `failure_class="metrics_unavailable"`, install hint, absent-vs-warming distinction; probe metrics-server at `setup` (P6).
8. `events` always newest-first (documented), auto-scoped, fed into `why`; hint on retention-window expiry (P7).
9. `connect`/`disconnect` = "N/A for k8s (stateless, no sticky-context footgun)" — a stated safety property (Q6).
