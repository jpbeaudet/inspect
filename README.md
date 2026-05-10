# inspect

[![ci](https://github.com/jpbeaudet/inspect/actions/workflows/ci.yml/badge.svg)](https://github.com/jpbeaudet/inspect/actions/workflows/ci.yml)
[![release](https://github.com/jpbeaudet/inspect/actions/workflows/release.yml/badge.svg)](https://github.com/jpbeaudet/inspect/actions/workflows/release.yml)
[![license](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![status: experimental](https://img.shields.io/badge/status-experimental-orange.svg)](#stability)

> ⚠️ **Experimental until v0.2.0.** The CLI surface, profile schema,
> and audit format may break between minor releases while the tool is
> shaped against real-world usage. That said, **it is already in
> active use on live production systems** for SRE and agent-driven
> debugging — it works, it is safe (dry-run by default, full audit
> trail), it is just not yet API-stable. Pin a release tag and read
> the [CHANGELOG](CHANGELOG.md) before upgrading until v0.2.0 ships.

`inspect` is an operational debugging CLI for fleets of servers
reached over SSH. One tool to **search** logs and config across many
machines, **diagnose** what is running, **safely apply** hot-fixes
with a built-in audit + revert trail, and **orchestrate** declarative
multi-step migrations with rollback.

- **Local-first.** No agent, no daemon, no central server. Just SSH
  (and `docker` / `systemctl` on the remote).
- **Dry-run by default.** Every mutating command previews a diff;
  `--apply` is the only way to enact a change. Every apply is audited
  and reversible with `inspect revert <audit-id>`.
- **Stable JSON envelope.** Every command can emit a versioned
  `summary | data | next` envelope (`--json`); the in-binary
  `--select '<jq>'` flag projects or reshapes it without an external
  `jq` install (F19), and the envelope still pipes cleanly into `jq`,
  scripts, or another tool.
- **LogQL-style search.** A familiar Loki-like query language to
  grep, parse, and aggregate across logs, files, and host state.
- **Bundle orchestration.** Declarative YAML migrations with
  preflight / postflight checks, parallel matrix steps, per-step and
  bundle-level rollback, all grouped by `bundle_id` in the audit log.
- **Block-until-condition.** `inspect watch` waits on log lines, SQL
  predicates, HTTP probes, or arbitrary commands — exits 0 on match,
  124 on timeout. Composes with `&&` and slots into bundles.
- **Built-in manual.** `inspect help`, `inspect help <topic>`, and
  `inspect help search <query>` work offline — no man pages, no
  network.

> **Current release:** `v0.1.3` — password auth + extended session
> TTL + `ssh add-key` helper, optional OS keychain, audit log
> retention, header / PEM / URL credential redaction,
> parameterized aliases, per-branch matrix rollback,
> per-namespace env overlay, stale-session auto-reauth,
> universal `--revert`, file-transfer (`put` / `get` / `cp`),
> session transcript, multi-step runner, first-class compose
> verbs, jaq-powered `--select` projection on every JSON-emitting
> verb. See [archives/v0.1.3/INSPECT_v0.1.3_BACKLOG.md](archives/v0.1.3/INSPECT_v0.1.3_BACKLOG.md)
> for the closed scope.
>
> **Next:** `v0.1.4` — Kubernetes support (mixed Docker + k8s
> fleets). `v0.1.5` — stabilization sweep before the v0.2.0
> contract freeze.

---

## Table of contents

- [Install](#install)
- [Quickstart](#quickstart)
- [How it works](#how-it-works)
- [Bundles](#bundles)
- [Watch](#watch)
- [Documentation](#documentation)
- [Building from source](#building-from-source)
- [Stability](#stability)
- [Contributing](#contributing)
- [Security](#security)
- [License](#license)

---

## Install

> The one-line installer is fetched directly from
> `raw.githubusercontent.com` — there is no separate server to deploy.
> Pushing `scripts/install.sh` to the default branch and tagging a
> release is enough.

### One-line (recommended)

```sh
curl -fsSL https://raw.githubusercontent.com/jpbeaudet/inspect/main/scripts/install.sh | sh
```

Pin a version, change the install root, or skip cosign verification:

```sh
curl -fsSL https://raw.githubusercontent.com/jpbeaudet/inspect/main/scripts/install.sh \
  | sh -s -- --version v0.1.3 --prefix /usr/local
```

The installer:

1. Resolves the latest release tag (or the one you pinned).
2. Downloads the right tarball for your host triple.
3. Verifies the SHA-256 checksum.
4. If `cosign` is on `$PATH`, verifies the keyless signature against
   the GitHub OIDC issuer.
5. Atomically installs the binary into `$PREFIX/bin` (default
   `~/.local/bin`).

### Homebrew (custom tap)

```sh
brew tap jpbeaudet/tap
brew install inspect
```

See [packaging/homebrew/inspect.rb](packaging/homebrew/inspect.rb) and
[docs/RELEASING.md](docs/RELEASING.md) for how the formula is published.

### Cargo

```sh
cargo install inspect-cli
```

### Pre-built binaries

Download from [GitHub Releases](https://github.com/jpbeaudet/inspect/releases).
Each archive ships with `.sha256`, `.sig`, and `.pem` for cosign
keyless verification — see [docs/RUNBOOK.md](docs/RUNBOOK.md) §1.2.

Tier-1 platforms: Linux (musl, x86_64 + aarch64), macOS (Intel +
Apple Silicon).

---

## Quickstart

```sh
# 1. Register a server (interactive — uses your ~/.ssh/config)
inspect add arte

# 2. Open one persistent SSH session for the rest of the work
inspect connect arte

# 3. Discover what is running on it
inspect setup arte
inspect ps arte
inspect status arte

# 4. Search across the fleet, LogQL-style
inspect search '{server="arte", source="logs"} |= "error"' --tail 100 --json

# 5. Preview a hot-fix, then apply it
inspect edit arte/atlas:/etc/atlas.conf 's/timeout=30/timeout=60/'
inspect edit arte/atlas:/etc/atlas.conf 's/timeout=30/timeout=60/' --apply --reason "raise atlas timeout for batch run"

# 6. Roll it back if it goes wrong
inspect audit ls --limit 5
inspect revert <audit-id> --apply

# 7. Wait for a remote condition (block-until-true)
inspect watch arte/atlas-api --until-log 'Started server' --timeout 60s

# 8. Run a declarative multi-step migration
inspect bundle plan ops/migrations/phase-0-snapshot.yaml
inspect bundle apply ops/migrations/phase-0-snapshot.yaml --apply --reason "atlas centralization phase 0"

# 9. Built-in manual — works offline
inspect help
inspect help search
inspect help bundle
inspect help watch
```

A longer guided tour lives in **[docs/MANUAL.md](docs/MANUAL.md)**.

---

## How it works

```
   you ──► inspect (local) ──► ssh ControlMaster ──► remote host
                │                                       │
                │                                       ├── docker / podman
                │                                       ├── systemctl
                │                                       └── POSIX coreutils
                ▼
        ~/.inspect/
          ├── profiles/<ns>.yaml      # discovered topology, mode 0600
          ├── audit/<id>/             # snapshot + diff per mutation
          └── audit/bundles/<id>/     # bundle-grouped audit entries
```

- **One SSH session per host.** OpenSSH `ControlMaster` keeps a
  single authenticated channel open for the duration of your shell.
  Type the passphrase once.
- **Profiles are cached.** `inspect setup <ns>` snapshots every
  container, volume, network, and listening port into
  `~/.inspect/profiles/<ns>.yaml`. Drift is detected on next use.
- **Mutations are reversible.** Before a write, the original is
  snapshotted under `~/.inspect/audit/`. `inspect revert <id>`
  replays the snapshot.
- **Bundles are correlated.** Every step inside `inspect bundle apply`
  shares one `bundle_id` in the audit log; `inspect audit ls --bundle
  <id>` reconstructs the whole transaction.
- **Secrets are redacted.** Output is filtered through a deterministic
  redactor before printing or logging. `--show-secrets` opts out.

---

## Bundles

`inspect bundle` runs a YAML-described sequence of `exec` / `run` /
`watch` steps with rollback semantics. The shape that worked on real
migrations:

```yaml
name: atlas-phase-0-snapshot
host: arte
reason: "Phase 0 pre-flight snapshot"

vars:
  snapshot_dir: /srv/snapshots/2026-04-27
  services: { clients: [atlas-api, nexus-api, onyx-api] }

preflight:
  - check: disk_free
    path: /srv/snapshots
    min_gb: 50
  - check: docker_running
    services: [atlas-pg, aware-milvus]

steps:
  - id: stop-clients
    exec: docker compose -f /srv/atlas/docker-compose.yml stop {{ services.clients }}
    on_failure: abort

  - id: dump-pg-atlas
    exec: docker exec atlas-pg pg_dumpall -U postgres | gzip > {{ snapshot_dir }}/atlas-pg.sql.gz
    requires: [stop-clients]
    on_failure: { rollback_to: stop-clients }

  - id: tar-volumes
    parallel: true
    matrix:
      volume: [atlas_milvus, atlas_etcd, aware_milvus]
    exec: docker run --rm -v {{ matrix.volume }}:/src -v {{ snapshot_dir }}:/dst alpine tar czf /dst/{{ matrix.volume }}.tar.gz -C /src .

rollback:
  - exec: docker compose -f /srv/atlas/docker-compose.yml start {{ services.clients }}

postflight:
  - exec: sha256sum {{ snapshot_dir }}/* > {{ snapshot_dir }}/MANIFEST.sha256
  - check: services_healthy
    services: [atlas-api, nexus-api]
    timeout: 60s
```

`inspect bundle plan <file>` always dry-runs (resolves variables,
runs preflight checks, no remote writes). `inspect bundle apply
<file> --apply` is required to enact destructive steps. First-class
checks: `disk_free`, `docker_running`, `services_healthy`, `http_ok`,
`sql_returns`, plus an `exec` escape hatch.

`inspect help bundle` covers the full surface.

---

## Watch

`inspect watch` is the synchronous primitive bundles consume. It also
stands alone for "did the service come back?" / "is the queue
drained?" checks:

```sh
# Wait for a log line
inspect watch arte/atlas-api --until-log 'Started server on 0.0.0.0:8000' --timeout 60s

# Wait for a SQL predicate
inspect watch arte/atlas-pg \
  --until-sql "SELECT count(*) = 0 FROM pg_stat_activity WHERE state = 'active'" \
  --interval 2s --timeout 5m

# Wait for an HTTP response
inspect watch arte/onyx-api --until-http https://localhost:8080/health --match 'status == 200' --timeout 90s

# Wait for an arbitrary command's output
inspect watch arte/temporal --until-cmd 'pending_jobs' --equals 0 --timeout 10m
```

Exit codes: `0` match, `124` timeout (matches `timeout(1)`), `130`
cancelled, `2` error. Every watch writes one audit entry with the
predicate, elapsed time, and matching value (or last-observed on
timeout).

---

## Documentation

| Doc | Audience | What is in it |
|---|---|---|
| [docs/MANUAL.md](docs/MANUAL.md) | end users | Hands-on user manual: install, register hosts, every verb, search DSL, recipes, bundles, watch, troubleshooting. |
| [docs/RUNBOOK.md](docs/RUNBOOK.md) | maintainers / on-call | Release rollout, incident response, hotfix flow, support matrix, current limitations. |
| [docs/RELEASING.md](docs/RELEASING.md) | maintainers | How to cut a tag, what the release workflow does, how to update the Homebrew tap. |
| [CHANGELOG.md](CHANGELOG.md) | everyone | Per-release changes (Keep a Changelog format). |
| [archives/](archives/) | everyone | Closed planning + smoke + audit artifacts from prior releases (v0.1.2, v0.1.3). |
| [CONTRIBUTING.md](CONTRIBUTING.md) | contributors | Dev setup, lint/test gates, PR rules. |
| [SECURITY.md](SECURITY.md) | reporters | How to report a vulnerability. |
| `inspect help <topic>` | end users | The same manual content, embedded in the binary, no network. |

In-binary topics: `quickstart`, `selectors`, `aliases`, `search`,
`formats`, `write`, `safety`, `fleet`, `recipes`, `bundle`, `watch`,
`discovery`, `ssh`, `examples`.

---

## Building from source

Requirements: Rust 1.75+ (pinned in [rust-toolchain.toml](rust-toolchain.toml)),
plus an OpenSSH client at runtime.

```sh
cargo build --release
cargo test
```

A static-musl Docker image is also provided:

```sh
docker build -t inspect:dev .
docker run --rm -it -v ~/.ssh:/home/inspect/.ssh:ro inspect:dev help
```

CI gates: `cargo fmt --all -- --check`, `cargo clippy --all-targets
-- -D warnings`, full test suite (28 suites, 1350+ tests at v0.1.3),
no module-wide lint suppressions.

---

## Stability

`inspect` follows semver, but with one explicit pre-1.0 caveat: **the
contract begins at v0.2.0**. Until then, breaking changes may land in
any release. The current shape:

| Release | What ships |
|---|---|
| v0.1.0 | First public release: 12 read verbs, 12 write verbs, LogQL parser, selectors, aliases, discovery, audit, snapshots, revert, 10 output formats, fleet, recipes, why, connectivity, help. |
| v0.1.1 | `run` verb, `--follow`, `--merged`, `--match` / `--exclude`, `--since-last`, secret masking, `--reason`, progress, exit-code surfacing, phantom-service fix. |
| v0.1.2 | Bundle orchestration (B9), `watch` verb (B10), field-feedback patches B1–B8, defensive hardening pass (audit fsync, http timeouts, panic-safe matrix). |
| v0.1.3 | Password auth + session TTL + `ssh add-key`, OS keychain, audit retention + GC, header / PEM / URL / env redaction, parameterized aliases, per-branch matrix rollback, per-namespace env overlay, stale-session auto-reauth, universal `--revert`, `put` / `get` / `cp`, session transcript, multi-step runner, first-class `compose` verbs, jaq-powered `--select` projection. |
| v0.1.4 | Kubernetes support: mixed Docker + k8s fleets, k8s-aware selectors, scoped write verbs (`scale`, `restart`, `delete pod` with audit), bundle-engine integration. |
| v0.1.5 | Pre-stabilization sweep: CLI surface audit, config / JSON schema freeze, help audit, README rewrite, dead-code + dependency audit, security audit. **No new features.** |
| v0.2.0 | Stability contract begins. |

After v0.2.0: CLI verb names, flag names, selector grammar, LogQL
syntax, `--json` schema (versioned), config formats, bundle YAML,
recipe YAML, audit log schema, exit codes, and help topic names are
**stable**. Internal APIs, error wording, performance characteristics,
discovery heuristics, and TUI keybindings remain unstable.

See [archives/v0.1.3/INSPECT_ROADMAP_TO_v01.3.md](archives/v0.1.3/INSPECT_ROADMAP_TO_v01.3.md) for the
full plan and [docs/RUNBOOK.md](docs/RUNBOOK.md) §6 for the
always-current list of known limitations.

---

## Contributing

Bug reports and PRs are welcome. Read
[CONTRIBUTING.md](CONTRIBUTING.md) for the dev loop, coding rules
(`-D warnings`, `dead_code = "deny"`, no module-wide lint
suppressions), and the test contract that gates merges.

---

## Security

If you find a vulnerability, **do not** open a public issue. See
[SECURITY.md](SECURITY.md) for the disclosure process.

---

## License

[Apache-2.0](LICENSE).

---

## Development history

The implementation plans, design "bibles", audit checklists, and
field-pitfall catalogs that drove the v0.1.0 → v0.1.2 builds live
under [archives/](archives/) for reference. They are historical and
not part of the maintained surface; the active docs are everything in
`docs/` and the in-binary `inspect help`.
