Starting: *Audit Phase 11 fleet orchestrator* (1/6)

Read [](file:///workspaces/inspect/src/commands/fleet.rs#1-1), lines 1 to 200

Read [](file:///workspaces/inspect/src/commands/fleet.rs#200-200), lines 200 to 340

Read [](file:///workspaces/inspect/src/commands/fleet.rs#340-340), lines 340 to 450

Ran terminal command:  cd /workspaces/inspect && grep -n "pub fn\|write\|append\|OpenOptions\|create\|flock\|fs::open\|write_atomic\|rename" src/safety/audit.rs | head -40

Read [](file:///workspaces/inspect/src/safety/audit.rs#90-90), lines 90 to 115

Ran terminal command:  cd /workspaces/inspect && grep -rn "INSPECT_FLEET_FORCE_NS\|INSPECT_NON_INTERACTIVE\|INSPECT_FLEET_CONCURRENCY" src/ | head -30

Ran terminal command:  cd /workspaces/inspect && grep -n "askpass\|passphrase\|secret\|redact\|key_passphrase_env" src/ssh/exec.rs src/ssh/master.rs src/ssh/askpass.rs 2>&1 | head -30

Let me check one more concern — whether the child stdout/stderr capture can deadlock, and check the bible for missed exit-criteria.  

Ran terminal command:  cd /workspaces/inspect && cat src/safety/audit.rs | sed -n '1,60p'

Ran terminal command:  cd /workspaces/inspect && cat src/safety/audit.rs | sed -n '60,100p'

Ran terminal command:  cd /workspaces/inspect && grep -n "rand_u32\|rand::" src/safety/audit.rs | head

Read [](file:///workspaces/inspect/src/safety/audit.rs#170-170), lines 170 to 200

I have enough context. Compiling findings. 

Starting: *Compile findings report* (6/6)

# Phase 11 + Codebase Audit

Below is everything I'd flag before Phase 12. Each item has a severity and a concrete location. I have **not** changed any code — this is a read-only review.

## High severity

### H1. Pipe-deadlock risk in fleet child capture
fleet.rs reads stdout to EOF, then stderr to EOF, on the **same thread**:

```rust
if let Some(mut s) = child.stdout.take() { let _ = s.read_to_string(&mut stdout); }
if let Some(mut s) = child.stderr.take() { let _ = s.read_to_string(&mut stderr); }
```

Classic pipe deadlock — if a child writes >64 KB to stderr (Linux default pipe buffer) before exiting, the child blocks on `write(stderr)` while we block on `read(stdout)`. Reproduces with verbose `recipe` or `search --tail` children.
**Fix**: replace with `cmd.output()` or `child.wait_with_output()`, both of which internally drain both pipes concurrently.

### H2. Fleet large-fanout interlock counts namespaces, not targets
Bible §13 explicitly says *"large-fanout interlock triggers on **total target count**"*. We gate on namespace count (fleet.rs). A user running `fleet --ns prod-1,prod-2 restart '*'` over two big namespaces with 50 services each gets through the parent gate (2 ≤ 10) — only the per-child Phase-5 gate fires (and only on a per-namespace basis). The aggregate "100 targets" is never surfaced as a single confirmation.
**Fix**: dry-resolve the inner selector against each chosen namespace's profile (cheap, in-memory) to compute the true total before fanout, and gate on that sum.

### H3. Stdin closure not set on fleet children
`Stdio::null()` is correctly used for child stdin in fleet.rs, but child verbs like `restart`/`rm` may still attempt `confirm()` flows that read from stdin. We set `INSPECT_NON_INTERACTIVE=1` which causes them to abort with a friendly error rather than block — this works today, but it means **mutating fleet runs always require `--yes` / `--yes-all` on the inner verb**. There is no way to confirm a fleet write interactively. This is a behavioral gap, not a bug, but it isn't documented anywhere user-facing.

## Medium severity

### M1. `INSPECT_FLEET_FORCE_NS` is an undocumented public footgun
The selector resolver honors this env var unconditionally (resolve.rs). If a user (or an LLM agent invoking inspect) accidentally exports it, every subsequent `inspect ps`/`status`/`grep` silently overrides their selector to a single namespace with no warning.
**Fix options**: (a) rename to `INSPECT_INTERNAL_FLEET_FORCE_NS` (private contract), or (b) require a paired sentinel like `INSPECT_FLEET_PARENT_PID` matching the parent's pid before honoring it, or (c) emit a stderr warning on every override.

### M2. Subtractive selector atoms are silently bypassed inside fleet
`fleet --ns 'prod-*' status '~prod-1/_'` resolves the user's `~prod-1` exclusion into a `ServerSpec`, but `INSPECT_FLEET_FORCE_NS` discards `sel.server` entirely (resolve.rs). The user thinks they excluded `prod-1`; fleet still ran it. Should at minimum reject server atoms in the inner selector when force-ns is active, or honor the original spec for filtering.

### M3. Fleet swallows children's `ExitKind::NoMatches` (exit 1) into a pure-1 collapse
fleet.rs only returns `NoMatches` when **every** child returned exit 1. A mixed run of (ok, ok, no-match) returns `Error` (2). That's defensible, but it means `inspect fleet grep "needle"` over 5 namespaces where 4 found nothing and 1 found a match returns 2, not 0. Bible §10 doesn't fully specify this; worth a deliberate decision.

### M4. SSH master prerequisites for fleet are undocumented
Each fleet child reuses the per-namespace SSH master if one exists (Phase 1 cache), otherwise tries to start one. With `INSPECT_NON_INTERACTIVE=1` (which fleet always sets), any namespace with `key_passphrase_env` unset and no agent forwarding fails outright. **No `inspect fleet connect` exists** to pre-warm masters. Operators have to script `for ns in ...; inspect connect $ns; done` manually.
**Fix**: add `setup`/`connect` to a recommended pre-flight, or accept `connect` under fleet (currently it's marked positional-only and works, but not advertised).

### M5. Rate-limit / backpressure on remote masters
With `INSPECT_FLEET_CONCURRENCY=64` and 64 namespaces all spawning a fresh ssh master simultaneously, we hammer the local OpenSSH binary and remote sshd hard. Bible §14 mentions ControlMaster reuse but no per-host connect rate limit. Phase-1 master start has no exponential backoff. Not a Phase-11 regression but exposed by Phase 11.

### M6. Audit IDs collide under fleet bursts
audit.rs uses `nanos.wrapping_mul(...).wrapping_add(pid)` masked to 16 bits. With fleet fanout, distinct PIDs save us, but **within a single child** two appends in the same millisecond collide with probability ~1.5% per pair (16-bit space, birthday bound at √65k ≈ 256). Recipes that perform multiple writes per ns can trigger this. Switch to a counter or pull in `getrandom` (pure-Rust, ~20 LOC).

## Low severity

### L1. `expand_ns_pattern` has a precedence quirk
A `--ns` of `@group,extra` is treated as a single comma-list piece `@group` (literal, will not match), not as "group ∪ extra". Documentation implies `@group` is exclusive. Either reject mixed forms or merge them. fleet.rs.

### L2. Fleet human output mangles JSON child output
When a child runs with `--json`, fleet's human renderer prefixes every line with 4 spaces (fleet.rs) — that breaks `inspect fleet status --ns prod-* --json | jq` (works for our top-level JSON path, but `inspect fleet ... ps --json` indents the inner JSON and the user can't pipe the whole thing). The supported path is "use `inspect fleet --json`, never pass `--json` to the inner verb". Should be documented.

### L3. Worker pool spawns a thread even when work is empty
`(0..concurrency.min(total).max(1))` ensures ≥1 worker even when `total == 0`. In practice we error before fanout, but the safety net is wasted. Cosmetic.

### L4. `INSPECT_FLEET_CONCURRENCY` not size-clamped at parse time
We clamp to `MAX_FLEET_CONCURRENCY` (64) only after the env-var parse. A user can set it to `usize::MAX` and `n.min(64)` saves us — fine, but the validator only rejects 0. Bible says "default 8"; we silently clamp 9999 → 64 with no warning. Add a stderr note when clamped.

### L5. `groups.toml` parser doesn't validate group/member names
A group entry like `members = ["../../etc/passwd*"]` is accepted; the glob then matches nothing (since known namespaces don't include path traversal), so it's effectively safe, but worth rejecting at parse time to catch typos. groups.rs.

### L6. `disconnect-all` listed in `DISALLOWED_INNER_VERBS` but `clap` actually emits `disconnect-all` (kebab) — that's correct, but the matching string is one of seven hardcoded entries; if Phase 12 renames any verb the list silently drifts. Move the disallow-list check to a single source-of-truth match against `cli::Command` shape (or cover with an explicit clap-parse round-trip test).

### L7. `safety/gate.rs::SafetyGate::confirm` is constructed by hand in fleet.rs
Phase 5 always uses `SafetyGate::new(...)`. We bypass it in fleet.rs and set fields directly. If `SafetyGate::new` ever gains side-effects (it already reads env), fleet diverges silently. Use the constructor.

## Security-specific findings

### S1. Process env is not sanitized when spawning children
We only set/remove `INSPECT_NON_INTERACTIVE` and `INSPECT_FLEET_FORCE_NS`. The child inherits the **entire** parent env, including any passphrase env vars (`ARTE_SSH_PASSPHRASE=…`), `ENV_INTERACTIVE_PASSPHRASE` set by Phase 1's interactive prompt, and any third-party secret env vars. For fleet across namespaces with different credentials, namespace `prod-2`'s child sees `ARTE_SSH_PASSPHRASE` even though it doesn't need it. Not a leak per se (env is process-private on Linux), but defense-in-depth would be to scope passphrase env vars per child:
- collect the set of `key_passphrase_env` names referenced in `servers.toml` for **other** namespaces
- `cmd.env_remove(...)` each one before spawning

### S2. `INSPECT_FLEET_CONCURRENCY` env-driven thread pool DoS
If an external process can write `~/.bashrc` or environment, it could set `INSPECT_FLEET_CONCURRENCY=10000`. We clamp to 64 — good. But `MAX_FLEET_CONCURRENCY` is private to 