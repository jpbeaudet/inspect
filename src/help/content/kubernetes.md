# Kubernetes (v0.1.4)

inspect drives Kubernetes the same way it drives Docker/SSH: the same
selectors, the same JSON envelope, the same audit + revert contract. k8s is a
namespace **type** — purely additive, `type = "docker"` users see no change.

## Configure a k8s namespace

```toml
[staging-k8s]
type       = "k8s"
context    = "staging"                 # REQUIRED — inspect pins this on every call
kubeconfig = "~/.kube/staging.yaml"    # optional; else kubectl's default resolution
namespace  = "default"                 # optional; the in-cluster namespace
```

Or: `inspect add staging-k8s --type k8s --context staging --namespace default`.
Auth inherits your kubeconfig — inspect adds **no new credential surface**.

**Prerequisite:** `kubectl` must be on your PATH (inspect shells out to it
locally; it never runs over SSH for k8s). `inspect show <ns>` reports the
detected version; action verbs fail with a four-question error if it's absent.

## Addressing — and the no-wrong-cluster guarantee

Selectors stay two-segment: `staging-k8s/<pod-or-workload>`. The kubeconfig
**context** and **namespace** live in config, not the selector — like the SSH
host does for a docker namespace. inspect pins `--context` explicitly on
**every** kubectl call and **never reads or mutates your ambient
`current-context`**, so a `kubectx` switch in another terminal can never
redirect an inspect verb at the wrong cluster (the #1 kubectl destruction
class). Use `-n <ns>` / `-A` for kubectl-parity namespace scoping.

## Read verbs

`setup` (discover pods + RBAC self-test + metrics probe), `test` (validate
reachability/RBAC), `status` / `ps` / `health` (pod rollup), `logs`
(`-c <container>`, `--previous`, `--merged`), `cat` / `ls` / `grep` / `run`
(via `kubectl exec`), `why` (deep diagnostic), `describe` (object → envelope,
`--select`-projectable — better than kubectl's text-only describe), `events`
(newest-first, auto-scoped), `top` (CPU/mem), `ports` / `network` / `volumes`
/ `images`.

`inspect logs <ns>/<deploy>` auto-picks the first container on a
multi-container pod (listing the others) rather than erroring like kubectl.
On a CrashLoop, add `--previous` for the crashed instance's logs. `--select`
(jaq) over the envelope is strictly better than kubectl jsonpath — no regex
limits, no brace-quoting traps.

## Write verbs (conservative, audited, revertible)

Dry-run by default; `--apply` to enact. Every write echoes the resolved
`{context, namespace, workload}` in the preview, prompt, and audit entry.

- `scale <ns>/<deploy> --replicas N` — cleanest revert (scale back to the
  captured prior count). `--replicas 0` trips an outage interlock.
- `restart <ns>/<deploy>` — `kubectl rollout restart`; revert = rollout undo
  to the captured revision.
- `rollout <ns>/<deploy> [--to-revision N]` — `kubectl rollout undo`, the fast
  rollback of a bad deploy.
- `delete <ns>/<pod>` — narrow pod deletion; the controller recreates it (that
  IS the revert). A naked pod (no controller) warns of permanent loss.
- `exec <ns>/<pod> --apply --no-revert -- <cmd>` — audited in-pod write.

`inspect revert <audit-id> --apply` runs the captured `kubectl` inverse
locally against the recorded context.

**Immutable-pod ops REFUSE** with an idiom hint: `edit`/`chmod`/`chown`/
`mkdir`/`touch`/`rm`/`cp` — pod filesystems are ephemeral. Edit the
ConfigMap/Secret and `inspect restart <deploy>`; use `exec --apply` for live
debugging. `stop`/`start` → `scale --replicas 0/1`.

## Exit codes (agent branch-points)

The exit code is the coarse class; the JSON `failure_class` carries the
detail. Transport band (parallel to the SSH F13 band 12–14):
`transport_unreachable` → 13, `transport_auth_failed`/`rbac_forbidden` → 14.
Operational-degradation band: `metrics_unavailable` → 15 (metrics-server
absent), `no_shell_in_container` → 16 (distroless pod). `not_found` /
`unknown` → 1. RBAC-forbidden errors embed the exact `kubectl auth can-i`
command to run.
