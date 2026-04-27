# Inspect CLI — v0.1.3 Patch Backlog

**Rule:** Ship when 7+ items accumulated OR one critical issue found.
**Source:** Known limitations from pre-v0.2.0 roadmap + retrospective items surfaced during v0.1.2 bundle implementation.
**Status:** **OPEN** — 0 / 7 shipped. v0.1.3 is the last "break things freely" release before the v0.1.4 stabilization sweep and the v0.2.0 contract.
**Contract:** No backward compatibility. Break whatever needs breaking. After v0.2.0 the CLI surface, JSON schema, and config formats are frozen.

| Item | Status | Notes |
|---|---|---|
| L4 — Password auth + session TTL + `ssh add-key` helper | ⬜ Open | small, unblocks legacy servers; bundles the key-migration helper |
| L2 — OS keychain integration (opt-in, cross-session only) | ⬜ Open | small, one crate (`keyring`); default stays ssh-agent / per-session |
| L5 — Audit log rotation / retention | ⬜ Open | small, maintenance hygiene |
| L7 — Header / PEM / URL credential redaction | ⬜ Open | medium, security hardening |
| L3 — Parameterized aliases | ⬜ Open | medium, parser change |
| L6 — Per-branch rollback in bundle matrix | ⬜ Open | medium, architectural — touches bundle executor |
| L1 — TUI mode (`inspect tui`) | ⬜ Open | largest, ships last |

**Implementation order:** L4 → L2 → L5 → L7 → L3 → L6 → L1. Smaller / lower-risk items first, TUI last when everything underneath is stable.

---

## Backlog (7 items)

### L4 — Password authentication + extended session TTL + `ssh add-key` helper
**Source:** Roadmap "Remaining work" / known limitation; key-only auth blocks integration with shared bastions and legacy boxes that still require password auth. Refined during v0.1.3 backlog review: password auth is only acceptable if (a) the session TTL is long enough that passwords are not re-prompted within a working session, and (b) there is a one-command path off password auth onto keys.
**Severity:** Medium (blocks adoption on legacy infrastructure).
**Problem:** `inspect connect` only supports key-based SSH auth. Legacy servers, locked-down bastions, and password-only managed appliances cannot be onboarded today. Operators must shell out to plain `ssh` and lose the audit trail / ControlMaster / discovery cache. Even when password auth is available, re-prompting every ~4h within a long working session would make the feature unusable.
**Fix:**
- New per-server config field `auth = "password"` in `~/.inspect/servers.toml`. Default remains `"key"`.
- Password sources, in order: `password_env = "VAR_NAME"` → interactive prompt at `inspect connect`. Never stored on disk.
- **Session TTL.** When `auth = "password"`, the OpenSSH ControlMaster `ControlPersist` defaults to `12h` (was effectively the agent / system default, often 4h). Configurable per-server via `session_ttl = "24h"`. Cap at 24h so a forgotten laptop doesn't hold a live remote session indefinitely; document the cap. Key-auth servers keep the existing default unless explicitly overridden — only password auth gets the longer default, because re-prompting passwords is the painful case.
- Password entered once at `inspect connect`; every subsequent `inspect <verb>` reuses the ControlMaster socket without re-prompting until TTL expires or the user runs `inspect disconnect`.
- **`inspect ssh add-key <ns>` helper** (new verb). Wraps `ssh-copy-id`: generates a key pair under `~/.ssh/inspect_<ns>_ed25519` if none specified, copies the public key to the remote `authorized_keys` over the current (password) session, then offers to flip the server's `auth = "password"` config to `auth = "key"` with `key_path = "..."`. One command from "password-only legacy box" to "first-class key-auth namespace".
  - `--key <path>` to use an existing key instead of generating one.
  - `--no-rewrite-config` to skip the auth-flip prompt (just install the key).
  - Audit-logged like any other write: `verb=ssh.add-key, target=<ns>`.
- One-time warning printed at first password connect: `warning: password auth is less secure than key-based. Run 'inspect ssh add-key <ns>' to migrate.`
- Max 3 failed password attempts then abort with a clear chained hint (point at `inspect help ssh`).
- Update `inspect connectivity` to surface password-auth status, current session TTL, and remaining session lifetime in its triage output.

```toml
[legacy-box]
host = "legacy.internal"
user = "admin"
auth = "password"
password_env = "LEGACY_BOX_PASS"   # optional; falls back to prompt
session_ttl = "12h"                # optional; default 12h for password, capped at 24h
```

**Test:**
- `inspect connect legacy-box` with `LEGACY_BOX_PASS` set succeeds without a prompt; unset, prompts once and connects.
- After connect, `inspect logs legacy-box/<svc>` reuses the session without re-prompting.
- ControlMaster persists for the configured TTL; verify by checking the socket exists 5 minutes after connect and a fresh command does not re-prompt.
- `session_ttl = "48h"` is rejected with a clear error explaining the 24h cap.
- `inspect ssh add-key legacy-box` over a live password session: generates a key, installs it, prompts to flip config; after flip, `inspect connect legacy-box` succeeds with key auth and never prompts.
- 3 wrong passwords aborts cleanly with non-zero exit and a hint.
- `inspect connect` against a `auth = "key"` server is unchanged (regression guard).
- `inspect connectivity legacy-box` shows `auth: password`, `session_ttl: 12h`, `expires_in: 11h47m`.

---

### L2 — OS keychain integration (opt-in, cross-session only)
**Source:** Roadmap "Remaining work" / known limitation. Refined during v0.1.3 backlog review: keychain is **only useful for cross-session persistence**. Within a single shell session, ssh-agent + ControlMaster already gives "enter passphrase once and reuse for the rest of the session" — that is the default, expected behavior and most users want exactly that (passphrase gone on logout, re-entered next session). Keychain is the opt-in for the smaller group who want passphrases to survive reboot.
**Severity:** Low (opt-in convenience for a subset of users; not on the default path).
**Problem:** A subset of operators want SSH passphrases to persist across shell sessions and reboots without leaving them in env vars, `.envrc` files, or shell history. There is no OS-native option today.
**Default behavior is unchanged and remains the recommended path:** ssh-agent holds the passphrase for the life of the shell session; logout / reboot clears it; next session prompts once again. This is what most users want and the docs should say so explicitly.
**Fix:**
- Add the `keyring` crate (single dep, cross-platform). Backends: macOS Keychain, GNOME Keyring (Secret Service), KDE Wallet, Windows Credential Manager (via WSL2).
- **Opt-in only.** No automatic keychain use. New explicit flag: `inspect connect <ns> --save-passphrase` — prompts once, stores in OS keychain under service `inspect-cli`, account `<namespace>`. Without the flag, behavior is exactly as in v0.1.2 (ssh-agent / per-session prompt).
- Subsequent `inspect connect <ns>` auto-retrieves silently **only if the namespace was previously saved with `--save-passphrase`**.
- New management verbs:
  - `inspect keychain list` — show stored namespaces (no values).
  - `inspect keychain remove <ns>` — delete entry.
  - `inspect keychain test` — verify the OS backend is reachable; exit 0/non-zero with a clear hint when keychain is unavailable.
- New credential-resolution order: socket → user ControlMaster → ssh-agent → **OS keychain (only if `--save-passphrase` was previously used for this ns)** → env var → prompt.
- Headless / CI: skip keychain silently if backend unavailable, fall back to env var or prompt. No hard dep on a running keychain daemon.
- **Inspect never writes passphrases to its own files**, ever. Keychain is the one persistent path, and only when explicitly opted into.
- Docs (`docs/MANUAL.md`) get a short "credential lifetime" section that spells out the three options: (1) **default** — ssh-agent, one prompt per shell session; (2) **`--save-passphrase`** — OS keychain, persists across sessions and reboots; (3) **env var** — for CI / scripted use only.

**Test:**
- Default path (no `--save-passphrase`): `inspect connect arte` behaves identically to v0.1.2 — agent if loaded, otherwise prompt; nothing written to keychain.
- `inspect connect arte --save-passphrase` stores; subsequent connects in a fresh shell don't prompt.
- `inspect keychain list` shows `arte`; `inspect keychain remove arte` deletes; subsequent connect prompts again.
- Headless container with no keychain backend: `inspect connect --save-passphrase` warns once and falls back to per-session prompt; `inspect keychain test` exits non-zero with a hint.
- Order test: a stored keychain entry for `arte` is consulted only because `arte` was previously saved; a fresh ns `bravo` ignores the keychain entirely (no implicit cross-namespace lookups).

---

### L5 — Audit log rotation + retention policy
**Source:** v0.1.2 bundle implementation retrospective.
**Severity:** Low (no scale issue today, but will bite long-running installations and bundle-heavy workflows).
**Problem:** `~/.inspect/audit/` grows unbounded. Monthly JSONL files are manageable today, but a team running 50 bundle ops per day will accumulate years of entries plus orphaned snapshot directories. There is no rotation, no retention policy, no cleanup verb, and no snapshot GC.
**Fix:**
- New verb `inspect audit gc` with two retention modes:
  - `--keep <duration>` (e.g. `90d`, `4w`, `12h`) — delete audit entries older than N.
  - `--keep <N>` — keep the last N entries per namespace.
  - `--dry-run` — preview what would be deleted (counts + total bytes freed).
- Snapshot directory gets the same treatment in the same pass: any snapshot dir under `~/.inspect/snapshots/` not referenced by a retained audit entry is an orphan and gets cleaned (subject to `--dry-run`).
- New config option in `~/.inspect/config.toml`:
  ```toml
  [audit]
  retention = "90d"     # or "100" for entry count; unset = no automatic GC
  ```
  When set, GC runs lightweight on every `--apply` invocation (just checks the oldest file's mtime; no full scan unless rotation is needed).
- `inspect audit gc --json` for machine consumers (counts, freed bytes, deleted snapshot ids).
- Clear, loud output on what was deleted and what was kept; never silently delete on an unrecognized retention value.

**Test:**
- Backfill `~/.inspect/audit/` with synthetic entries dated >100d in the past; `inspect audit gc --keep 90d --dry-run` lists them; `inspect audit gc --keep 90d` deletes them and the orphaned snapshot dirs.
- `audit.retention = "90d"` in config: an `--apply` invocation triggers GC; second invocation within the same minute does not re-scan (cheap-path guard).
- `--json` output includes `deleted_entries`, `deleted_snapshots`, `freed_bytes`.
- Snapshot referenced by a retained audit entry is **never** deleted.

---

### L7 — Header / PEM / URL credential redaction in stdout
**Source:** v0.1.2 bundle implementation retrospective.
**Severity:** Medium (security — agent workflows pipe stdout into LLM context windows; one leaked Bearer token can be catastrophic).
**Problem:** The current secret masker (`src/redact.rs`) is line-oriented and only catches `KEY=value` env-var-style strings. It misses:
1. HTTP `Authorization: Bearer <token>` headers in `curl -v` output.
2. PEM private key blocks in file-content dumps (`cat /etc/ssl/private/...`).
3. Credentials embedded in URLs (`postgres://user:pass@host/db`, `redis://:pass@host:6379`).
4. Cookies / `Set-Cookie` headers carrying session tokens.
**Fix:** Add three additional masking patterns running in sequence alongside the existing env-var masker. Each is a separate module under `src/redact/` so future patterns plug in cleanly:

1. **Header masker** (`src/redact/headers.rs`):
   - Match (case-insensitive) `Authorization:`, `X-API-Key:`, `X-Auth-Token:`, `Cookie:`, `Set-Cookie:`, `Proxy-Authorization:`. Mask the value portion: `Authorization: Bearer ****`.
2. **PEM masker** (`src/redact/pem.rs`):
   - Match `-----BEGIN (RSA |EC |DSA |OPENSSH |ENCRYPTED )?PRIVATE KEY-----` through the matching `-----END ... PRIVATE KEY-----`. Replace the entire block (including delimiters) with `[REDACTED PEM PRIVATE KEY (N lines)]`.
3. **URL credential masker** (`src/redact/url_creds.rs`):
   - Match `<scheme>://<user>:<pass>@<host>` in connection strings (postgres, mysql, redis, mongodb, amqp, http, https, etc.). Mask the password portion only: `postgres://user:****@host/db`.

- All maskers run on every `inspect run`, `inspect exec`, `inspect logs`, `inspect search`, `inspect bundle run` stdout/stderr line.
- `--show-secrets` bypasses **all** maskers (extends existing behavior — single flag, single bypass).
- Maskers are streaming-safe (no buffering of full output); PEM masker uses a small line-window state machine.
- Per-masker counter exposed in `--json`: `redactions: { env: 2, header: 1, pem: 0, url: 1 }` so consumers can audit what was masked.

**Test:**
- `inspect run <ns>/<svc> -- 'curl -v https://api.example.com/'` masks the `Authorization:` header.
- `inspect run <ns>/<svc> -- 'cat /etc/ssl/private/test.pem'` replaces the PEM block.
- `inspect run <ns>/<svc> -- 'echo postgres://u:p@h/db'` masks the password.
- `--show-secrets` returns the unmasked output verbatim across all three patterns.
- `--json` output's `redactions` counters match the patterns triggered.
- Streaming test: a slow `curl -v` heartbeat does not break the header mask across chunk boundaries.

---

### L3 — Parameterized aliases
**Source:** Roadmap "Remaining work" / known limitation; agent + recipe ergonomics.
**Severity:** Medium (parser change + alias storage format change).
**Problem:** Aliases today are static strings. A recipe that wants "logs for *any* service on arte" has to either define one alias per service or fall back to writing the full LogQL each time. Agents can't compose aliases programmatically.
**Fix:** Add `$param` placeholders in alias bodies, supplied at call site as `@name(key=val,key2=val2)`. Aliases may chain other aliases.

```bash
# Define
inspect alias add svc-logs '{server="arte", service="$svc", source="logs"}'

# Use
inspect search '@svc-logs(svc=pulse) |= "error"'

# Chain
inspect alias add prod-pulse '@svc-logs(svc=pulse) |= "$pat"'
inspect search '@prod-pulse(pat=ERROR)'

# Agent discovery
inspect alias show svc-logs --json
# → {"name":"svc-logs","parameters":["svc"],"type":"logql","body":"..."}
```

Design points:
- `$param` syntax in alias body. Tokenizer recognizes `$<ident>` outside of string-literal quoting.
- Call site syntax: `@name(k=v,k=v)`. Bare `@name` still works for parameterless aliases (back-compat with v0.1.2 within the v0.1.x series).
- Missing required param → clear error listing the alias name and required params: `error: alias 'svc-logs' requires param 'svc' (call as @svc-logs(svc=...))`.
- Chain depth cap: 5. Beyond that → error at expansion time with the chain shown.
- Circular reference detection at definition time (`alias add`) — refuse to write the alias and explain the cycle.
- `inspect alias show <name> --json` includes a `parameters: []` list for agent discovery.
- **No defaults in v0.1.3.** `${svc:-pulse}` syntax is deferred to v0.2.0 so the parser stays minimal.
- Storage: `aliases.toml` schema gains an optional `parameters = []` field for cached param list (rebuilt on `alias add`); old aliases without the field continue to work.

**Test:**
- `alias add svc-logs '{service="$svc"}'` then `inspect search '@svc-logs(svc=pulse)'` resolves to `{service="pulse"}`.
- Missing param → exit 2 with the required-param error format.
- Chained aliases work to depth 5; depth 6 errors with the chain printed.
- `alias add a '@b'; alias add b '@a'` — second `alias add` errors with "circular reference: a → b → a".
- `inspect alias show <name> --json` includes the resolved `parameters` array.
- Bare `@name` (no parens) on a parameterless alias is unchanged from v0.1.2.

---

### L6 — Per-branch rollback tracking in bundle matrix steps
**Source:** v0.1.2 bundle implementation retrospective.
**Severity:** Medium (architectural — affects correctness of parallel rollback in the bundle engine that just shipped).
**Problem:** Bundle matrix steps (`parallel: true` with `matrix:`) currently execute as all-or-nothing per step. If 4 of 6 volume tars succeed and 2 fail, rollback today undoes the entire step — including the 4 successful branches whose output may already have been used downstream. The correct behavior: rollback only the branches that actually succeeded; leave the failed ones alone (nothing to undo).
**Fix:**
- Track per-branch completion status in the bundle executor (`src/bundle/exec.rs`):
  ```rust
  struct BranchResult {
      branch_id: String,        // e.g. "atlas_milvus"
      status: BranchStatus,     // Ok | Failed | Skipped
      audit_id: Option<String>, // per-branch audit entry id
  }
  ```
- On rollback, only execute the rollback block for branches with `status == Ok`. Failed branches are skipped silently (with an audit note explaining why).
- Audit log: each matrix branch gets its own audit entry under the same `bundle_id` and step id, with a `branch:` field for filtering.
- Rollback templating becomes branch-aware: `{{ matrix.<key> }}` inside a `rollback:` block resolves to **only the succeeded branches' values** when the rollback is triggered. (Inside the forward `exec:`, behavior is unchanged — full matrix.)
- New verb `inspect bundle status <bundle_id>` shows per-branch outcomes:
  ```
  bundle: atlas-phase-0-snapshot (id: 0e3a…)
    step: tar-volumes
      ✓ atlas_milvus    (12.3s)
      ✓ atlas_etcd      (4.1s)
      ✗ aware_milvus    (failed: no space left on device)
  ```
- `--json` variant emits the structured per-branch outcomes for agent consumption.
- Integration with B9's existing `on_failure: rollback_to:` — when rolling back **to** a checkpoint that includes a matrix step, all completed branches of that step are kept (rollback target is "everything after this checkpoint"); when rolling back **a** matrix step itself, only succeeded branches are reversed.

**Test:**
- Inject a mid-matrix failure (3 of 5 branches succeed, 2 fail) — rollback runs only for the 3 succeeded branches; audit log shows per-branch entries with correct `status`.
- `inspect bundle status <id>` displays the per-branch outcomes; `--json` matches.
- `rollback:` block referencing `{{ matrix.volume }}` is invoked exactly once per succeeded branch with the correct value substituted.
- A matrix step where all branches succeed and a *later* step fails (triggering rollback to a checkpoint before the matrix): rollback covers all branches as expected (regression guard for the existing path).
- `--json` audit query `inspect audit show --bundle <id>` returns the per-branch entries grouped by step.

---

### L1 — TUI mode (`inspect tui`)
**Source:** Roadmap "Remaining work" / known limitation; long-standing field request for an interactive dashboard.
**Severity:** Low (large feature, but read-only and additive — does not affect existing surface).
**Problem:** Interactive triage today is a sequence of `inspect status`, `inspect logs --follow`, `inspect why` invocations in separate terminals. Operators want a single live dashboard for at-a-glance health and drill-down. Agents do not need this; humans do.
**Fix:** Ship `inspect tui` — a read-only three-pane dashboard built on `ratatui`. Thin presentation layer over the existing verb functions (no new business logic). Read-only in v0.1.3; write actions inside the TUI are deferred to v0.2.0+.

```
┌─ Services ─────────────────┬─ Logs ──────────────────────────────────┐
│ ▶ pulse        ✓ healthy   │ [pulse]   Request received...           │
│   atlas        ✓ healthy   │ [atlas]   Query OK (42ms)               │
│   synapse      ✗ down      │ [synapse] Connection refused            │
├─────────────────────────────┼─ Detail ────────────────────────────────┤
│                             │ synapse — exited (code 137)             │
│                             │ depends_on: [pulse, atlas, redis]       │
│                             │ Suggested: inspect why arte/synapse     │
└─────────────────────────────┴─────────────────────────────────────────┘
 [q]uit [/]search [f]ollow [m]atch [enter]drill [w]hy [e]xec [?]help
```

Design points:
- **Crate:** `ratatui` + `crossterm`.
- **Left pane:** `inspect status` data, refreshed every 10s (configurable via `--refresh <dur>`).
- **Right-top pane:** `inspect logs --follow --merged` for the selected service(s); supports `/` search and `m` match-filter live.
- **Right-bottom pane:** service detail card (image, ports, depends_on, last health) + suggested `inspect why` invocation; pressing `w` runs `why` and replaces the pane content.
- **Keybindings:** `j`/`k` navigate, `Enter` drill, `/` search, `f` toggle follow, `m` set match filter, `w` run why, `r` force refresh, `?` help overlay, `q` quit. **No mouse.** **No custom layouts.** Fixed three-pane layout.
- **Selector targeting:** `inspect tui [<namespace>|<selector>]` — defaults to all configured namespaces; narrows to a namespace or selector when given.
- **Read-only:** any keypress that would cause a write action (`e`, `x`, etc. in v0.2.0+) shows `read-only in v0.1.3` in the status bar and exits the keypress.
- **Crash safety:** restore terminal state on panic via `crossterm::terminal::disable_raw_mode` in a panic hook; never leave the user's terminal in a broken state.
- **CI:** smoke test runs `inspect tui --selftest` which initializes the layout, ticks the refresh once, dumps the rendered frame to stdout as text, and exits 0.
- **Scope discipline:** no themes, no plugins, no custom layouts, no per-cell styling configuration in v0.1.3. The TUI is a window onto existing commands, nothing more.

**Test:**
- `inspect tui --selftest` exits 0 within 2s and prints a layout snapshot to stdout.
- Snapshot test: rendered frame for a fixed mock dataset matches a golden file in `tests/golden/tui/`.
- Panic hook test: forcibly panic inside the render loop, verify terminal mode is restored (`stty -a` snapshot returns to cooked mode).
- Keybinding test: each documented key produces the documented effect against a mock data backend (no real SSH).
- `inspect tui` against an unreachable namespace shows the namespace as `unreachable` in the left pane and surfaces the underlying connectivity error in the detail pane (no crash).

---

## Out of scope for v0.1.3
*(explicitly deferred — do not let scope creep drag these in)*

- **TUI write actions.** Read-only in v0.1.3; `e` / `x` / `apply` from inside TUI is v0.2.0+.
- **Alias defaults / fallback values** (`${svc:-pulse}`). Parser stays minimal in v0.1.3; defaults land in v0.2.0.
- **Cross-medium bundles** (Docker + k8s steps in one bundle). v0.2.0 introduces k8s; cross-medium is post-v0.2.0.
- **CLI surface renames / config schema freeze.** That is the entire job of v0.1.4. Resist any rename in v0.1.3 unless it is fixing an outright bug.
- **kubectl / k8s anything.** v0.2.0.
- **Themes, plugins, custom layouts in TUI.** Permanent no for v0.1.x.

---

## Shipped
*(move items here when released)*

---

## Running total: 0 / 7 — **OPEN**

**Next step after L1 lands:** open `INSPECT_v0.1.4_BACKLOG.md` covering the S1–S7 stabilization sweep (CLI surface audit, config freeze, JSON schema freeze, help audit, README rewrite, dead code + dependency audit, security audit). v0.1.4 is the last release before the v0.2.0 contract; it ships **no new features**.
