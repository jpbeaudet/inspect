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

## Live-verified GREEN (no mindtrap) — Wave A so far

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
