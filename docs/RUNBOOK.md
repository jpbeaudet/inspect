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

## 11. Script mode for `inspect run` (v0.1.3, F14)

`inspect run --file <path>` and `inspect run --stdin-script` ship
the entire script body to the remote via the same byte-for-byte
stdin pipe F9 forwards on, dispatching `bash -s -- <args>` (or
`<interp> -` for non-bash interpreters declared via shebang).

### 11.1 Dispatch shape

| Local invocation | Rendered remote command |
|---|---|
| `inspect run arte/_ --file s.sh` (host target, bash shebang or none) | `bash -s` |
| `inspect run arte/_ --file s.sh -- a b` | `bash -s -- 'a' 'b'` |
| `inspect run arte/atlas --file s.sh` (container target) | `docker exec -i 'atlas' bash -s` |
| `inspect run arte/_ --file py.py` (`#!/usr/bin/env python3`) | `python3 -` |
| `inspect run arte/_ --stdin-script` | identical to `--file`, body from stdin |

Args after `--` are POSIX-shell-quoted before crossing the SSH
boundary. The script body itself is never re-quoted — it crosses
intact in the stdin pipe.

### 11.2 Audit fields

Every script-mode invocation writes a per-step audit entry. New
optional fields (omitted on non-script-mode entries):

- `script_path` — absolute local path (`null` for `--stdin-script`)
- `script_sha256` — hex SHA-256 of the body
- `script_bytes` — body length
- `script_interp` — selected interpreter (`bash` / `sh` / `python3` / ...)
- `script_body` — full body, present only under `--audit-script-body`

The body itself is dedup-stored at
`~/.inspect/scripts/<script_sha256>.sh` (mode 0600, inside the 0700
home) so audit reconstruction works even after the operator deletes
the local file. The store is content-addressed; identical scripts
across many invocations share one on-disk file.

### 11.3 Mutual-exclusion contract

| Combination | Outcome |
|---|---|
| `--file` + `--stdin-script` | clap rejects (exit 2) |
| `--file` + `--no-stdin` | clap rejects (exit 2) |
| `--stdin-script` + `--no-stdin` | clap rejects (exit 2) |
| `--stdin-script` with tty / empty stdin | runtime exits 2 with `--file`-pointing hint |
| `--file <missing>` | runtime exits 2 with the path in the error |
| `--file <directory>` | runtime exits 2 (rejects directories) |
| `--file <path>` above `--stdin-max` cap | runtime exits 2 with the `inspect cp` chained hint |

### 11.4 Composes with the rest of v0.1.3

Script mode dispatches through the same SSH executor as bare
`inspect run`, so:

- F12 namespace env overlay applies (the script sees the configured
  `PATH`, `LANG`, `KUBECONFIG`, ...).
- F13 stale-session auto-reauth fires identically; a script-mode
  step that hits `transport_stale` is retried after reauth, and
  the audit entry stamps `retry_of` / `reauth_id` /
  `failure_class` exactly as for argv-cmd-mode runs.
- F9 size cap (`--stdin-max`) protects against pathological script
  sizes (default 10 MiB; raise with `--stdin-max 100m`, set to
  `0` to disable, or use `inspect put` (F15) for bulk file
  transfer (uncapped, audit-tracked, F11-revertible)).
- F10.7 `--clean-output` and the F7.4 `--quiet` summary
  suppression compose unchanged.

`inspect run --file` itself remains read-by-default (per the F11
run/exec split). `inspect exec` is **not** gaining `--file` in
v0.1.3 — that's a v0.1.5 follow-up that will require the
`# inspect-revert: <inverse-script-path>` directive contract for
revertability.

## 12. File transfer internals (v0.1.3, F15)

`inspect put` / `inspect get` / `inspect cp` are implemented in
`src/verbs/transfer.rs` and dispatch through the same SSH executor
as every other namespace verb. Three contracts are load-bearing:

**ControlPath reuse.** Transfers spawn no separate `scp` process.
Push uses `RunOpts::with_stdin(<bytes>)` to stream the local body
into a remote `sh -c 'set -e; cat > <tmp>; ... ; mv <tmp> <path>'`
pipeline; pull uses `base64 -- <path>` over the same pipe and
decodes locally. Both paths reuse the namespace's existing
`~/.inspect/sockets/<ns>.sock` master, so they inherit auth,
F12 env overlay, and F13 auto-reauth behaviour automatically.

**Atomic rename + permission preservation.** The atomic-write
shell snippet (`transfer::build_stream_atomic_script`) reads
stdin into `<path>.inspect.<sha8>.tmp`, then conditionally
mirrors mode/ownership from `<path>` (when it exists) via
`chmod --reference` / `chown --reference`. Operator-supplied
`--mode` / `--owner` overrides apply *after* the mirror so they
always win. Atomic `mv` preserves the inode for the prior file's
hardlinks and ensures readers see the file at a consistent state.

**F11 revert capture.** `put` invokes `read_remote` (a `cat --
<path>` round-trip) **before** dispatching the write so the prior
content can be put into the snapshot store and the audit entry's
`revert.kind = state_snapshot` field can point at it. When the
read fails (file does not exist or permission denied), the audit
entry instead records `revert.kind = command_pair` with an
inverse `rm -f -- <path>` so revert deletes the brand-new file.
`get` is read-only on the remote — its `revert.kind` is always
`unsupported` (the operator deletes the local file to undo); the
audit entry still records bytes + sha256 for byte-for-byte
verifiability.

**Container vs host dispatch.** The selector form decides:
`<ns>/_:/path` or `<ns>:/path` runs the helper directly via
`sh -c '...'`; `<ns>/<svc>:/path` wraps it in
`docker exec -i <ctr> sh -c '...'`. Both use the same atomic
helper script.

**No size cap (post-F15).** The pre-F15 `cp` had a 4 MiB hard
cap because the body was base64-encoded into the command argv.
F15 streams via stdin, so the only practical limits are the SSH
master's multiplex starvation behaviour (warning > 1 MiB,
silenceable with `INSPECT_CP_WARN_BYTES=0`) and the remote disk.

**Deferred to v0.1.5:** `--since <duration>` / `--max-bytes
<size>` on `get` (log-retrieval ergonomics, redundant with
`inspect logs --since`), `--resume` for partial transfers
(chunked-protocol design pass).

---

## 13. Streaming executor design (v0.1.3, F16)

`inspect run --stream` (alias `--follow`) wires the existing
line-streaming SSH executor (`run_remote_streaming` in
`src/ssh/exec.rs`, originally written for `inspect logs --follow`)
into the bare `inspect run` path, plus the SSH PTY trick that
makes the remote process line-buffer and propagates Ctrl-C
end-to-end. Three contracts are load-bearing:

**`RunOpts.tty: bool` is the single dispatch knob.** The new
`tty` field on `RunOpts` is the only thing F16 added to the
executor's per-call options struct. It is threaded through all
three SSH dispatch paths — `run_remote`, `run_remote_streaming`,
`run_remote_streaming_capturing` — and at each site adds
`ssh.arg("-tt")` immediately after the `-S <socket>` /
`ControlPath` block, before `BatchMode=yes`. Off by default for
non-streaming runs (PTY allocation can change command behaviour:
CRLF endings via the PTY's ONLCR translation, color output via
`isatty(1)`, prompt suppression for tools that read passwords
from `/dev/tty`); `inspect run --stream` is the only call site
in v0.1.3 that flips it. The drift discovery probe and every
other internal dispatch keep `tty: false` explicitly via the
`RunOpts { ..., tty: false }` struct literal in
`src/discovery/drift.rs`.

**Why `-tt` (double-t) and not `-t`.** OpenSSH's `-t` requests a
PTY *if local stdin is a TTY*, which it never is when `inspect
run` is dispatched (we hand `Stdio::null()` or a piped `Stdio`
to the SSH child). `-tt` forces PTY allocation regardless of
local stdin shape — this is the OpenSSH idiom for "I want a PTY
because the *remote* tool needs one, even though I am wrapping
this in a script." See `ssh(1)` "Multiple -t options force tty
allocation, even if ssh has no local tty."

**Default 8 h timeout.** `inspect run --stream` bumps the verb's
default `--timeout-secs` from 120 to 28 800 (8 hours) because the
operator is expected to terminate via Ctrl-C, not by reaching the
timeout. Matches `inspect logs --follow`'s default so operators
do not have to learn two regimes. Override with `--timeout-secs
<N>` either way; the override path is the same `args.timeout_secs
.unwrap_or(if args.stream { 28800 } else { 120 })` branch in
`src/verbs/run.rs`.

**`AuditEntry.streamed: bool` discriminator field.** New field on
`AuditEntry`, `Option<T>`-shaped via `skip_serializing_if =
"is_false"` so pre-F16 entries deserialize unchanged (matches the
F11/F12/F13/F14/F15 audit-extension pattern). Stamped `true` on
every `--stream` / `--follow` invocation and absent otherwise.
The reason it exists at all: a post-hoc audit query needs to
distinguish `tail -f`-shaped invocations from short-lived
commands without parsing the args text — the same separation
F15's `transfer_direction` field provides for uploads vs
downloads.

**`inspect run` audit trigger surface (post-F16).** `inspect run`
remains un-audited by default (read verb), but writes an audit
entry under any of these triggers:
- F9: forwarded stdin (`stdin_audited` — the default when local
  stdin is a non-tty pipe with data).
- F13: dispatch wrapper retried after a stale-session reauth
  (`exit.retried`).
- F16: `--stream` / `--follow` was set (`stream_audited`).

All three surface via the same audit-write path in the
per-step loop in `src/verbs/run.rs`; `e.streamed = args.stream`
stamps the F16 field on every audit entry written from a `run`
invocation regardless of which trigger fired (so an audit entry
written because of F9 stdin forwarding still records `streamed:
false` correctly via the `skip_serializing_if` shape).

**SIGINT propagation policy (post-F16-followup, v0.1.3).** Two
levels:

1. **First Ctrl-C → SIGINT-via-PTY.** The local `inspect`
   process registers SIGINT handlers via
   `exec::cancel::install_handlers` (see `main.rs`). The
   streaming read loop in `run_remote_streaming` /
   `run_remote_streaming_capturing` polls
   `exec::cancel::signal_count()` on every iteration. On the
   first new signal, the executor writes the ASCII INTR byte
   (`\x03`) into the SSH stdin pipe — the remote PTY's terminal
   driver sees ETX and delivers SIGINT to the remote process
   group. The local cancel flag is then cleared via
   `exec::cancel::reset_cancel_flag()` so the verb surfaces the
   remote's real exit code (matching the field-validation gate's
   "Ctrl-C terminates `docker logs -f`, exit code is the
   docker-logs exit code"). To make this work, the SSH child's
   stdin is now `Stdio::piped()` (instead of `Stdio::null()`)
   when `opts.tty` is set so the executor has a write handle for
   the `\x03` byte.

2. **Second Ctrl-C within 1 second → channel-close-SIGHUP.**
   `classify_cancel()` checks `last_intr_at` against the current
   `Instant`. If a new signal arrived within 1 second of the
   previous forward, the executor escalates: drop the intr pipe,
   `child.kill()` the local SSH process. The channel close
   triggers the remote sshd to deliver SIGHUP to the remote
   process group via PTY teardown — covering the corner case of
   a remote process that ignores SIGINT but exits on SIGHUP.
   Returns exit 130. Same escalation path fires immediately when
   no PTY was allocated (no way to deliver SIGINT through the
   stdin pipe).

The signal counter (`exec::cancel::SIGNAL_COUNT`) is a separate
`AtomicU32` from the cancel flag (`CANCELLED: AtomicBool`)
because the streaming loop needs to detect "a NEW signal arrived
since the previous poll" — the flag alone is a one-way trip.
The counter is incremented from the signal handler
(async-signal-safe relaxed atomic add) and never reset in
production.

**Mutex with `--stdin-script`.** Streaming a script body over
local stdin while also streaming output back is a half-duplex
protocol headache (the SSH stdin channel is now used for
forwarding `\x03` on first Ctrl-C, so a script body taking the
same channel would conflict with SIGINT forwarding). Enforced at
the clap level via `conflicts_with_all = ["no_stdin", "stream"]`
on `stdin_script`. `--stream --file <script>` is fine because
the script body is delivered in one shot at dispatch time, not
streamed (and `--file` doesn't conflict with the F16-followup
intr-pipe hold because `--file` writes the body via
`spawn_stdin_writer` and then drops the handle, whereas
`--stream` without `--file` keeps the pipe open).

---

## 14. Multi-step runner internals (v0.1.3, F17)

`inspect run --steps <manifest.json>` is implemented in
`src/verbs/steps.rs` and short-circuits the bare-`run` per-target
fanout in favor of an explicit per-(step, target) sequential
dispatch loop. Six contracts are load-bearing:

**Multi-target dispatch (sequential within each step).** The
selector resolves to N>=1 targets. Each manifest step fans out
across all N targets sequentially within the step (target 1's
output completes before target 2's begins). Aggregate per-step
status is `ok` only if every target succeeded; `failed` if any
target had a non-zero exit; `timeout` if any target overran
`timeout_s`. `on_failure: "stop"` applies globally (any target's
failure aborts the next manifest step on every target). Parallel
fan-out within a step is intentionally not supported in v0.1.3
(output interleaving + audit-link-ordering races would need a
separate design pass — sequential keeps the audit log
deterministic).

**`steps_run_id` linkage.** A single fresh id is generated at
dispatch time via `AuditEntry::new("steps", &label).id` (matches the
`<ms>-<4hex>` shape every other audit id uses) and stamped onto:
- the parent entry (`verb: "run.steps"`, `id == steps_run_id`,
  `selector: <operator-typed selector>`, `manifest_steps:
  <ordered name list>`, `manifest_sha256: <sha of manifest body>`),
- every per-(step, target) entry (`verb: "run.step"`, `step_name:
  <name>`, `selector: <target's label>`, `steps_run_id: <parent_id>`),
- every auto-revert entry under `--revert-on-failure` (`verb:
  "run.step.revert"`, `auto_revert_of: <original-step-id>`,
  `steps_run_id: <parent_id>`),
- every entry produced by `inspect revert <steps_run_id>` after the
  fact (`verb: "run.step.revert"`, `reverts: <parent_id>`,
  `steps_run_id: <parent_id>`).

The id is never recomputed; an audit query for `steps_run_id =
<id>` returns the full ordered chain regardless of how many
revert passes ran against it.

**Composite payload shape.** F11 declared `RevertKind::Composite`
but had no constructor; F17 nails down the payload as a JSON-encoded
ordered list of `{step_name, kind, payload}` records, in **manifest
order**. `inspect revert <parent-id>` walks the list in reverse so
the most-recent step is undone first. Items with `kind:
"unsupported"` (steps with no declared `revert_cmd`) are skipped at
walk time without aborting the unwind. The list is preserved
verbatim in the parent's `revert.payload` field as a JSON string so
the audit log is the single source of truth for the inverse
dispatch order. The payload is target-agnostic: at revert time, the
parent's `selector` is re-resolved and each inverse fans out
across the resolved targets (matching the original dispatch shape).

**Per-step capture model + 10 MiB cap.** Each (step, target)
dispatches via `runner.run_streaming_capturing` so live progress
prints to local stdout AND the per-(step, target) audit entry
stores a faithful copy of the captured stdout. The captured copy
is capped at **10 MiB per (step, target)**; live printing
continues unaffected past the cap. When the cap is reached, the
captured copy stops growing and stamps `output_truncated: true`
on the per-target result so JSON consumers know the captured
blob is partial. Cap matches the F9 `--stdin-max` default for
consistency. Under `--steps --stream`, each per-(step, target)
dispatch flips `tty: true` on its `RunOpts` so the remote process
line-buffers (live output instead of 4 KB bursts) and the F16
PTY/SIGINT propagation contract applies per step.

**Per-step audit + parent audit ordering.** Audit entries are
appended in dispatch order: each per-step entry lands immediately
after that step's dispatch returns; the parent entry lands AFTER
the entire pipeline (and after any `--revert-on-failure`
auto-revert entries) so its `duration_ms` reflects the full
wall-clock span. Auto-revert entries from `--revert-on-failure`
land between the last per-step entry and the parent entry, in
reverse-manifest order, so reading the audit log top-to-bottom
mirrors the on-screen order the operator saw.

**`cmd_file` F14 composition.** A step with `cmd_file: "./x.sh"`
reads the local file body, ships it via `bash -s` over SSH stdin,
and stamps `script_sha256` + `script_bytes` + `script_path` onto
the per-step audit entry — the same fields F14 stamps for
`inspect run --file`. **The body is read twice** during dispatch
(once to compute the sha + size for the audit entry, once to
provide the bytes for the dispatch closure); this is a
~negligible-cost simplification that keeps the closure's
lifetime trivial. Heavy callers can pre-compress or use a `cmd`
that references a remote-side script if the double-read is a
concern.

**Default per-step timeout: 8 hours.** Mirrors the F16 `--stream`
default. Migration steps can run for many minutes (atlas vault
data migrations in the field have hit ~12 minutes on cold caches);
the default cap should not be the thing that aborts a real
migration. Operators override per step with `timeout_s` in the
manifest.

**Failure-class taxonomy on per-step entries.** Each per-step
entry carries `failure_class`: `"ok"` (exit 0), `"command_failed"`
(non-zero exit, command actually ran on remote), `"transport_error"`
(SSH layer failed), `"timeout"` (per-step wall-clock overrun, exit
recorded as `-2`). The parent's `failure_class` is `"ok"`,
`"stopped_on_failure"` (one or more steps failed AND
`on_failure: "stop"` aborted the pipeline), or `"command_failed"`
(steps failed but `on_failure: "continue"` ran the rest).

**F13 + F17 dispatch wrapping.** Each (step, target) dispatch is
wrapped in `dispatch_with_reauth` so a stale-socket failure on
target N of M during step K triggers F13's transparent reauth +
retry path on that exact (step, target) pair, without aborting
the rest of the pipeline. The retried entry stamps `retry_of`
+ `reauth_id` for cross-correlation with the inserted
`connect.reauth` entry. The per-target result's `retried: true`
flag surfaces in the JSON output so wrappers can spot transparent
recoveries.

**`--reason` + `--steps` audit.** When `--reason "<text>"` is
passed alongside `--steps`, the text is echoed to stderr at start
(matching bare `inspect run` semantics) AND stamped onto the
parent `run.steps` audit entry's `reason` field so a 4-hour
migration's operator intent is recoverable from the audit log
alone — no terminal scrollback required.

**Out of scope for v0.1.3.** Parallel multi-target fan-out within
a single step. Sequential fan-out within each step is shipped;
parallel is genuinely a separate design pass — output
interleaving + audit-link-ordering races would require a render
+ capture refactor.

---

## 15. Audit retention + orphan-snapshot GC (v0.1.3, L5)

`~/.inspect/audit/` and `~/.inspect/audit/snapshots/` grew without
bound through v0.1.2. After F11/F14/F15/F16/F17 each added new audit
fields and (in F17's case) per-(step, target) plus per-revert
entries that multiply the per-mutation footprint by an order of
magnitude, the maintenance gap had to close. L5 adds one verb plus
an opt-in lazy trigger.

### 15.1 The deletion algorithm

Single source of truth: `src/safety/gc.rs::run_gc(policy, dry_run)`.
Every code path — the `audit gc` subcommand, the lazy trigger from
`AuditStore::append`, and the unit tests — calls this one function.

1. **Walk every JSONL file** under `~/.inspect/audit/` (top level,
   not the `snapshots/` subdir). Files are ordered alphabetically;
   within each file, entries are read in append order. Malformed
   lines are skipped, not fatal.
2. **Compute the deletion set** from the [`RetentionPolicy`]:
   - `Duration(d)` keeps entries where `now - entry.ts <= d`.
   - `Count(n)` keeps the newest `n` entries **per namespace**.
     Namespace is parsed from the entry's `selector` field
     (`arte/atlas-vault` → `arte`); selector-less entries group
     under the sentinel `_`.
3. **Pin every snapshot reachable from a *retained* entry.** Walk
   the entry's `previous_hash`, `new_hash`, `snapshot` filename,
   and `revert.payload` for `state_snapshot` reverts. For
   `composite` reverts (F17 parent entries), parse the JSON
   payload and **recurse** into its array — nested
   `state_snapshot` records pin further hashes. Best-effort:
   malformed JSON is ignored (the GC must never delete a
   snapshot it can't prove is orphaned).
4. **Sweep `~/.inspect/audit/snapshots/`** for `sha256-<hex>`
   files whose hash isn't in the pinned set. `.part` temp files
   and anything not matching the `sha256-<hex>` shape is left
   alone.
5. **Mutate (skipped on `--dry-run`)**:
   - For each JSONL file with at least one to-delete entry:
     write the kept entries to `<file>.gctmp.<pid>` (mode 0600
     from the start on unix) and `rename(2)` over the original.
     If every entry in a file was deleted, the file is
     `unlink(2)`-ed instead.
   - For each orphan snapshot file: `remove_file`.
   - Touch `~/.inspect/audit/.gc-checked` so subsequent appends
     don't re-scan within the same minute.

`freed_bytes` in the report covers BOTH JSONL shrinkage AND snapshot
file sizes — the operator reads one number to size the next
retention window.

### 15.2 The pinned-snapshot invariant

The GC has exactly one invariant the config cannot relax: a snapshot
referenced by any retained audit entry is **never** deleted. This is
the F11 revert contract. The acceptance test
`l5_gc_keeps_snapshot_referenced_by_retained_entry` is the
load-bearing regression guard — a failure here means the GC could
silently break `inspect revert <id>` on retained entries, which
violates the v0.1.3 production-grade-only mandate. If a future
refactor of `RevertKind` adds a new variant that pins additional
snapshots, `collect_snapshot_hashes` and `collect_composite_hashes`
in `src/safety/gc.rs` must be extended in lockstep.

### 15.3 The lazy trigger and its cheap-path guard

`[audit] retention = "<X>"` in `~/.inspect/config.toml` opts an
installation in. The trigger fires from `AuditStore::append` after
every successful append — i.e. after every write verb that produced
an audit record. Without the cheap-path guard, this would mean an FS
scan per audit entry, which a busy F17 multi-step run could fire
hundreds of times per minute.

The cheap path:

1. Read `~/.inspect/config.toml`. If `[audit] retention` is unset,
   return immediately — the most common case for installations
   that haven't opted in.
2. Stat `~/.inspect/audit/.gc-checked`. If it exists and its mtime
   is within the last 60 seconds, return immediately. **No FS scan
   beyond the marker stat.**
3. Touch the marker (this happens *before* the rotation pass so a
   transient failure doesn't make us retry every audit append for
   the next minute).
4. For duration policies: stat every JSONL file in
   `~/.inspect/audit/`, take the minimum mtime, compare against
   the cutoff. If the oldest file is fresher, return without
   scanning entries.
5. For count policies: skip the mtime probe (it can't decide them)
   and run a full pass.
6. Run `run_gc(policy, false)`.

Errors from the lazy path are swallowed at the `let _ = ...` call
site in `AuditStore::append` so a transient GC failure cannot break
the just-appended audit record.

### 15.4 The global config file

`~/.inspect/config.toml` is a fresh file shipped by L5. It is
distinct from `servers.toml` (per-namespace runtime config); the
intent is cross-cutting policy that is not keyed on a server. The
schema lives in `src/config/global.rs::GlobalConfig` and is empty by
default. Future cross-cutting toggles (cache TTLs, history rotation,
default redaction policy) plug in as new tables here without
polluting the per-server schema.

A missing file is not an error — `load()` returns
`GlobalConfig::default()` and lazy GC stays off until the operator
opts in.

### 15.5 Acceptance test surface

11 tests in `tests/phase_f_v013.rs::l5_*` lock the contract:

- Dry-run reports counts but doesn't modify the JSONL.
- Apply deletes old entries and orphan snapshots; rewrites JSONL
  atomically; unlinks fully-emptied JSONL files.
- Snapshot pinned by a retained entry's `revert.payload` is NEVER
  deleted (the F11 invariant).
- JSON envelope schema: `dry_run` / `policy` / `entries_total` /
  `entries_kept` / `deleted_entries` / `deleted_snapshots` /
  `freed_bytes` / `deleted_ids` / `deleted_snapshot_hashes`.
- Count policy keeps the newest N per namespace, not overall.
- Invalid `--keep` exits with a chained hint pointing at
  `inspect audit --help`.
- `--keep 0` is rejected loudly.
- `--help` documents the GC + RETENTION section, the `[audit]
  retention` config hook, and the cheap-path-marker semantics.
- Empty audit dir yields zero counts (clean fresh-install path).
- Manual `audit gc` touches the cheap-path marker so a subsequent
  audit append within 60s no-ops the lazy trigger.
- Lazy GC fires on the next audit append when the oldest JSONL
  file's mtime crosses the threshold (uses
  `std::fs::File::set_modified` to backdate the seed file; runs
  `cache clear arte` to produce the audit entry that drives the
  trigger).

Plus 9 unit tests in `src/safety/gc.rs::tests` (parser edge cases,
namespace extraction, composite-payload recursion) and 3 in
`src/config/global.rs::tests` (missing file = defaults; valid
retention parses; empty file = defaults).

---

## 16. Session transcripts (v0.1.3, F18)

Per-namespace, per-day, human-readable transcripts complement the
structured audit log. Each `~/.inspect/history/<ns>-<YYYY-MM-DD>.log`
is a single file (mode 0600) holding one fenced block per verb
invocation against that namespace.

### 16.1 The tee architecture

Every `inspect <verb>` invocation is a separate process. The
transcript writer is per-process state held in
`OnceLock<Mutex<Option<TranscriptContext>>>`, installed at the top
of `main()` (after `cancel::install_handlers()` and before clap
parsing) via `transcript::init(&argv)`. The `argv` is captured
post-redaction (`--password=` / `--token=` flags masked).

User-visible output flows through two macros defined in
`src/transcript.rs`:

- `tee_println!` — writes to stdout AND appends to the per-process
  buffer.
- `tee_eprintln!` — same for stderr.

Every central rendering site uses these — `Renderer::print`,
`JsonOut::write`, `OutputDoc::print_*`, `format::render::render_doc`,
and the streaming line-emit sites in run/logs/cat/grep/find/merged/
steps/watch/status/health/cache/why/ports/connectivity/network/
correlation/cursor/dispatch/transfer/ls/ps/images/volumes.
`error::emit` tees stderr too. New verbs MUST use the macros at
their emit sites or their output won't appear in the transcript.

### 16.2 The namespace hook

`transcript::set_namespace(ns)` resolves the per-ns redaction mode
+ disabled flag from `[namespaces.<ns>.history]` and stamps the
transcript context. It fires from two places:

1. `verbs::runtime::resolve_target` — every dispatch verb that
   crosses this function (every read/write verb that talks to a
   remote host).
2. Verbs that manage namespaces locally without going through
   `resolve_target`. Today that's `inspect cache clear <ns>`; new
   verbs of this shape MUST call `transcript::set_namespace`
   explicitly.

If neither hook fires, `finalize` short-circuits: no transcript is
written, no `_global-*.log` file appears in the history dir.

### 16.3 The redaction pipeline

Every line tee'd to the transcript runs through the L7 four-masker
pipeline (`OutputRedactor::new(false, false)`) before being
appended to the buffer. The PEM masker's `None` return suppresses
the line (just like in stdout); the BEGIN-line marker is what gets
stored. Per-namespace `redact = "off"` short-circuits the masker
entirely; `redact = "strict"` is reserved (identical to "normal" in
v0.1.3 — the future tightening will mask any KEY=VALUE whose key
looks secret-shaped).

`--show-secrets` on the originating verb bypasses redaction in
both stdout and transcript — single flag, single bypass.

### 16.4 The audit cross-link

`AuditStore::append` calls `transcript::set_audit_id(&entry.id)`
on first append (subsequent calls in the same verb are no-ops:
first-write-wins). This means the transcript footer's
`audit_id=<id>` always points at the **parent** entry on
multi-audit verbs (F17 `--steps` parent, bundle parent, etc.) —
not the per-step entries. Forensic round-trip from a transcript
hit back to a structured audit entry is one `inspect audit show
<id>` away.

### 16.5 The fenced-block format

```text
── 2026-04-28T14:32:11Z arte #b8e3a1 ──────────────────────────
$ inspect run arte -- 'docker ps'
<output lines, post-redaction>
── exit=0 duration=423ms audit_id=01HXR9Q5YQK2 ──
```

The fence pattern is `awk '/^── /,/^── exit=/'`-friendly. Footer
omits `audit_id=` when the verb didn't append an audit entry (read
verbs without F17/F11 instrumentation, e.g. `inspect status`).

### 16.6 Performance + buffer cap

Output accumulates in memory during the verb. At finalize, the
full block is written via `OpenOptions::new().create(true).append(true).open(...)` and `sync_data()`. **One fdatasync(2) per verb
invocation, regardless of output volume** — satisfies the F18
≤ 70-fsyncs-per-10-min performance gate trivially.

The buffer is capped at 16 MiB (`MAX_BUFFER_BYTES` in
`src/transcript.rs`). Past the cap, the buffer is closed with a
`[transcript truncated: buffer cap reached]` marker and subsequent
writes are no-ops. A runaway streaming verb cannot OOM the
process — the cap is reached, the marker is recorded, the verb
keeps writing to stdout normally.

### 16.7 Rotation + retention + compression

`src/transcript/rotate.rs::run_rotate(policy, dry_run)` is the
single source of truth. Three steps in order:

1. Delete files dated older than `retain_days` (default 90).
2. Gzip files dated older than `compress_after_days` (default 7)
   that aren't already `.gz`. Atomic via `<name>.part` →
   `rename(2)`. Original is `unlink(2)`-ed only after the rename
   succeeds.
3. Enforce `max_total_mb` cap (default 500): sort oldest-first,
   evict until under. Today's file is never evicted (active write
   target).

`maybe_run_lazy()` fires from `transcript::finalize` gated by a
once-per-day marker (`~/.inspect/history/.rotated`, mtime-checked
against a 23-hour stale window). Errors swallowed at the call site
so transient rotation failure cannot break the just-emitted
transcript block. `inspect history rotate` calls `run_rotate(None)`
explicitly with the on-disk policy.

### 16.8 The `inspect history` verb tree

Implemented in `src/commands/history.rs`. Subcommands:

- `show [<ns>] [--date YYYY-MM-DD] [--grep <pattern>] [--audit-id
  <id>]` — renders fenced blocks. Transparently decompresses
  `.log.gz` via `read_transcript`. Filter logic: `--date` selects
  by file (one per day), `--grep` filters per-block on the full
  block bytes (header + argv + body + footer), `--audit-id` filters
  on the footer's `audit_id=` token (substring match).
- `list [<ns>]` — walks the dir grouping by (namespace, date) with
  byte sizes; sort by namespace then date desc.
- `clear <ns> --before YYYY-MM-DD` — deletes files in that namespace
  with date < cutoff. `--yes` gate to confirm.
- `rotate` — calls `run_rotate(None)`.

All four have `--json` envelopes (top-level, no `data` wrapper).

### 16.9 Writing new verbs F18-correctly

Three rules:

1. Use `tee_println!` / `tee_eprintln!` for any line that should
   appear in the operator's transcript. Bare `println!` /
   `eprintln!` calls write to stdout/stderr only.
2. If your verb resolves a namespace, it should go through
   `verbs::runtime::resolve_target`. If it must handle the
   namespace directly (e.g. local-only operations like cache
   clear), call `crate::transcript::set_namespace(ns)` explicitly
   at the point where the namespace is committed.
3. Audit entries written via `AuditStore::append` automatically
   stamp the transcript's audit_id — no extra work needed.

---

## 17. Per-branch rollback in bundle matrix steps (v0.1.3, L6)

The bundle executor in `src/bundle/exec.rs` was extended in v0.1.3
to track per-branch outcomes for `parallel: true` + `matrix:` steps
so rollback inverts only the branches that actually applied an
effect. Pre-L6, a 4-of-6 partial failure rolled back all 6
branches — including the 4 that succeeded — and the rollback body
was rendered with an empty matrix map so any `{{ matrix.<key> }}`
reference silently expanded to nothing.

### 17.1 Data model

```rust
pub(crate) struct BranchResult {
    pub branch_id: String,           // "<key>=<value>", e.g. "svc=atlas"
    pub status: BranchStatus,        // Ok | Failed | Skipped
    pub matrix_value: serde_yaml::Value,
    pub matrix_key: String,
}

pub(crate) enum StepOutcome {
    Single,
    Matrix(Vec<BranchResult>),
}
```

The `apply` loop carries `step_branches: BTreeMap<usize,
Vec<BranchResult>>` alongside the existing `completed: Vec<usize>`.
Single-branch steps are recorded only in `completed`; matrix steps
are recorded in both (so the iteration order in `do_rollback` stays
declaration-order-reverse).

### 17.2 The lifecycle of a matrix branch

1. `run_step` dispatches `parallel: true` + `matrix:` steps to
   `run_parallel_matrix`.
2. `run_parallel_matrix` spawns scoped threads (cap = `max_parallel`,
   default = number of matrix entries, hard-capped at 8) that pull
   from a shared queue and call `run_single_branch` per branch.
3. Each `run_single_branch` invocation builds a `{matrix_key →
   matrix_value}` map, dispatches the body via the existing
   selector pipeline, appends one audit entry per `exec`/`watch`
   step, and stamps `bundle_branch = "<key>=<value>"` +
   `bundle_branch_status = "ok"|"failed"` on the entry.
4. On worker completion, the parent thread appends a `BranchResult`
   to a shared `Vec<BranchResult>` (via `Mutex<>`).
5. On first failure, the worker sets `stop_flag = true`. Other
   workers check this flag at the top of their loop and exit
   without pulling more entries — the leftover queue items are
   then synthesized as `Skipped` BranchResults so the post-mortem
   table is complete.
6. Branches are sorted by `branch_id` before return so the rollback
   walk and post-mortem queries see a deterministic order
   regardless of worker scheduling.

### 17.3 Threading the partial-failure ledger to apply

Bare `Result<T, E>` can't carry a payload alongside an `Err`. We
use a `BranchFailureCarrier` thread-local sidecar:

```rust
thread_local! {
    static BRANCH_LEDGER_SIDECAR: RefCell<Option<Vec<BranchResult>>> =
        const { RefCell::new(None) };
}
```

When `run_parallel_matrix` returns `Err(...)`, it stashes the
partial branch ledger into the cell. The apply loop's `Err(e)` arm
calls `BranchFailureCarrier::drain()` immediately and stores the
result in `step_branches[idx]`. This avoids widening every error-
bearing helper's signature and keeps the matrix-failure path
locally reasonable.

### 17.4 Branch-aware rollback

`do_rollback` was rewritten to consult `step_branches`:

- For each step in `to_visit.iter().rev()` (the union of
  `completed` and `step_branches.keys()` — partial-failure steps
  that didn't make it into `completed` still need their succeeded
  branches inverted):
  - If the step has an entry in `step_branches`:
    - For each `Ok` branch: build `{matrix_key → matrix_value}`,
      run `interpolate(rb_cmd, &bundle.vars, &mtx)`, dispatch the
      rollback body, append a `bundle.rollback` audit entry with
      `bundle_branch` + `bundle_branch_status` stamped.
    - For each `Failed` / `Skipped` branch: emit a
      `bundle.rollback.skip` audit entry whose
      `diff_summary` carries the why-skipped explanation. No
      remote dispatch.
  - Otherwise (single-branch step): the legacy single-rollback
    path runs unchanged with an empty matrix map.

The composition with `on_failure: rollback_to: <id>` is
unchanged: the existing checkpoint loop walks until it hits the
target step. A fully-succeeded matrix step before the checkpoint
stays in place; a partially-failed matrix step's succeeded
branches get inverted because the failed step itself triggered
the rollback.

### 17.5 The `inspect bundle status <id>` verb

`src/commands/bundle.rs::status` reads every audit entry, prefix-
matches the bundle id (ambiguous → exit 2 with the match list;
unknown → exit 1 with chained hint), groups entries by
`bundle_step`, and renders the per-branch table. It treats a step
as "matrix" when at least one of its entries has
`bundle_branch.is_some()`, otherwise "single". `--json` returns
the full structure for agent consumption — see §14.1 of MANUAL for
the schema.

The audit log is the source of truth — a bundle that ran a year ago
is queryable as long as its entries haven't aged out per L5
retention. No per-bundle on-disk state is added; if you `inspect
audit gc --keep 30d` and a bundle's entries fall outside the
window, `bundle status` will report no-match.

### 17.6 Schema hygiene

Two new optional `AuditEntry` fields:

- `bundle_branch: Option<String>` — `<matrix-key>=<value>` (e.g.
  `volume=atlas_milvus`).
- `bundle_branch_status: Option<String>` — `"ok"` | `"failed"` |
  `"skipped"`.

Both `Option<T>` with `skip_serializing_if`, so pre-L6 entries
deserialize unchanged. The pre-existing `is_revert: bool` is now
explicitly set on `bundle.rollback` audit entries (a v0.1.2 omission
that meant `inspect audit grep --revert` missed bundle-level
inversions).

---

*Source: this runbook implements Phase 12 of the original implementation
plan in `archives/IMPLEMENTATION_PLAN.md`. §8 was added in v0.1.3 (F2)
to lock in the three-bucket discipline; §9 in F12 (env overlay); §10
in F13 (auto-reauth + transport exit class); §11 in F14 (script
mode); §12 in F15 (file transfer); §13 in F16 (streaming executor);
§14 in F17 (multi-step runner); §15 in L5 (audit gc + lazy trigger);
§16 in F18 (session transcripts); §17 in L6 (per-branch rollback).
Any deviation between this runbook and the bible is a runbook bug.*
