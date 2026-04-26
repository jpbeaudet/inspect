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

*Source: this runbook implements Phase 12 of the original implementation
plan in `archives/IMPLEMENTATION_PLAN.md`. Any deviation between this
runbook and the bible is a runbook bug.*
