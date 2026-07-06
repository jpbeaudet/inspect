# SMOKE_v0.1.4 — live results (2026-07-06)

Full P0–P10 run of `SMOKE_v0.1.4.md` against the **real maker cluster**
(context `z2-maker`, `~/.kube/maker.yaml`), driving the **`cargo install`-ed**
binary (not a debug/target build). P9 mutations confined to a throwaway
`inspect-livetest` namespace, authorized by JP for this session.

Binary: `cargo install --path . --force` → `inspect 0.1.3` (the 0.1.4
version-bump is root's release step). kubectl: v1.36.2.

| Phase | Scope | Result |
|-------|-------|--------|
| **P0** | fmt/clippy/test + install + version + deferral-scan | ✅ PASS — fmt clean, clippy `-D warnings` clean, 1429 tests / 0 failed, binary installed, `--version` matches Cargo.toml, deferral-scan hits all legitimate |
| **P1** | config + discovery (maker) | ✅ PASS — add(k8s) exit 0; add without `--context` → exit 2 (WA-6); show renders type/context/`kubectl v1.36.2`; `show --json \| jq .context` → `z2-maker`; `test` all pass (config/kubectl/api/rbac/metrics); setup discovered 7 containers; connect → N/A sessionless exit 0 |
| **P2** | status / ps / health | ✅ PASS — "7 pod(s): 7 healthy"; glob `coredns*` filters to 1; ps + health render (the exit-141 on head-truncated pipes is the correct SIGPIPE behavior, not a verb failure) |
| **P3** | logs | ✅ PASS (with SMOKE-1) — `--tail 5` real redacted logs exit 0; `--previous` on a never-restarted pod → exit 1 + a *generic* not_found hint (SMOKE-1: should be a clean "no previous instance") |
| **P4** | in-pod reads | ✅ PASS — cat/ls/grep/run exit 0 with correct output; distroless coredns → exit 16 `[no_shell_in_container]` (WA-4) |
| **P5** | k8s-native diagnostics | ✅ PASS — why (phase+rollup), describe (envelope), `describe --json --select '.data.pod.status.phase'` → `"Running"`, events (newest-first envelope), top (per-pod cpu/mem, metrics-server present). H4 envelope + `--select` + H1 scrub all verified live |
| **P6** | inventory | ✅ PASS — network (9 svc), ports (14), volumes (0 PVC, fine), images (6 unique) |
| **P7** | fleet mixed rollup | ✅ PASS (with SMOKE-2) — exit 0, "1 ok"; k8s rollup renders. Cosmetic: prewarm emits a docker-centric "has no host" skip note on stderr for the k8s namespace (SMOKE-2) |
| **P8** | write DRY-RUNS + refusals | ✅ PASS — scale/restart/rollout/delete/exec all dry-run exit 0, each echoes the resolved `{namespace 'kube-system', context 'z2-maker', workload}` (H3) + a correct revert preview; `stop`/`chmod` refuse exit 2 with chained idiom hints |
| **P9** | MUTATING round-trip (`inspect-livetest`, authorized) | ✅ PASS — `scale --replicas 3 --apply` → 3 (verified); audit entry records `context=z2-maker`, `k8s_namespace=inspect-livetest`, `revert.kind=command_pair` with a self-contained `--kubeconfig`+`-n` payload (H3 + WD-1 live); `revert --apply` restored replicas to 1 via local kubectl (WD-2); `rollout restart --apply` captured its undo; `delete pod --apply` → controller recreated it. All exit 0, nothing escaped `inspect-livetest` |
| **P10** | cleanup | ✅ PASS — `inspect-livetest` deleted (Terminating), smoke config removed, all `luminary-*` namespaces untouched. Cluster left as found |

## Findings (both minor, agent-message quality on the k8s surface)

- **SMOKE-1 [Low] — `logs --previous` on a never-restarted pod emits a
  misleading hint.** kubectl returns "previous terminated container not
  found"; inspect classifies it as `not_found` and prints the generic hint
  *"the addressed object does not exist … check the name and `-n`/`--namespace`"*
  (exit 1). The pod exists — there is simply no previous instance. The hint
  points the operator at the wrong cause; the runbook's P3.2 pass condition
  expects a clean "no previous" outcome. Fix: detect the "no previous"
  kubectl shape and emit a clear message.
- **SMOKE-2 [Low, cosmetic] — fleet prewarm docker-centric skip note for
  k8s.** `fleet status` over a k8s namespace prints `fleet: prewarm: ns
  'makersys' skipped (target: namespace 'makersys' has no host)` on stderr.
  Correct behavior (k8s is sessionless, no SSH prewarm), but "has no host"
  reads as a spurious error for a k8s namespace. Fix: skip prewarm silently
  for k8s namespaces (or with a k8s-appropriate note).

Neither is a functional failure (both phases pass with correct output + exit
codes), but both are agent-facing message-quality issues on the new k8s
surface — fixed per the LLM-trap-fix-on-first-surface rule.
