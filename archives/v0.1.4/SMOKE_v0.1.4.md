# SMOKE — v0.1.4 (Kubernetes) release-readiness gate

The field-validation gate for the k8s medium. Every k8s verb is exercised
against a **real cluster** (not synthetic fixtures). Run after the last K-item
ships; a clean P0→P10 PASS is a tag precondition (with the full-sweep audit,
per `INSPECT_v0.1.4_IMPLEMENTATION_PLAN.md` §6).

## Conventions an agent running this smoke MUST know

- **Two clusters wired on the box.** `~/.kube/maker.yaml` (maker zone, context
  `z2-maker`) and `~/.kube/hub.yaml` (z00 hub). `kubectl` is on PATH.
- **HARD GUARDRAILS (non-negotiable):** READ verbs are free on both clusters.
  **MUTATING verbs run ONLY inside the namespace `inspect-livetest` on MAKER.**
  NEVER mutate/delete anything in `luminary-*` namespaces or at node level. HUB
  is CI infra — **reads only**. The mutating phase (P9) is gated: do not run it
  without explicit human authorization for that session.
- **Isolate config.** `export INSPECT_HOME=/tmp/inspect-smoke-k8s` and write a
  0600 `servers.toml` there — never touch the operator's real `~/.inspect`.
- **Exit-code contract:** 0 ok · 1 no-match/not-found · 2 usage/preflight ·
  13 transport_unreachable · 14 transport_auth/rbac_forbidden · 15
  metrics_unavailable · 16 no_shell_in_container.

## P0 — pre-flight (no cluster)

| id | command | pass |
|---|---|---|
| P0.1 | `export PATH=$HOME/.cargo/bin:$PATH; cargo fmt --check` | exit 0 |
| P0.2 | `cargo clippy --all-targets -- -D warnings` | exit 0 |
| P0.3 | `cargo test 2>&1 \| grep -E "^test result\|^running" \| tail -40` | every suite `0 failed` |
| P0.4 | `cargo install --path . --force` | installs `inspect` |
| P0.5 | `inspect --version` | matches `Cargo.toml` |
| P0.6 | deferral-scan grep (CLAUDE.md) | every hit legitimate |

## P1 — config + discovery (read-only, maker)

```sh
export INSPECT_HOME=/tmp/inspect-smoke-k8s; rm -rf $INSPECT_HOME; mkdir -p $INSPECT_HOME
inspect add makersys --type k8s --context z2-maker --kubeconfig ~/.kube/maker.yaml \
  --namespace kube-system --non-interactive --force        # exit 0
inspect add nocxt   --type k8s --kubeconfig ~/.kube/maker.yaml --non-interactive --force
  # exit 2 — WA-6: k8s namespace requires an explicit --context
inspect show makersys                                        # renders type/context/kubectl:<ver>
inspect show makersys --json | jq .context                  # "z2-maker" (valid JSON)
inspect test makersys                                        # config/kubectl/api/rbac/metrics all pass
inspect setup makersys                                       # discovers pods -> cached profile
inspect connect makersys                                     # N/A (sessionless) exit 0
```

## P2 — status / ps / health (read-only)

```sh
inspect status makersys            # "N pod(s): N healthy, ..." rollup
inspect status "makersys/coredns*" # glob filter
inspect ps makersys                # pod list (name/image/phase)
inspect health makersys            # per-pod [OK]/[BAD] probes
```

## P3 — logs

```sh
POD=$(KUBECONFIG=~/.kube/maker.yaml kubectl get po -n kube-system -o jsonpath='{.items[0].metadata.name}')
inspect logs "makersys/$POD" --tail 5                  # real logs, redacted
inspect logs "makersys/$POD" --previous                # last-terminated (or clean "no previous")
# multi-container pod: auto-picks first container + hints the others
```

## P4 — in-pod reads (kubectl exec)

```sh
SH=local-path-provisioner-...            # a pod WITH a shell (alpine-based)
inspect cat "makersys/$SH:/etc/hostname" # file contents, exit 0
inspect ls  "makersys/$SH:/etc"          # dir listing
inspect grep root "makersys/$SH:/etc/passwd"   # match, exit 0
inspect run "makersys/$SH" -- echo hello # "hello", exit 0
# distroless pod (coredns): each -> [no_shell_in_container] EXIT 16
inspect cat "makersys/coredns-...:/etc/resolv.conf"    # exit 16
```

## P5 — k8s-native diagnostics

```sh
inspect why makersys/coredns-...     # phase + restarts + exit-reason + events, severity rollup
inspect describe makersys/coredns-...            # object dump in envelope
inspect describe makersys/coredns-... --json --select '.data.pod.status.phase'  # "Running"
inspect events makersys              # newest-first (0 if quiet; renders on a busy ns)
inspect top makersys                 # per-pod CPU/mem (or exit 15 if no metrics-server)
```

## P6 — inventory mappings

```sh
inspect network makersys   # Services (type/clusterIP/ports)
inspect ports makersys     # Service ports
inspect volumes makersys   # PVCs (0 in kube-system is fine)
inspect images makersys    # unique pod images (from profile)
```

## P7 — fleet (mixed rollup)

```sh
inspect fleet status --ns makersys   # 1 ok, renders the k8s rollup inside fleet
# (with a docker namespace also configured: mixed docker+k8s rollup together)
```

## P8 — write DRY-RUNS (read-only — no mutation, any namespace)

Every write is dry-run by default; verify the resolved-target echo + captured
revert without mutating:

```sh
inspect scale   makersys/coredns --replicas 2   # DRY RUN + "revert: scale back to --replicas=1"
inspect restart makersys/coredns                # DRY RUN + rollout-restart + revert preview
inspect rollout makersys/coredns                # DRY RUN + "revert: rollout undo --to-revision=N"
inspect delete  makersys/coredns-...            # DRY RUN + "revert: unsupported — ReplicaSet recreates it"
inspect exec    makersys/$SH -- touch /tmp/x    # DRY RUN + revert: unsupported
# refusals (exit 2):
inspect stop  makersys/coredns                  # -> use scale --replicas 0
inspect chmod makersys/coredns:/etc/x 644       # -> immutable-pod hint
```

## P9 — MUTATING round-trip (GATED: `inspect-livetest` on maker ONLY)

> **Do not run without explicit authorization.** All mutations are confined to
> the throwaway `inspect-livetest` namespace and cleaned up in P10.

```sh
kubectl --context z2-maker create namespace inspect-livetest 2>/dev/null || true
kubectl --context z2-maker -n inspect-livetest create deployment smoke-nginx \
  --image=nginx --replicas=1
inspect add lt --type k8s --context z2-maker --kubeconfig ~/.kube/maker.yaml \
  --namespace inspect-livetest --non-interactive --force
inspect setup lt

# scale up, then revert back to 1 via the captured inverse
inspect scale lt/smoke-nginx --replicas 3 --apply --yes    # scaled; audit records revert
REV=$(inspect audit ls --json --select '.data.entries[0].id' --select-raw)
inspect revert "$REV" --apply --yes                        # runs `kubectl scale --replicas=1` LOCALLY
# assert: kubectl get deploy smoke-nginx -> 1 replica again

# rollout restart + revert; delete a pod (controller recreates)
inspect restart lt/smoke-nginx --apply --yes
POD=$(kubectl --context z2-maker -n inspect-livetest get po -o jsonpath='{.items[0].metadata.name}')
inspect delete "lt/$POD" --apply --yes                     # controller recreates it
```

Pass = every apply exits 0, each audit entry carries `context`/`k8s_namespace`
+ a revert, and `inspect revert --apply` restores prior state via a **local**
kubectl (WD-2). No mutation escapes `inspect-livetest`.

## P10 — cleanup (idempotent)

```sh
kubectl --context z2-maker delete namespace inspect-livetest --wait=false 2>/dev/null || true
rm -rf /tmp/inspect-smoke-k8s
```

## Sign-off

- P0–P8 clean against maker (reads + write dry-runs).
- P9 clean in `inspect-livetest` (mutations + local revert round-trip), when
  authorized.
- P10 leaves the cluster as found. Then: Cleaning Duty, archive sweep, and the
  ROOT-dispatched full-sweep audit (0-Critical/0-High) before the tag.
