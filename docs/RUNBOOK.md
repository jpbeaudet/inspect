# GA Runbook — `inspect` v0.1.0

Operator-facing playbook for incident handling, hotfix patching, and
release rollout. Companion to `CHANGELOG.md` and the v2 catalog in
`archives/INSPECT_BIBLEv6.2.md` §27.

---

## 1. Release rollout

### 1.1 Cut a tag

```sh
# from main, with a clean working tree
cargo test --locked
cargo build --release --locked

# bump version in Cargo.toml + CHANGELOG.md, commit
git tag -s v0.1.0 -m "v0.1.0"
git push origin v0.1.0
```

The `release` workflow runs automatically on tag push:

1. Builds static-musl Linux (`x86_64`, `aarch64`) and Apple Darwin
   (`x86_64`, `aarch64`) tarballs.
2. Generates per-artifact `sha256` plus aggregate `SHA256SUMS`.
3. Signs each tarball with cosign keyless (GitHub OIDC).
4. Publishes a GitHub Release with all artifacts attached.
5. Optionally publishes to crates.io if repo variable
   `PUBLISH_CRATE = "true"` and secret `CARGO_REGISTRY_TOKEN` are set.

### 1.2 Verify a release

```sh
# Checksum
shasum -a 256 -c inspect-0.1.0-x86_64-unknown-linux-musl.tar.gz.sha256

# Cosign keyless
cosign verify-blob \
  --certificate-identity-regexp 'https://github.com/jpbeaudet/inspect/.*' \
  --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' \
  --certificate inspect-0.1.0-x86_64-unknown-linux-musl.tar.gz.pem \
  --signature   inspect-0.1.0-x86_64-unknown-linux-musl.tar.gz.sig \
  inspect-0.1.0-x86_64-unknown-linux-musl.tar.gz
```

### 1.3 Post-release smoke

On a staging fleet:

```sh
inspect --version
inspect setup arte
inspect ps arte
inspect status arte
inspect search '{server="arte", source="logs"} |= "error"' --tail 50
```

---

## 2. Hotfix patch flow

For a security or correctness fix shipped between minor releases:

1. Branch from the affected tag: `git checkout -b hotfix/0.1.1 v0.1.0`.
2. Land the smallest possible patch + a regression test.
3. Bump patch version in `Cargo.toml` and append a `## [0.1.1]` entry
   to `CHANGELOG.md`.
4. Tag `v0.1.1`, push, let the release workflow run.
5. Update the Homebrew formula sha256s (if a tap is configured).
6. Operators upgrade with `scripts/install.sh --version v0.1.1` (which
   refuses to clobber a newer installed version unless `--force`).

---

## 3. Incident response

### 3.1 Triage matrix

| Symptom | First check | Likely cause |
|---|---|---|
| `inspect` exits 2 with `ssh: connection refused` | `~/.ssh/config` has the namespace host | namespace not configured locally |
| Empty `ps` output, no error | `inspect setup <ns> --force` | stale or missing profile |
| `cargo build` failure on a fresh clone | rust-toolchain pin (1.75 minimum) | MSRV drift |
| Hung command, no output | SIGINT once; check `inspect why` | SSH ControlMaster stall |
| Slow first results across 5+ servers | `INSPECT_MAX_PARALLEL=8 inspect …` | concurrency cap |
| Secrets visible in JSON output | file a P0 — redaction is contract | redactor bug |

### 3.2 P0 — secret leakage

1. Stop publishing further releases. Lock the repo from new tags.
2. Reproduce on the offending version with the smallest input that leaks.
3. Land a regression test (assert redaction in the JSON envelope).
4. Cut a hotfix per §2.
5. Yank the broken release from crates.io if it was published:
   `cargo yank --version <X.Y.Z> inspect-cli`.
6. Mark the GitHub Release as a pre-release and prepend a
   "DO NOT USE — see CVE-… " banner in the release notes.

### 3.3 P0 — corrupted profile cache on apply

`inspect` writes profiles atomically (tempfile + rename, mode 0600). If
a user reports a corrupted cache:

1. Have them move it aside: `mv ~/.inspect/profiles/<ns>.yaml{,.bad}`.
2. Re-run `inspect setup <ns> --force`.
3. Capture the `.bad` file (with secrets redacted) for the bug report.

### 3.4 Failed mutating apply

Every mutating verb writes a snapshot under `~/.inspect/audit/<id>/`
before changing remote state. To roll back:

```sh
inspect revert <audit-id>
```

If `revert` reports drift, the remote has been changed since the
snapshot. Force-revert is intentional and noisy:

```sh
inspect revert <audit-id> --force
```

---

## 4. Compatibility statement

- **JSON envelope** (`schema_version`): semver-tracked. Field additions
  are non-breaking. Removals or renames bump the major.
- **Exit codes**: `0` success, `1` no-match (search-shaped verbs only),
  `2` error. Stable across patch releases.
- **CLI flag surface**: stable across patches. Deprecations emit a
  warning for at least one minor before removal.
- **Profile cache schema**: versioned. Older clients may refuse newer
  profiles; `inspect setup --force` always recovers.

---

## 5. Support matrix (v0.1.0)

| OS | arch | tier | notes |
|---|---|---|---|
| Linux (musl) | x86_64 | tier 1 | static binary in release artifacts |
| Linux (musl) | aarch64 | tier 1 | static binary in release artifacts |
| macOS | x86_64 | tier 1 | release artifact (Intel) |
| macOS | aarch64 | tier 1 | release artifact (Apple Silicon) |
| Windows | any | unsupported | `inspect` shells out to `ssh` and `docker` |

Remote (target) requirements: `ssh` reachable, plus `docker` or
`systemctl` for service-shaped verbs. Host-only verbs (`_/host:…`)
require POSIX coreutils on the remote.

---

## 6. Known limitations (v0.1.0)

Tracked in `archives/INSPECT_BIBLEv6.2.md` §27 as v2 features:

- No TUI mode.
- No Kubernetes discovery (Phase 12 ships docker + systemd only).
- No distributed tracing integration.
- No OS keychain integration for SSH passphrases.
- No per-user policy enforcement (single global safety gate).
- No russh-based fallback when system `ssh` is unavailable.
- No parameterized aliases (`@logs(svc=$x)` is reserved syntax).
- No password authentication (key-based only).
- No remote agent — `inspect` is strictly local-first.

---

## 7. Quick reference

```sh
# Discovery
inspect setup <ns>           # one-time profile capture
inspect setup <ns> --force   # refresh on drift

# Read verbs
inspect ps <selector>
inspect status <selector>
inspect health <selector>
inspect logs <selector>      [--tail N] [--since 1h] [--follow]
inspect grep <pattern> <selector>

# Search engine
inspect search '<logql>'     # streaming + metric queries

# Write verbs (dry-run by default)
inspect cp <src> <dst> <selector>     [--apply]
inspect edit <path> <selector>        [--apply]
inspect restart <selector>            [--apply]

# Recovery
inspect revert <audit-id>             [--force]

# Help
inspect help                          # topic catalog
inspect help <topic>                  # full topic body
inspect help search <query>           # keyword search
```

---

## 8. Probe author checklist (v0.1.3)

### 8.1 Docker inspect timeout classification — F2 three-bucket rule

**The rule:** every batched remote probe that can time out classifies its
outcome into one of *exactly three* buckets before deciding what to surface
to the operator. The default channel is silence; the warning channel is
reserved for actionable, non-fatal problems; the error channel is reserved
for "the daemon is unreachable and discovery cannot be trusted".

| Bucket                | What it means                                                                                          | Where it goes                                                                |
| --------------------- | ------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------- |
| **Clean**             | Every container inspected on the first try. No noise.                                                  | Silence. (Healthy hosts must never emit a `warning:` line on first setup.)   |
| **SlowButSuccessful** | The batched call exceeded its budget but the per-container fallback rescued every container.           | `eprintln!("debug: …")` only when `INSPECT_DEBUG=1` or `RUST_LOG=…debug…`.   |
| **PartialTimeout**    | After fallback, `N` of `M` containers still failed.                                                    | One summary line: `warning: docker inspect timed out for N/M containers; rerun with --force or check daemon load`. |
| **GenuineFailure**    | Zero containers inspected — the daemon is down or the socket is gone.                                  | Probe-level fatal (`ProbeResult.fatal = Some(...)`); engine returns `Err`, setup exits non-zero with a chained hint pointing at `inspect run … 'sudo systemctl status docker'` then `inspect setup --force`. |

The classifier is implemented as a pure function
[`classify_inspect_outcome(total, succeeded, batch_was_slow, last_error)`](../src/discovery/probes.rs)
so every probe author follows the same rule. Adding a new probe with
similar timeout characteristics? Reuse this function. Do not invent a
fourth bucket.

### 8.2 Inventory-scaled timeout formula

The batched `docker inspect` budget is **not** a fixed 10 seconds (the
v0.1.0–v0.1.2 default that produced the spurious-warning regression on
30+-container hosts). Instead:

```text
timeout = max(10s, 250ms * container_count), capped at 60s
```

| Containers | Budget |
| ---------- | ------ |
| 0–40       | 10s (floor) |
| 80         | 20s    |
| 100        | 25s    |
| 240        | 60s (cap) |
| 1000       | 60s (cap) |

**Operator override.** Set `INSPECT_DOCKER_INSPECT_TIMEOUT=<seconds>` to
bypass the formula entirely (e.g. on a pathologically slow daemon where
even 60s isn't enough, or when you intentionally want a tighter budget
for a flaky discovery run). The override is taken verbatim and is not
re-clipped against the cap.

The formula lives in
[`compute_docker_inspect_timeout(count, override_secs)`](../src/discovery/probes.rs);
unit tests pin every boundary (floor, exact-floor crossover at 40
containers, scaling region, cap, override).

---

## 9. Per-namespace remote env overlay (v0.1.3, F12)

**The rule:** the per-namespace env overlay is rendered and prepended
at the verb boundary (`run`, `exec`), not inside the SSH executor.
Any new write verb that calls `dispatch::plan` and inherits an
`NsCtx` already has the overlay populated; to honor it, the verb
must:

1. Optionally accept `--env KEY=VAL` / `--env-clear` / `--debug` and
   parse user-side env via `exec::env_overlay::parse_kv` *before* the
   per-step loop. Validation failure must abort the whole batch.
2. Per step, compute the effective overlay with
   `exec::env_overlay::merge(&ns.env_overlay, &user_env, env_clear)`
   and call `exec::env_overlay::apply_to_cmd(&cmd, &effective)` to
   produce the rendered command before quoting/transport.
3. Record `env_overlay` (the effective map) and `rendered_cmd` on the
   `AuditEntry` so revert and forensics see exactly what shipped.
4. When `--debug` is set, print the rendered command to stderr
   *before* sending it. The format is stable
   (`debug: rendered: <cmd>`) and is the supported way for operators
   to verify what they're about to ship.

**Quoting.** Values are double-quoted. `$VAR` still expands on the
remote (so `PATH="$HOME/.cargo/bin:$PATH"` works as written), but
`;`/`&`/`|` stay literal — the overlay can't smuggle a second
command past the safety contract. `"`, `\`, and backtick inside
values are escaped.

**Validation boundary.** POSIX env-key validation
(`[A-Za-z_][A-Za-z0-9_]*`) lives in `NamespaceConfig::validate` and
is invoked from `verbs/runtime::resolve_target`, so every dispatch
path catches an invalid key — not just the writers that read the
overlay. New verbs do not need to re-validate.

---


---

## 10. Stale-session auto-reauth + transport exit class (v0.1.3, F13)

The dispatch boundary that ships operator-supplied commands to a
remote (`inspect run`, `inspect exec`) splits failures into four
buckets, each with its own exit code and operator playbook. Wrappers
and CI scripts can branch on `$?` reliably:

| Bucket | Exit | OpenSSH stderr it fires on | Operator action |
|---|---:|---|---|
| `transport_stale` | `12` | `Connection closed by`, `Control socket … connect: No such file or directory`, `master process … exited`, `mux_client_request_session: session request failed`, `ControlPath unusable` | re-establish the master socket; default behaviour is to reauth + retry once |
| `transport_unreachable` | `13` | `Could not resolve hostname`, `Connection refused`, `Connection timed out`, `No route to host`, `Network is unreachable`, `Host key verification failed` | network / DNS / firewall problem; `inspect connectivity <ns>` to diagnose |
| `transport_auth_failed` | `14` | `Permission denied (publickey)`, `Too many authentication failures`, `error in libcrypto`, or auto-reauth that itself fails | wrong/expired key, wrong passphrase env, or revoked authorized_keys; `inspect connect <ns>` interactively |
| `command_failed` | remote `1..125` | non-zero exit from the operator's command, classifier returned `None` | the remote command reported failure; the contract is identical to a direct ssh |

`ExitKind::Inner` is clamped to `1..=125`, so the three transport
codes never collide with a remote command's exit. A wrapper script
can do:

```sh
inspect run arte/api -- ./migrate.sh
case $? in
  0)            echo "ok" ;;
  12)           echo "stale session; reauth + retry" ;;
  13)           echo "host unreachable; check network" ;;
  14)           echo "auth failed; rotate key" ;;
  1|2|125|126|127) echo "remote command failed with code $?" ;;
  *)            echo "other error" ;;
esac
```

### 10.1 Auto-reauth contract

Default behaviour on `transport_stale`:

1. Stderr gets a single
   `note: persistent session for <ns> expired — re-authenticating…`
   line.
2. An audit entry with `verb=connect.reauth`,
   `args=trigger=transport_stale,original_verb=run,selector=<sel>`
   is written to `~/.inspect/audit/<YYYY-MM>-<user>.jsonl`.
3. The persistent master socket is torn down via
   `ssh::exit_master(socket_path, target)` and re-established with
   the same `AuthSelection { passphrase_env, allow_interactive,
   skip_existing_mux_check: false }` interactive `inspect connect`
   would use. Askpass / agent / `*_PASSPHRASE_ENV` env-var paths
   are unchanged.
4. The original step is re-dispatched exactly once. Whatever it
   returns (success, command_failed, transport_*) is final.
5. The retry's audit entry stamps `retry_of=transport_stale@<label>`
   and `reauth_id=<id of the connect.reauth entry>` so a downstream
   consumer can correlate the pair with a single `jq` filter.

A failed reauth (e.g. agent unlocked, but the network dropped
between the unlock and the retry) escalates to
`transport_auth_failed` (exit 14) so the operator sees the
auth-shaped problem rather than the transport-shaped one. There is
no exponential backoff and no second retry — the contract is one
attempt to recover, then surface the truth.

### 10.2 Operator opt-outs

| Knob | Scope | Effect |
|---|---|---|
| `--no-reauth` on `run` / `exec` | per-invocation | classify + exit 12, do not retry |
| `[namespaces.<ns>] auto_reauth = false` in `~/.inspect/servers.toml` | per-namespace, persistent | classify + exit 12, do not retry |

Both knobs ANd into the runtime check
`policy.allow_reauth = !args.no_reauth && s.ns.auto_reauth`. CI
runners that prefer a hard stale-failure surface over a transparent
re-auth typically set the namespace flag for their service-account
namespaces and leave operator namespaces with the default.

### 10.3 SUMMARY trailer + JSON contract

When every failed step shares the same transport class, the
human-format summary appends a chained recovery hint:

```
SUMMARY: run: 0 ok, 1 failed (ssh_error: stale connection — run
  'inspect disconnect arte && inspect connect arte' or pass --reauth)
```

The JSON contract gains a final envelope per verb invocation:

```json
{"_schema_version":1,"_source":"run","_medium":"run",
 "server":"arte/api","phase":"summary","ok":0,"failed":1,
 "failure_class":"transport_stale"}
```

`failure_class` is one of
`ok | command_failed | transport_stale | transport_unreachable | transport_auth_failed | transport_mixed`.
The streaming line envelopes earlier in the run remain unchanged so
`inspect run … --json | jq -c '.line'` continues to work and a new
consumer can opt into the summary by filtering on
`select(.phase=="summary")`.

### 10.4 Disabling for `connect`-only diagnostics

`inspect connect`, `inspect disconnect`, `inspect connectivity`,
`inspect why` and the read verbs (`logs`, `ps`, `status`, …) do
**not** flow through the F13 wrapper — they would either be the
recovery action itself (`connect`) or are read-only diagnostics
where reauth has no useful semantics. Only `run` and `exec` (the
two verbs that ship operator-supplied free-form commands) carry
the transport exit-class contract.

---

## 10. Stale-session auto-reauth + transport exit class (v0.1.3, F13)

The dispatch boundary that ships operator-supplied commands to a
remote (`inspect run`, `inspect exec`) splits failures into four
buckets, each with its own exit code and operator playbook. Wrappers
and CI scripts can branch on `$?` reliably:

| Bucket | Exit | OpenSSH stderr it fires on | Operator action |
|---|---:|---|---|
| `transport_stale` | `12` | `Connection closed by`, `Control socket … connect: No such file or directory`, `master process … exited`, `mux_client_request_session: session request failed`, `ControlPath unusable` | re-establish the master socket; default behaviour is to reauth + retry once |
| `transport_unreachable` | `13` | `Could not resolve hostname`, `Connection refused`, `Connection timed out`, `No route to host`, `Network is unreachable`, `Host key verification failed` | network / DNS / firewall problem; `inspect connectivity <ns>` to diagnose |
| `transport_auth_failed` | `14` | `Permission denied (publickey)`, `Too many authentication failures`, `error in libcrypto`, or auto-reauth that itself fails | wrong/expired key, wrong passphrase env, or revoked authorized_keys; `inspect connect <ns>` interactively |
| `command_failed` | remote `1..125` | non-zero exit from the operator's command, classifier returned `None` | the remote command reported failure; the contract is identical to a direct ssh |

`ExitKind::Inner` is clamped to `1..=125`, so the three transport
codes never collide with a remote command's exit. A wrapper script
can do:

```sh
inspect run arte/api -- ./migrate.sh
case $? in
  0)            echo "ok" ;;
  12)           echo "stale session; reauth + retry" ;;
  13)           echo "host unreachable; check network" ;;
  14)           echo "auth failed; rotate key" ;;
  1|2|125|126|127) echo "remote command failed with code $?" ;;
  *)            echo "other error" ;;
esac
```

### 10.1 Auto-reauth contract

Default behaviour on `transport_stale`:

1. Stderr gets a single
   `note: persistent session for <ns> expired — re-authenticating…`
   line.
2. An audit entry with `verb=connect.reauth`,
   `args=trigger=transport_stale,original_verb=run,selector=<sel>`
   is written to `~/.inspect/audit/<YYYY-MM>-<user>.jsonl`.
3. The persistent master socket is torn down via
   `ssh::exit_master(socket_path, target)` and re-established with
   the same `AuthSelection { passphrase_env, allow_interactive,
   skip_existing_mux_check: false }` interactive `inspect connect`
   would use. Askpass / agent / `*_PASSPHRASE_ENV` env-var paths
   are unchanged.
4. The original step is re-dispatched exactly once. Whatever it
   returns (success, command_failed, transport_*) is final.
5. The retry's audit entry stamps `retry_of=transport_stale@<label>`
   and `reauth_id=<id of the connect.reauth entry>` so a downstream
   consumer can correlate the pair with a single `jq` filter.

A failed reauth (e.g. agent unlocked, but the network dropped
between the unlock and the retry) escalates to
`transport_auth_failed` (exit 14) so the operator sees the
auth-shaped problem rather than the transport-shaped one. There is
no exponential backoff and no second retry — the contract is one
attempt to recover, then surface the truth.

### 10.2 Operator opt-outs

| Knob | Scope | Effect |
|---|---|---|
| `--no-reauth` on `run` / `exec` | per-invocation | classify + exit 12, do not retry |
| `[namespaces.<ns>] auto_reauth = false` in `~/.inspect/servers.toml` | per-namespace, persistent | classify + exit 12, do not retry |

Both knobs ANd into the runtime check
`policy.allow_reauth = !args.no_reauth && s.ns.auto_reauth`. CI
runners that prefer a hard stale-failure surface over a transparent
re-auth typically set the namespace flag for their service-account
namespaces and leave operator namespaces with the default.

### 10.3 SUMMARY trailer + JSON contract

When every failed step shares the same transport class, the
human-format summary appends a chained recovery hint:

```
SUMMARY: run: 0 ok, 1 failed (ssh_error: stale connection — run
  'inspect disconnect arte && inspect connect arte' or pass --reauth)
```

The JSON contract gains a final envelope per verb invocation:

```json
{"_schema_version":1,"_source":"run","_medium":"run",
 "server":"arte/api","phase":"summary","ok":0,"failed":1,
 "failure_class":"transport_stale"}
```

`failure_class` is one of
`ok | command_failed | transport_stale | transport_unreachable | transport_auth_failed | transport_mixed`.
The streaming line envelopes earlier in the run remain unchanged so
`inspect run … --json | jq -c '.line'` continues to work and a new
consumer can opt into the summary by filtering on
`select(.phase=="summary")`.

### 10.4 Disabling for `connect`-only diagnostics

`inspect connect`, `inspect disconnect`, `inspect connectivity`,
`inspect why` and the read verbs (`logs`, `ps`, `status`, …) do
**not** flow through the F13 wrapper — they would either be the
recovery action itself (`connect`) or are read-only diagnostics
where reauth has no useful semantics. Only `run` and `exec` (the
two verbs that ship operator-supplied free-form commands) carry
the transport exit-class contract.

---

*Source: this runbook implements Phase 12 of the original implementation
plan in `archives/IMPLEMENTATION_PLAN.md`. §8 was added in v0.1.3 (F2)
to lock in the three-bucket discipline; §9 in F12 (env overlay); §10
in F13 (auto-reauth + transport exit class). Any deviation between
this runbook and the bible is a runbook bug.*
