# Inspect CLI — Roadmap to v0.2.0 (Stabilization Release)

**Current state:** v0.1.3 in progress (B9 bundle engine + docs in active session)
**Contract:** No backward compatibility until v0.2.0. Breaking changes are free. The tool is shaped by real use (agentic + human review). README says experimental.
**v0.2.0 = the contract begins.** Stable CLI surface, stable JSON schema, stable config format. Breaking changes after v0.2.0 require a major version bump.

---

## What's already done

| Version | What shipped |
|---|---|
| v0.1.0 | Full CLI: 12 read verbs, 12 write verbs, LogQL parser, selectors, aliases, discovery, safety contract, audit log, snapshots, revert, 10 output formats, fleet, recipes, why, connectivity, help system |
| v0.1.1 | `run` verb, `--follow`, `--merged`, `--match`/`--exclude`, `--since-last` cursor, secret masking, `--reason` on audit, progress indicators, exit code surfacing, phantom service fix, help fallback, discovery per-container fallback |
| v0.1.2 (in progress) | B1-B8 polish committed. B9 (bundle engine) + B10 (watch) + docs in active session |

---

## Remaining work (one spec, four releases)

### v0.1.2 (finish current session)

**B9 — Bundle engine.** Declarative YAML grouped ops with preflight/postflight, parallel matrix, `on_failure: rollback_to`, audit grouping via `bundle_id`. Integration with `inspect watch` via `wait:` clause on steps.

Ship when the current session completes. Tag and move on.

---

### v0.1.3 — Clear the debt

Seven items. No backward compatibility concerns — break whatever needs breaking. Four from the known limitations list, three surfaced during v0.1.2 bundle implementation.

#### L1 — TUI mode (`inspect tui`)

Interactive three-pane dashboard via `ratatui`:

```
┌─ Services ─────────────────┬─ Logs ──────────────────────────────────┐
│ ▶ pulse        ✓ healthy   │ [pulse] Request received...             │
│   atlas        ✓ healthy   │ [atlas] Query OK (42ms)                 │
│   synapse      ✗ down      │ [synapse] Connection refused            │
├─────────────────────────────┼─ Detail ────────────────────────────────┤
│                             │ synapse — exited (code 137)             │
│                             │ depends_on: [pulse, atlas, redis]       │
│                             │ Suggested: inspect why arte/synapse     │
└─────────────────────────────┴─────────────────────────────────────────┘
 [q]uit [/]search [f]ollow [m]atch [enter]drill [w]hy [e]xec [?]help
```

- `ratatui` crate. Thin presentation layer over existing verb functions.
- Left: `inspect status` data, refreshed every 10s
- Right-top: `inspect logs --follow --merged` for selected service(s)
- Right-bottom: service detail / `inspect why` output
- Keyboard: j/k navigate, Enter drill, `/` search, `f` follow, `m` match filter, `w` run why, `r` refresh, `q` quit
- Read-only in v0.1.3. Write actions from TUI in v0.2.0+.
- No mouse. No custom layouts. Fixed three-pane.

#### L2 — OS keychain integration

```bash
inspect connect arte --save-passphrase    # store in OS keychain
inspect connect arte                       # auto-retrieves
inspect keychain list                      # show stored
inspect keychain remove arte               # delete
```

- `keyring` crate. macOS Keychain, GNOME Keyring, KDE Wallet, Windows Credential Manager via WSL2.
- Headless/CI: skip silently, fall back to env var or prompt.
- New credential resolution order: socket → user ControlMaster → ssh-agent → **OS keychain** → env var → prompt
- Never store passphrases in inspect's own files.

#### L3 — Parameterized aliases

```bash
inspect alias add svc-logs '{server="arte", service="$svc", source="logs"}'
inspect search '@svc-logs(svc=pulse) |= "error"'

# Alias chaining
inspect alias add prod-pulse '@prod-svc(svc=pulse, src=logs)'

# Agent discovery
inspect alias show svc-logs --json
# → { "name": "svc-logs", "parameters": ["svc"], "type": "logql" }
```

- `$param` in alias body = parameter. Supplied at call site as `@name(key=val)`.
- Missing param → clear error listing required params.
- Chaining: aliases can reference other aliases. Max depth 5. Circular → error at definition time.
- `--json` output for agent programmatic discovery.
- No defaults in v0.1.3 (`${svc:-pulse}` is v0.2.0).

#### L4 — Password authentication

```toml
[legacy-box]
host = "legacy.internal"
user = "admin"
auth = "password"
password_env = "LEGACY_BOX_PASS"
```

- `auth = "password"` in config. Default remains `"key"`.
- Password from env var or interactive prompt. Never stored on disk.
- ControlMaster persistence: password entered once at `inspect connect`, session reused for TTL.
- One-time warning: "password auth is less secure than key-based."
- Max 3 failed attempts then abort.

#### L5 — Audit log rotation/retention policy
**Source:** v0.1.2 bundle implementation retrospective
**Severity:** Low (no scale issue today, but will bite on long-running installations)
**Problem:** Audit log at `~/.inspect/audit/` grows unbounded. Monthly JSONL files are manageable now, but a team running 50 bundle operations a day will accumulate. No rotation, no retention policy, no cleanup command.
**Fix:**
- `inspect audit gc --keep 90d` — delete audit entries + orphaned snapshots older than N days
- `inspect audit gc --keep 10` — keep last N entries per namespace
- Config option in `~/.inspect/config.toml`: `audit_retention = "90d"` for automatic GC on every `--apply` invocation (lightweight — just check the oldest file's date)
- Snapshot directory gets the same treatment: snapshots not referenced by any retained audit entry are orphans and get cleaned
- `inspect audit gc --dry-run` to preview what would be deleted

#### L6 — Per-branch rollback tracking in bundle matrix steps
**Source:** v0.1.2 bundle implementation retrospective
**Severity:** Medium (architectural, affects correctness of parallel rollback)
**Problem:** Bundle matrix steps (`parallel: true` with `matrix`) currently execute all-or-nothing per step. If 4 of 6 volume tars succeed and 2 fail, rollback undoes the entire step — including the 4 successful branches. The correct behavior is: rollback only the branches that succeeded, leave the failed ones alone (nothing to undo).
**Fix:**
- Track per-branch completion status in the bundle executor: `{ branch: "atlas_milvus", status: "ok" }`, `{ branch: "aware_etcd", status: "failed" }`
- On rollback, only execute rollback for branches with `status: "ok"`
- In the audit log, record per-branch status within the step's `bundle_id` entry
- `inspect bundle status <bundle_id>` shows per-branch outcomes
- Requires the rollback block to support branch-aware templating: `{{ matrix.volume }}` in rollback refers to only the succeeded branches

#### L7 — Header/PEM/URL credential redaction in stdout
**Source:** v0.1.2 bundle implementation retrospective
**Severity:** Medium (security, especially for agent workflows)
**Problem:** The current secret masker is line-oriented and pattern-matches `KEY=value` env-var format. It does not catch: HTTP `Authorization: Bearer <token>` headers in curl output, PEM private key blocks in file content, credentials embedded in URLs (`postgres://user:pass@host/db`), or base64-encoded secrets in config files.
**Fix:**
- Add three additional masking patterns alongside the existing env-var masker:
  1. **Header masker:** match `Authorization:`, `X-API-Key:`, `Cookie:`, `Set-Cookie:` — mask the value portion
  2. **PEM masker:** match `-----BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY-----` through `-----END ... KEY-----` — replace entire block with `[REDACTED PEM KEY]`
  3. **URL credential masker:** match `://user:pass@` patterns in connection strings — mask the password portion: `postgres://user:****@host/db`
- All maskers run in sequence on every `inspect run` and `inspect exec` stdout line
- `--show-secrets` bypasses all maskers (existing behavior, extended to new patterns)
- Each masker is a separate module so new patterns are easy to add

#### Implementation order (updated)

```
L4 (password auth)           — small, unblocks legacy servers
L2 (OS keychain)             — small, one crate, quality-of-life
L5 (audit rotation)          — small, maintenance hygiene
L7 (extended secret masking) — medium, security hardening
L3 (parameterized aliases)   — medium, parser change
L6 (per-branch rollback)     — medium, architectural, touches bundle executor
L1 (TUI)                     — largest, ships last when everything underneath is stable
```

---

### v0.1.4 — Pre-stabilization cleanup

This is the "break everything one last time" release. No new features. Pure cleanup before v0.2.0 locks the contract.

#### S1 — CLI surface audit

Review every verb, flag, and argument name. This is the last chance to rename things.

Things to examine:
- Is `inspect run` the right name? Or should it be `inspect shell`? Or `inspect read-exec`? Pick the best name. After v0.2.0, it's permanent.
- Is `--match` / `--exclude` the right flag name on `logs`? Or `--grep` / `--reject`? Check convention parity with rg/stern.
- Is `--merged` the right name? Or `--interleave`? Or is it just the default behavior for multi-service selectors?
- Is `inspect bundle run` the right verb structure? Or `inspect run-bundle`? Or `inspect op`?
- Is `inspect watch` the right name? Or `inspect wait`? (kubectl uses `wait`.)
- Do we need both `inspect tui` and `inspect dashboard`? Pick one.
- Are there any flags that should be renamed, consolidated, or split?

No new features. Just naming. The agent + human review every command and flag against convention sources (kubectl, stern, rg, terraform, ansible) and pick the best name for each.

#### S2 — Config format freeze

Finalize `servers.toml`, `aliases.toml`, `groups.toml`, `recipes/*.yaml`, `bundles/*.yaml`. Review every field name. Add `schema_version` to each config file. After v0.2.0, config changes are migrations.

#### S3 — JSON output schema freeze

Review every `--json` output across every command. Finalize field names, nesting, types. Write a JSON Schema document for each command family (status, logs, search, exec, audit, etc.). After v0.2.0, field removals are breaking changes.

#### S4 — Help text audit

Every help topic, every `--help` output, every error message, every `see:` reference — reviewed for accuracy against the actual v0.1.4 behavior. Examples tested. Translation guide verified.

#### S5 — README + docs rewrite

Replace the "experimental" disclaimer with the v0.2.0 stability promise. Document the contract: what's stable (CLI surface, JSON schema, config format), what's not (internal APIs, undocumented flags). Write the CHANGELOG retroactively for v0.1.0 → v0.1.4.

#### S6 — Dead code + dependency audit

Same pattern as pre-v0.1.0: remove all `#[allow(dead_code)]`, let the compiler scream, triage every warning. Audit `Cargo.toml` for unused dependencies. Minimize the dependency tree before stabilization.

#### S7 — Security audit

Run through the pitfalls document one more time against the current codebase:
- sed injection escaping (§3.1)
- `$field$` map interpolation safety (§3.3)
- Secret masking completeness
- Audit log integrity
- Socket permissions
- Password credential handling (new in v0.1.3)
- Keychain credential handling (new in v0.1.3)
- Bundle execution safety (rollback behavior under adversarial input)

---

### v0.2.0 — Kubernetes + Stability Contract

The contract begins. Two parts ship together:

#### Part 1: Stability contract

```
STABLE (breaking changes = major version bump):
  - CLI verb names and flag names
  - Selector grammar
  - LogQL query syntax and reserved labels
  - --json output schema (versioned via schema_version)
  - Config file formats (servers.toml, aliases.toml, groups.toml)
  - Bundle YAML format
  - Recipe YAML format
  - Audit log schema
  - Exit codes
  - Help topic names

UNSTABLE (may change in minor versions):
  - TUI layout and keybindings
  - Error message wording (structure stable, prose may change)
  - Internal module APIs
  - Performance characteristics
  - Discovery heuristics (what gets detected and how)
  - Correlation rules
```

#### Part 2: Kubernetes support (additive)

```toml
[staging-k8s]
type = "k8s"
kubeconfig = "~/.kube/staging.yaml"
context = "staging"
namespace = "default"
```

- Namespace gains `type` field: `"docker"` (default, existing) or `"k8s"` (new)
- Executor trait: `DockerExecutor` and `K8sExecutor` implement same interface
- Same selectors: `staging-k8s/api` works the same as `arte/api`
- Same verbs: `logs`, `grep`, `run`, `exec`, `status`, `health`, `why`, `watch`, `ps`
- Mixed fleet: `inspect fleet status` shows Docker and k8s namespaces together
- `inspect search` works across both: `{server=~".*", source="logs"} |= "error"`

**K8s-specific changes:**
- Discovery: `kubectl get pods/services/deployments/configmaps -o json` instead of `docker ps/inspect`
- Logs: `kubectl logs` with `-c` for multi-container pods
- `run`: `kubectl exec` for read-only
- `exec --apply`: `kubectl exec` for writes, audit-logged
- `--merged` across pod replicas: fan out `kubectl logs -f` to all replicas of a Deployment
- `why`: Pod conditions + Events + container restart counts + dependency probing
- `edit`/`cp` on pods: **refuse with hint.** Pods are immutable. Hint: "edit the ConfigMap/Secret and run `kubectl rollout restart`"
- `setup`: kubeconfig context check + `kubectl auth can-i` RBAC self-test
- Auth: inherit kubeconfig. `--context` / `--kubeconfig` flags. No new credential surface.
- Per-user policies (deferred from v0.1.3): `allow`/`deny` verb lists per namespace, `require_reason`, RBAC-aware

**K8s backend crate:** `kube-rs` (preferred for type safety and no kubectl dependency) OR shell out to `kubectl` (simpler, universal). Decision made during the k8s bible phase based on binary size and compile time testing. Can ship with `kubectl` backend first, add `kube-rs` as a feature flag later.

**What does NOT change for Docker users:** Nothing. `type = "docker"` is the default. Existing configs, selectors, aliases, recipes, bundles — all work unchanged. k8s is purely additive.

---

## Timeline (realistic, hobby pace)

| Release | Scope | Estimated effort |
|---|---|---|
| v0.1.2 | Bundle engine (finishing) | ~2-3h remaining |
| v0.1.3 | TUI + keychain + param aliases + password auth + audit rotation + extended masking + per-branch rollback | ~12-16h |
| v0.1.4 | Pre-stabilization cleanup + audit | ~4-6h |
| v0.2.0 | K8s + stability contract | ~10-15h (bible: 3h, impl: 7-12h) |

**Total to v0.2.0: ~30-40h from now.**

At that point, the README drops "experimental," the contract is real, and the tool works on Docker VMs and Kubernetes clusters. Your CEO's Claude Code can use it on staging.

---

## Intentional scope boundaries (not debt, not planned)

These are permanent "no" answers, not deferred features:

- **No distributed tracing viewer.** Different data plane. LogQL already surfaces trace IDs.
- **No russh fallback.** OpenSSH is everywhere.
- **No remote agent.** Local-first is the feature. Zero install on targets.
- **No Windows native host.** WSL2 is the answer.
- **No LLM integration.** The tool is the surface. LLMs drive it from outside.
