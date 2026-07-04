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

## Live-verified GREEN (no mindtrap) — Wave A so far

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
