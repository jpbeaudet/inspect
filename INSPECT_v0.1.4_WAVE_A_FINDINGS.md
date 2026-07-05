# INSPECT v0.1.4 — Wave A live-test findings + crit-bug log

Durable log of findings from live-testing Wave A against the real maker
cluster (`~/.kube/maker.yaml`, context `z2-maker`, ns `inspect-livetest`)
via the installed `inspect` binary. Per the LLM-mindtrap=CRIT-BUG rule:
any behavior that would mislead/trap an agent is filed here immediately and
fixed **within the wave**. Ranked: crit-bugs first.

Status legend: `🟥 Open crit-bug · 🟧 In progress · ✅ Fixed · 📝 coverage/polish`.

---

## WA-1 — `inspect add` success message reports the WRONG config path (mindtrap) ✅ Fixed

**Surfaced:** K2 live test, 2026-07-04. `INSPECT_HOME=/tmp/k2-livetest inspect
add maker --type k8s …` printed:

```
SUMMARY: namespace 'maker' added in ~/.inspect/servers.toml
```

…but it actually wrote to `/tmp/k2-livetest/servers.toml` (the `INSPECT_HOME`
override, correctly honored — verified: real `~/.inspect/servers.toml` does not
exist). The success message **hardcodes `~/.inspect/servers.toml`** instead of
the resolved path.

**Why it's a crit-bug (agent mindtrap):** an agent (or operator) that reads
"added in ~/.inspect/servers.toml" and then goes to inspect/edit that file finds
nothing there — the reported location is a lie whenever `INSPECT_HOME` is set.
Silent path divergence between "what I did" and "what I told you I did" is
exactly the trap class the philosophy targets.

**Scope:** likely pre-existing (docker `add` shares the string) but it is on the
`add` surface K2 touches, so it is in-wave. Sweep: any other message that
hardcodes `~/.inspect/...` while a resolved path exists (`inspect remove`,
`connect`, cache/audit paths in success/hint text).

**Fix:** report the **resolved** `servers.toml` path (via `paths::` resolution
that already honors `INSPECT_HOME_ENV`), not a literal `~/.inspect/...`. Add a
regression test `wa1_add_reports_resolved_config_path_under_inspect_home`.

---

## WA-2 — K2 acceptance test-name/coverage gap 📝

**Surfaced:** K2 gate, 2026-07-04. The plan (`INSPECT_v0.1.4_IMPLEMENTATION_PLAN.md`
§5 K2) names five acceptance tests; the committed set
(`tests/phase_k_v014.rs`) has 5 k2_* tests but not the exact named ones —
present: `k2_show_renders_ssh_fields_na_for_k8s`,
`k2_k8s_namespace_shows_without_host_user`, `k2_unknown_type_is_rejected`
(+2). **Missing meaningful coverage:**
- `k2_docker_namespace_still_requires_host_user` — the *negative* half of the
  type-conditional validate (a docker ns without host/user must still fail).
- `k2_type_defaults_to_docker_when_absent`.
- `k2_schema_version_bumped` (SCHEMA_VERSION=2 is present in code but untested).

**Fix:** add the three tests under their spec names (the behavior appears
correct; this is coverage alignment, not a functional bug). Keep the extra
tests. Low effort.

---

## WA-3 — `inspect show <k8s-ns>` hard-fails on absent kubectl, breaking the config-read JSON contract 🟧 (design-review; K6-gated)

**Surfaced:** K3 live test, 2026-07-04. K3 wired the kubectl preflight into
`inspect show` (the only k8s-reachable surface before K6). Live behavior:
- kubectl present → `inspect show maker` exit 0, renders `kubectl: v1.36.2`. ✅
- kubectl absent → `inspect show maker` **exit 2**, four-question error on stderr,
  **empty stdout**. Same for `inspect show maker --json`.

**The concern (borderline mindtrap, --json case):** `show` is fundamentally a
*config read* ("show a namespace's resolved configuration"). Hard-failing it when
the kubectl **backend** is absent conflates two concerns — *display the config I
wrote* vs *is the backend ready*. Under `--json`, an agent doing
`inspect show maker --json | jq .context` to read the configured context gets
**empty stdout + exit 2 + a non-JSON stderr blob** — the JSON contract for a pure
config read is broken by an unrelated backend check. The config is right there on
disk; refusing to display it is surprising.

**Why not a crit-bug (and why it's not fixed this turn):** the error itself is
loud, specific, actionable, correct-exit (not misleading) — and `show` was the
*only* k8s-reachable surface in K3 (setup/test/read verbs land in K6+). The
hard-fail-somewhere requirement (`k3_k8s_verb_fails_loud_when_kubectl_absent`)
needed a reachable verb this turn; `show` was the pragmatic choice.

**Recommended resolution (named unblock = K6):** when K6 lands `test`/`setup`
(the natural preflight verbs), **move the hard four-question fail there**, and
make `show` **always display the config + a `kubectl: <version>` / `kubectl: NOT
FOUND — <fix hint>` readiness line** — never hard-fail a pure config read, and
never break `--json`. `show` reports readiness; `test`/`setup`/read/write verbs
enforce it. Tracked to K6 (a real, named unblock — not a silent deferral);
surfaced to root/JP for the call.

---

## WA-4 — k8s operational-failure exit codes (open decision, flagged to JP) 📝

**Surfaced:** K4 design, 2026-07-04. The K4 exit-code policy (recorded on
`KubectlFailure`): transport reuses the F13 band by semantic class
(`transport_unreachable` → 13, `transport_auth_failed` → 14);
`rbac_forbidden` → 14 (authorization failure; `failure_class` distinguishes
it from a credential expiry); `not_found` → 1 (no-match). The **open
decision**: `metrics_unavailable` / `no_shell_in_container` currently map
to a coarse exit 1 (general non-match) with the precise `failure_class`
carrying the detail. Whether these operational classes deserve a
**dedicated exit code** (vs exit-1-plus-failure_class) is a contract
decision for JP. The `exit_code()` accessor itself lands in K6 with its
first verb-exit consumer; the policy is recorded now so the decision isn't
lost. **Not a blocker.**

## WA-5 — `inspect test` ran SSH-only checks on a k8s namespace (mindtrap) ✅ Fixed

**Surfaced + fixed in K4, 2026-07-04.** Before K4, `inspect test <k8s-ns>`
ran the docker/SSH check set (key_file, tcp) and would report a k8s
namespace as failing with "no key_path configured" / "no host configured"
— nonsensical, misleading noise for a kubeconfig target. K4 added a k8s
branch (config + kubectl backend + context-pinned API reachability) and a
k8s-specific text emit (no `host:port` line; sessionless NEXT hint). Fixed
in the same item that needed `test` as its classifier consumer.

---

## WA-6 — a context-less k8s namespace falls through to the ambient context (footgun) 🟧 (K6-gated)

**Surfaced:** K5, 2026-07-04. `K8sRuntime::scope_flags` only emits
`--context` when a context is configured (`Some`). K2 made `context`
optional at config time (resolvability deferred to setup/test). So a k8s
namespace added **without** a context would build kubectl commands with **no
`--context`**, letting kubectl fall through to the ambient `current-context`
— exactly the footgun K5's invariant exists to prevent. `inspect show` also
renders `<current-context>` for such a namespace, *implying* inspect uses
the ambient context.

**Why not fixed in K5:** the enforcement point is `setup`/`test` (K6) —
where inspect resolves + validates the context against the live cluster.
**Recommended (K6):** `setup`/`test` must **require** an explicit context
for a k8s namespace (or resolve-and-record one, never leaving it ambient),
and the runtime should refuse to build a command without a pinned context.
Tracked to K6 (named unblock). Not a blocker for K5, whose invariant holds
for every *configured* context.

---

## Live-verified GREEN (no mindtrap) — Wave A so far

- **K5 context-pinning invariant** — verified by an exhaustive test over
  every `K8sRuntime` command builder (`--context` pinned on inventory /
  read-exec / write-exec / restart / reload / stop / start) plus a
  source-scan test that fails the build if any `kubectl config
  current-context / use-context` call is ever introduced. `AuditEntry`
  `context` / `k8s_namespace` fields round-trip through JSON and are omitted
  for docker entries. (Live end-to-end echo in verb `meta` lands with the
  read verbs in K6+; the invariant itself is structural + test-enforced.)

- **K4 failure classifier + `inspect test` k8s branch** live-passes against
  maker: `inspect test maker` → all checks pass (`config`, `kubectl v1.36.2`,
  `api API server reachable`), exit 0, sessionless NEXT hint; `inspect test`
  with a **bad context** → `[transport_unreachable]` + the reachability
  hint, exit 2. **The real-cluster fixtures caught two genuine classifier
  bugs the synthetic assumptions missed:** (1) kubectl's bad-context message
  is `context "X" does not exist`, not the `Error in configuration …` shape
  first assumed; (2) `context was not found` was being misread as an object
  `NotFound` (fixed by ordering transport before NotFound + guarding
  NotFound on `from server`). This is the no-synthetic-verification rule
  earning its keep.

- **K3 kubectl backend probe** live-passes against maker via the installed binary:
  present → `inspect show maker` reports `kubectl: v1.36.2` (exit 0); absent
  (`PATH=/usr/bin:/bin`) → **exit 2** with the four-question error (what/where/why/
  fix + the install URL + `kubectl version --client` verify command) — **no raw OS
  error, correct exit class**. The probe is a **local** spawn (`kubectl version
  --client -o json`), never over SSH (surface map §10). Docker namespaces are
  unaffected. (One design-review caveat filed as WA-3 above.)

- **K2 config surface** live-passes against maker: `inspect add maker --type k8s
  --context z2-maker --kubeconfig ~/.kube/maker.yaml --namespace inspect-livetest`
  → exit 0, clean envelope; `inspect list` shows it; `inspect show maker` renders
  `type: k8s`, context/kubeconfig/namespace, and every SSH-only field as
  `N/A (k8s)` (not an error) → exit 0. **This is the correct, agent-legible
  shape.**
- **`inspect add --help`** documents the k8s flags *and* the anti-footgun
  property ("the kubeconfig context inspect pins on every call — it never reads
  your ambient current-context") — the K5 invariant already surfaced in help.
- **`INSPECT_HOME` isolation honored** — no real user config clobbered (guardrail
  respected).
- **K1** (Runtime trait + docker refactor) committed `a547eda`; docker suite
  green; retroactive live pass deferred to when a k8s read verb exists to
  exercise it (K1 is internal — no user-facing k8s verb yet).

---

*WA-1 + WA-2 to be fixed within Wave A (next worker dispatch or folded into the
K3 dispatch). Fixing WA-1 also swept across other hardcoded-path messages.*
