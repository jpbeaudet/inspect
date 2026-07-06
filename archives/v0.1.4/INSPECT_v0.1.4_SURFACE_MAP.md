# INSPECT v0.1.4 — Kubernetes Surface Map (Phase 2)

**Author:** inspect-subcoord (agentic program, JP-commissioned)
**Date:** 2026-07-04
**Branch:** `feat/v0.1.4-program`
**Status:** Phase 2 of 5 — MAP. Feeds Phase 3 (research validates/challenges this map) → Phase 4 (deep design + backlog itemization) → Phase 5+ (waves).
**Inputs:** `INSPECT_v0.1.4_STUDY.md` (Phase 1), the kube design seed (roadmap v0.2.0 Part 2 + v0.1.3 backlog tail), and the code seam confirmed in `src/`.

This maps **every k8s verb + surface inspect needs**, aligned with the
agentic-first philosophy ("the shell is the integration layer — no MCP
server required"). It is a *map*, not the final design: Phase 3 research
(kubectl/k9s/stern/kubectx/popeye/kail/lens/kubecolor + practitioner
pain points) will pressure-test it, and Phase 4 will itemize it into
`INSPECT_v0.1.4_BACKLOG.md`. Design questions requiring JP are collected
in §12, not silently resolved.

---

## 1. Design principles for the k8s surface

1. **Runtime-agnostic core; additive k8s medium.** The selector grammar,
   JSON envelope, audit/revert contract, exit-code classes, help system,
   and `--select` projection are **runtime-agnostic** and reused
   verbatim. k8s is a new *runtime medium* introduced at the namespace
   level. Docker users see **zero change** (`type = "docker"` default).
2. **The shell stays the integration layer.** Every k8s verb must be
   learnable cold from `inspect <verb> --help` and chainable to the next
   action with no adapter. This bears directly on the backend decision
   (§9): a `kubectl`-shell-out backend keeps inspect's output legible in
   the same idiom operators/agents already know; the abstraction must
   not produce shapes an agent can't predict from `-h`.
3. **Same verb, same contract, new medium.** An agent that knows
   `inspect logs arte/api` must drive `inspect logs staging-k8s/api`
   identically. Divergence is only where k8s *semantics* differ (pod
   immutability, replicas, RBAC), and every divergence is surfaced in
   `-h` + a chained hint.
4. **Conservative write surface.** k8s mutations are powerful and
   cluster-wide. Writes are dry-run-by-default, `--apply`-gated,
   audited, and F11-revert-captured — scoped conservatively (`scale`,
   `rollout restart`, `delete pod`), with genuinely-irreversible ops
   marked `Revert::unsupported` carrying a manual-inverse hint.
5. **No new credential surface.** k8s auth inherits kubeconfig
   (`--context` / `--kubeconfig` / config fields). inspect's SSH /
   keychain credential machinery is untouched.
6. **No silent deferrals.** Anything mapped here that is genuinely
   v0.2.0+ (cross-medium bundle *composition*, per-user RBAC policy
   engine) is named against that release with a reason; everything else
   ships in v0.1.4.

---

## 2. The runtime-abstraction shape (the central build)

Today the docker runtime is assumed structurally (Phase 1 §2.3):
`discovery/probes.rs` builds `docker ps|inspect` and `verbs/dispatch.rs`
builds `docker logs|exec|restart|…`. v0.1.4 introduces a **runtime
executor abstraction** so the same verb dispatches to `docker …` or
`kubectl …` (or a `kube-rs` client) based on the namespace's type.

**Proposed seam (Phase 4 finalizes):** a `Runtime` trait (working name)
that abstracts the command-building + inventory layer, with two impls:

| Concern | Docker impl (exists, refactor to trait) | K8s impl (new) |
|---|---|---|
| Inventory / discovery | `docker ps` + `docker inspect` | `kubectl get pods,svc,deploy,cm -o json` |
| Resolve selector → target | container name | pod / workload (+ container `-c`) |
| Read exec | `docker exec` | `kubectl exec` (read-only) |
| Write exec | `docker exec` (audited) | `kubectl exec` (audited) |
| Logs | `docker logs [-f]` | `kubectl logs [-f] [-c] [--previous]`, fan-out across replicas |
| Lifecycle | `docker restart/stop/start/kill` | `kubectl rollout restart` / `scale` / `delete pod` |
| Transport | SSH ControlMaster to the host | local `kubectl` against kubeconfig context (no SSH) |

**Modules touched:** `config/namespace.rs` (type + k8s fields +
conditional validation), `profile/schema.rs` (`ServiceKind::Pod` /
workload model, or a runtime tag on `Service`), `discovery/probes.rs`
(runtime-dispatched inventory), `verbs/dispatch.rs` (runtime-dispatched
command build), `exec/` (the trait + a k8s transport that is *not* SSH),
`cli.rs` (new verbs + k8s-relevant flags), `help/` (topics + `LONG_*`).

**Transport note (important):** docker namespaces reach the host over
SSH; k8s namespaces run `kubectl` **locally** against the kubeconfig
context. So the k8s runtime bypasses the SSH master entirely — F13
transport exit codes (12–14) and the reauth path are SSH-specific.
k8s needs its **own** transport-failure classification (unreachable
API server, expired token, RBAC-forbidden) mapped onto the same
exit-code discipline and `failure_class` JSON field. This is a
first-class map item, not an afterthought.

---

## 3. Config surface (`servers.toml`)

`NamespaceConfig` today: `host/user/port/key_path/auth/env/...` — the
docker-over-SSH shape. Additions:

```toml
[staging-k8s]
type = "k8s"                       # new; default "docker" when absent
kubeconfig = "~/.kube/staging.yaml"  # optional; else default resolution
context = "staging"                # optional; else current-context
namespace = "default"              # optional; else "default" / -n override
```

- New fields: `type: Option<String>` (`"docker"` | `"k8s"`, default
  docker), `kubeconfig`, `context`, `namespace` — all `Option`, all
  `#[serde(skip_serializing_if = "Option::is_none")]`.
- **`validate()` becomes type-conditional:** docker requires
  `host` + `user` (today's rule); k8s requires **neither** — it requires
  a resolvable context (validated at `setup` / `test` via `kubectl
  config get-contexts` + `kubectl auth can-i`). This is the one behavior
  change to an existing validation path; flagged in CHANGELOG.
- SSH-only fields (`key_path`, `auth`, `password_env`, `session_ttl`,
  `auto_reauth`) are inert for k8s namespaces; `show` renders them as
  N/A rather than erroring.

---

## 4. Selector grammar — the reconciliation (recommendation + JP question)

Two forms appear in the seed docs:

- Roadmap: **2-segment** `staging-k8s/api` — "works the same as
  `arte/api`".
- Backlog tail: **3-segment** `<ctx>/<namespace>/<workload>`.

Today's grammar (confirmed in `src/selector/`) is
`<server> [/ <service>] [: <path>]` where **`server` = a configured
namespace**. For docker, the SSH host + all connection detail live in
**config**, never the selector. Applying that same principle to k8s:

**RECOMMENDATION (for JP ratification, §12-Q1):** keep the **2-segment**
selector `<inspect-namespace>/<workload>`. The kubeconfig **context** and
**k8s namespace** live in the inspect-namespace's **config** (like the
SSH host does for docker), *not* in the selector. This preserves the
"same selectors" guarantee and the uniform grammar — no parser change,
no medium-specific selector shape (which would itself be an LLM-trap:
agents would need to know which segment-count a namespace takes).

For the common "same cluster, many k8s namespaces" need, add a
**kubectl-parity `-n` / `--namespace` override flag** on k8s-capable
verbs (agents already know `kubectl -n`), overriding the config default
for that invocation. This is strictly better than encoding the k8s
namespace in the selector because it (a) matches muscle memory, (b)
keeps the selector grammar frozen, (c) is discoverable from `-h`.

The 3-segment backlog-tail form is thus **not adopted as selector
syntax**; its intent (ctx + k8s-ns + workload addressability) is met by
config + `-n`. If JP wants literal 3-segment selectors, that is a
grammar change to weigh against the uniform-grammar agentic benefit —
hence the §12 question.

Multi-container pods: reuse the existing `:`-less path — a `-c`
/ `--container` flag (kubectl parity) selects the container; default is
the pod's first/only container with a hint when ambiguous.

---

## 5. Verb-by-verb mapping

Legend: **REUSE** = same verb, runtime dispatches command build, no
surface change · **REUSE+** = same verb, k8s adds flags/semantics ·
**NEW** = new verb/subcommand for a k8s concept · **REFUSE** = verb
exists but refuses on k8s with a chained hint.

### 5.1 Read / diagnostic verbs

| Verb | Disposition | k8s mapping |
|---|---|---|
| `status` | REUSE | `kubectl get pods,deploy -o json` → health rollup; pod phase + readiness + restart counts feed the same status model. |
| `health` | REUSE | Pod readiness/liveness probe status from pod conditions. |
| `logs` | REUSE+ | `kubectl logs [-f]`; `-c` for multi-container; `--previous` for crashed containers (new flag, kubectl parity); `--merged` fans `kubectl logs -f` across all replicas of a Deployment/ReplicaSet. |
| `grep` | REUSE | Runs over `kubectl logs` / file reads via `kubectl exec cat`. |
| `cat` | REUSE+ | `kubectl exec <pod> -- cat <path>`; `-c` container select. |
| `ls` | REUSE | `kubectl exec <pod> -- ls`. |
| `find` | REUSE | `kubectl exec <pod> -- find`. |
| `ps` | REUSE | Lists pods (the container-equivalent). `kubectl get pods`. |
| `ports` | REUSE+ | Service ports + container ports from `kubectl get svc`/pod spec (declarative), plus in-pod `ss` where exec is allowed. |
| `volumes` | REUSE+ | PVCs / mounted volumes from pod spec + `kubectl get pvc`. |
| `images` | REUSE | Container images from pod specs (`kubectl get pods -o jsonpath`). |
| `network` | REUSE+ | Services + Endpoints + NetworkPolicies (declarative) rather than docker networks. |
| `why` | REUSE+ | Pod conditions + `kubectl get events` + container restart counts + probe failures + dependency probing — the k8s analogue of the compose-aware deep bundle (F4). This is the highest-leverage diagnostic. |
| `connectivity` | REUSE+ | Service→Endpoint reachability matrix. |
| `run` | REUSE | Read-only `kubectl exec <pod> -- <cmd>`. |
| `exec` | REUSE+ (write) | Writing `kubectl exec` under `--apply`, audited, F11-captured. |
| `watch` | REUSE+ | Block-until-condition over `kubectl get -w` / pod phase / log line / probe. kubectl also has `kubectl wait`; map inspect `watch` semantics onto it where cleaner. |
| `search` | REUSE | LogQL across k8s log/state mediums; `fleet` spans docker + k8s namespaces together. |
| `recipe` | REUSE | Recipes are verb sequences; runtime-agnostic. |

### 5.2 NEW read verbs (k8s-native concepts with no docker analogue)

| Verb | Purpose | Backing |
|---|---|---|
| `describe <ns>/<workload>` | The kubectl-parity deep object dump (events, conditions, spec) in inspect's JSON envelope. | `kubectl describe` / `get -o json` reshaped into the envelope. |
| `events <ns> [/<workload>]` | Cluster/namespace/object events, newest-first (mirrors `audit ls` ordering discipline). | `kubectl get events --sort-by`. |
| `top <ns> [/<workload>]` | Resource usage (CPU/mem) for pods/nodes. | `kubectl top pods/nodes` (requires metrics-server; degrade with a clear hint if absent — CI-gate-quality error). |

Open (Phase 3 research input): whether `get`/`describe` for arbitrary
resource kinds (configmaps, secrets, ingress, jobs) is a v0.1.4 verb or
covered by `describe`/`search`. Research on k9s/kubectl usage patterns
informs scope. **Secrets:** any secret value surfaced must run through
the redaction family — a first-class safety item.

### 5.3 Write / lifecycle verbs

| Verb | Disposition | k8s mapping | Revert (F11) |
|---|---|---|---|
| `restart` | REUSE→remap | `kubectl rollout restart deploy/<w>` (NOT delete-pod). | `Unsupported` (rollout has no clean inverse) OR capture prior `rollout history` revision → `command_pair` `rollout undo`. Phase 4 decides. |
| `scale` (NEW) | NEW | `kubectl scale deploy/<w> --replicas=N`. | `command_pair`: capture prior replica count, inverse = scale back. Cleanly revertible. |
| `delete pod` (NEW, narrow) | NEW | `kubectl delete pod <p>` (controller recreates). | `Unsupported` with hint (deletion isn't undoable; the controller's recreation is the "revert" — state it in preview). |
| `stop` / `start` | REFUSE or map | Pods have no stop/start; `scale --replicas=0` is the idiom. REFUSE with hint pointing at `scale`. Phase 4: refuse vs alias-to-scale. |
| `reload` | REUSE+ | SIGHUP-equivalent → `rollout restart` or config reload; likely alias to `rollout restart` with hint. |
| `edit` | REFUSE | Pods immutable. Hint: "edit the ConfigMap/Secret and run `inspect restart <deploy>` (rollout restart)". |
| `cp` / `put` / `get` | REFUSE (pods) | Pod filesystems are ephemeral. Hint toward ConfigMap/Volume/`kubectl cp` for the rare legit case; primary answer is refuse-with-guidance. Phase 4: whether a narrow `kubectl cp` passthrough is warranted. |
| `chmod`/`chown`/`mkdir`/`touch`/`rm` | REFUSE (pods) | In-pod fs mutation on ephemeral containers is an anti-pattern; refuse with the immutability hint. (In-pod `exec` remains for legit debugging under `--apply`.) |
| `rollout` (NEW subcommand?) | NEW | `rollout status` / `rollout undo` / `rollout restart` as a subcommand tree, or fold into `restart` + `why`. Phase 4 shape decision. |

### 5.4 Namespace-management + setup verbs

| Verb | k8s behavior |
|---|---|
| `add` | Interactive add supports `type = k8s` + kubeconfig/context/namespace prompts. |
| `setup` / `discover` | kubeconfig context check + `kubectl auth can-i` RBAC self-test + `kubectl get pods,svc,deploy,cm -o json` inventory → cached profile. Replaces the docker probe path for k8s namespaces. |
| `test` | `kubectl config get-contexts` + API reachability + `auth can-i` for the verbs inspect will run. |
| `show` / `profile` | Render k8s config (context/namespace) + cached inventory; SSH fields N/A. |
| `connect`/`disconnect`/`connections` | k8s has no persistent SSH session. Either no-op-with-note for k8s namespaces, or repurposed to cache/validate the context. Phase 4 decides; likely a clear "not applicable for k8s (kubeconfig is stateless)" note. |
| `fleet` | Mixed docker + k8s rollup — first-class per the roadmap. |

---

## 6. The k8s write/revert contract (F11 under a new medium)

F11 requires every write verb to capture an inverse before applying.
k8s mutations map onto the existing `Revert` kinds:

- **`command_pair`** — `scale` (capture prior `--replicas`), possibly
  `restart` via `rollout undo` to the captured revision.
- **`unsupported`** — `delete pod` (recreation ≠ revert), irreversible
  ops; preview carries the manual inverse / explanation. Follows the
  CLAUDE.md rule: CLI-only or non-dispatchable inverses are
  `Unsupported`, never a `command_pair` the runner would fail to run.
- **`state_snapshot`** — capture a resource manifest (`kubectl get
  <obj> -o yaml`) before an in-place edit, revert = `kubectl apply` the
  snapshot. Candidate for config/scale where a full manifest round-trip
  is cleaner than a command pair. Phase 4 evaluates.
- **`composite`** — multi-object mutations (bundle steps).

**Audit schema:** k8s writes reuse `AuditEntry` unchanged. Any new field
(e.g. `context` / `k8s_namespace`) is `Option<T>` +
`skip_serializing_if` per the freeze-bound schema rule. The revert
payload for k8s is a `kubectl` command (runs locally, not on a remote) —
the runner dispatches it against the namespace's context.

---

## 7. Discovery mapping

Docker: `docker ps` + batched `docker inspect` (three-bucket timeout
classification). k8s: a single `kubectl get pods,services,deployments,
configmaps -o json` (one API round-trip, no per-object fan-out — the
timeout-batching machinery is docker-specific and not needed). Maps into
the same cached-profile `Service` model: pod → container-equivalent,
Deployment/ReplicaSet → the grouping (analogous to compose project),
Service → ports/network. The cache freshness model (F8: `SOURCE:` line,
`--refresh`, TTL) is runtime-agnostic and reused.

---

## 8. JSON envelope + agentic contract (preserved verbatim)

Every k8s verb emits the same `{schema_version, summary, data, next,
meta}` envelope with the same discriminators (`state`, `failure_class`,
`revert.kind`). `--select '<jq>'` projects it. Chained `hint:` /
`see: inspect help <topic>` trailers on every non-zero exit. New k8s
help topic under `src/help/content/` (e.g. `kubernetes.md`) + per-verb
`LONG_*` additions documenting k8s semantics (immutability refusals,
`-c`/`-n`/`--previous` flags, `top` metrics-server dependency, the k8s
transport failure classes). The help-search index cap bumps if prose
pushes it over (precedent: 50→64→80 KB).

---

## 9. Backend decision framing (kubectl shell-out vs kube-rs)

**Phase 4 decides; Phase 3 research feeds it.** The map of tradeoffs:

| Axis | `kubectl` shell-out | `kube-rs` (Rust client) |
|---|---|---|
| Dependency Policy | zero new crate; relies on `kubectl` on PATH (probe like `docker`) | heavy generated-client + async stack; a real dependency decision |
| Agentic legibility | output/errors in the idiom agents already know; trivially predictable from `-h` | inspect-shaped only |
| Binary size / compile | none | +significant (roadmap flags this) |
| Type safety | string parsing of `-o json` | typed |
| Universality | works anywhere kubectl is installed | self-contained |
| Auth | inherits kubeconfig natively | must implement kubeconfig/exec-cred/OIDC |

The roadmap's own guidance: "**Can ship with `kubectl` backend first,
add `kube-rs` as a feature flag later.**" The Dependency Policy ("prefer
native; only add a crate when the domain is unsafe to reimplement or
years-proven and irreplaceable") + the shell-is-the-integration-layer
philosophy both **lean kubectl-shell-out for v0.1.4**, with the
`Runtime` trait keeping a future `kube-rs` swap mechanical. This is a
strong preliminary lean, ratified in Phase 4 against research + a
binary-size/auth spike, and surfaced to JP (§12-Q2).

---

## 10. k8s transport & auth failure model

k8s namespaces don't use SSH, so they need their own failure taxonomy
mapped onto inspect's exit-code + `failure_class` discipline:

- API server unreachable / kubeconfig missing → transport class (reuse
  12–14 family or a k8s-specific sibling; Phase 4 aligns).
- Token/cert expired → auth class with a hint (`kubectl` re-auth is
  external; no inspect reauth loop — surface the kubectl fix).
- RBAC `forbidden` → a distinct, clearly-hinted class (`auth can-i`
  told you; the verb tells you which permission is missing) — CI-gate
  quality error: what was denied, on which resource, and the `can-i`
  command to check.

Auth inherits kubeconfig entirely — **no new credential surface**,
consistent with the roadmap guarantee.

---

## 11. Bundle-engine integration (the cross-medium seam)

The v0.1.4 charter says bundle integration should make cross-medium
k8s+docker bundles *possible*; the v0.1.3 backlog's "Not part of the
contract" list puts cross-medium *composition* at v0.2.0+. **Map
resolution:** v0.1.4 builds the **seam** — the bundle executor becomes
runtime-aware so a bundle step can target a k8s namespace, and k8s
verbs are bundle-callable with per-step audit + revert. Whether a single
bundle file may *mix* docker and k8s steps (true cross-medium
composition) is the v0.2.0 line. This split (build-seam in v0.1.4, ship-
mixed-composition in v0.2.0) is the natural reconciliation but is a
**JP decision** (§12-Q3), because the two docs point slightly different
directions.

---

## 12. Open questions for JP (do not silently resolve)

- **Q1 — Selector grammar.** Ratify the **2-segment** selector
  `<inspect-ns>/<workload>` with kubeconfig-context + k8s-namespace in
  config and a kubectl-parity `-n`/`--namespace` override flag
  (RECOMMENDED, §4) — versus adopting the backlog-tail 3-segment
  `<ctx>/<namespace>/<workload>` as literal selector syntax (grammar
  change, medium-specific shape). Recommendation: 2-segment + `-n`.
- **Q2 — Backend.** Approve **kubectl shell-out for v0.1.4** with the
  `Runtime` trait preserving a future `kube-rs` swap (RECOMMENDED, §9) —
  versus committing to `kube-rs` now. Recommendation: kubectl-first.
- **Q3 — Cross-medium bundles.** Confirm the split: v0.1.4 builds the
  runtime-aware bundle **seam** (k8s steps callable, per-step
  audit/revert); mixed docker+k8s **composition** in one bundle file is
  v0.2.0+ (§11). Or pull mixed-composition into v0.1.4.
- **Q4 — Write-surface conservatism.** Confirm the initial k8s write
  set is exactly `scale` + `rollout restart` (via `restart`) +
  `delete pod` + in-pod `exec --apply`, with everything else REFUSE-
  with-hint (§5.3). Anything to add (e.g. `cordon`/`drain`,
  `rollout undo`) or hold to v0.2.0?
- **Q5 — Resource breadth.** Is arbitrary-kind `describe`/`get`
  (configmaps/secrets/ingress/jobs/…) in v0.1.4, or is the read surface
  scoped to pods/deploy/svc/events/top with `search` covering the rest?
  (Secrets redaction is mandatory either way.)
- **Q6 — `connect` semantics for k8s.** k8s is sessionless; should
  `connect`/`disconnect` be a clear "N/A for k8s" note, or repurposed to
  context-validate/cache?

---

## 13. Preview of Phase 4 item decomposition (waves)

Not the backlog — a sketch so Phase 3 research targets the right
surfaces and Phase 4 itemizes cleanly. Likely `K<n>` prefix (new medium,
distinct from F/L/S/P).

- **Wave A — foundation:** `Runtime` trait + docker refactor behind it;
  `type`/kubeconfig config fields + conditional validation; kubectl
  backend probe; k8s transport/auth failure model. (Blocks everything.)
- **Wave B — discovery + read core:** k8s `setup`/`test` (inventory +
  `auth can-i`); `status`/`ps`/`health`/`logs`(+`-c`/`--previous`
  /`--merged`)/`cat`/`grep`/`run`. The daily-driver read surface.
- **Wave C — k8s-native reads:** `describe`, `events`, `top`, `why`
  (k8s deep bundle), `ports`/`network`/`volumes`/`images` k8s mappings.
- **Wave D — conservative writes:** `scale`, `restart`(rollout),
  `delete pod`, `exec --apply`, all F11-captured + audited; REFUSE
  mappings for immutable ops with hints.
- **Wave E — integration + polish:** `fleet` mixed rollup, `search`
  across mediums, bundle seam, help topic + `LONG_*` sweep, smoke
  runbook against a real cluster.

Each wave closes only on `cargo check → clippy -D warnings → test` green
+ the 5-surface sweep per item + deferral-scan clean.

---

*Phase 2 complete. Next: Phase 3 — RESEARCH (fan workers w1/w2/w3,
parallel dispatch, synchronous in-turn) on kubectl/k9s/stern/kubectx/
popeye/kail/lens/kubecolor + practitioner pain points touching these
mapped surfaces; integrate findings back into these docs.*
