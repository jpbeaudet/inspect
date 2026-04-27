# inspect

[![ci](https://github.com/jpbeaudet/inspect/actions/workflows/ci.yml/badge.svg)](https://github.com/jpbeaudet/inspect/actions/workflows/ci.yml)
[![release](https://github.com/jpbeaudet/inspect/actions/workflows/release.yml/badge.svg)](https://github.com/jpbeaudet/inspect/actions/workflows/release.yml)
[![license](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![status: experimental](https://img.shields.io/badge/status-experimental-orange.svg)](#stability)

> ⚠️ **Experimental until v0.2.0.** The CLI surface, profile schema, and
> audit format may break between patch releases while the tool is shaped
> against real-world usage. That said, **it is already in active use on
> live production systems** for SRE and agent-driven debugging — it
> works, it's safe (dry-run by default, full audit trail), it's just
> not yet API-stable. Pin a release tag and read the
> [CHANGELOG](CHANGELOG.md) before upgrading until v0.2.0 ships.

`inspect` is an operational debugging CLI for fleets of servers reached
over SSH. It gives you one tool to **search** logs and config across
many machines, **diagnose** what's running, and **safely apply**
hot-fixes with a built-in audit + revert trail.

- **Local-first.** No agent, no daemon, no central server. Just SSH
  (and `docker` / `systemctl` on the remote).
- **Dry-run by default.** Every mutating command previews a diff;
  `--apply` is the only way to enact a change. Every apply is audited
  and reversible with `inspect revert <audit-id>`.
- **Stable JSON envelope.** Every command can emit a versioned
  `summary | data | next` envelope (`--json`) suitable for piping into
  `jq`, scripts, or another tool.
- **LogQL-style search.** A familiar Loki-like query language to grep,
  parse, and aggregate across logs, files, and host state.
- **Built-in manual.** `inspect help`, `inspect help <topic>`, and
  `inspect help search <query>` work offline — no man pages, no network.

> Status: **v0.1.0, first public release.** Tier-1 platforms are
> Linux (musl, x86_64 + aarch64) and macOS (Intel + Apple Silicon).
> See [Known limitations](#known-limitations) for what is intentionally
> out of scope for this version.

---

## Table of contents

- [Install](#install)
- [Quickstart](#quickstart)
- [How it works](#how-it-works)
- [Documentation](#documentation)
- [Building from source](#building-from-source)
- [Known limitations](#known-limitations)
- [Contributing](#contributing)
- [Security](#security)
- [License](#license)

---

## Install

> **Note on the install URL.** The one-line installer is fetched
> directly from `raw.githubusercontent.com` — there is **no separate
> server to deploy**. Pushing `scripts/install.sh` to the default branch
> and tagging a release is enough.

### One-line (recommended)

```sh
curl -fsSL https://raw.githubusercontent.com/jpbeaudet/inspect/main/scripts/install.sh | sh
```

Pin a version, change the install root, or skip cosign verification:

```sh
curl -fsSL https://raw.githubusercontent.com/jpbeaudet/inspect/main/scripts/install.sh \
  | sh -s -- --version v0.1.0 --prefix /usr/local
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

(See [packaging/homebrew/inspect.rb](packaging/homebrew/inspect.rb) and
[docs/RELEASING.md](docs/RELEASING.md) for how to publish the formula.)

### Cargo

```sh
cargo install inspect-cli
```

### Pre-built binaries

Download from [GitHub Releases](https://github.com/jpbeaudet/inspect/releases).
Each archive ships with `.sha256`, `.sig`, and `.pem` for cosign
keyless verification — see [docs/RUNBOOK.md](docs/RUNBOOK.md) §1.2.

---

## Quickstart

```sh
# 1. Register a server (interactive — uses your ~/.ssh/config)
inspect add arte

# 2. Open one persistent SSH session for the rest of the work
inspect connect arte

# 3. Discover what's running on it
inspect setup arte
inspect ps arte
inspect status arte

# 4. Search across the fleet, LogQL-style
inspect search '{server="arte", source="logs"} |= "error"' --tail 100 --json

# 5. Preview a hot-fix, then apply it
inspect edit arte/atlas:/etc/atlas.conf 's/timeout=30/timeout=60/'
inspect edit arte/atlas:/etc/atlas.conf 's/timeout=30/timeout=60/' --apply

# 6. Roll it back if it goes wrong
inspect audit ls --limit 5
inspect revert <audit-id> --apply

# 7. Built-in manual — works offline
inspect help
inspect help search
inspect help search 'metric query'
```

For a longer guided tour, see **[docs/MANUAL.md](docs/MANUAL.md)**.

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
          └── audit/<id>/             # snapshot + diff per mutation
```

- **One SSH session per host.** OpenSSH `ControlMaster` keeps a single
  authenticated channel open for the duration of your shell — type the
  passphrase once.
- **Profiles are cached.** `inspect setup <ns>` snapshots every
  container, volume, network, and listening port into
  `~/.inspect/profiles/<ns>.yaml`. Drift is detected on next use.
- **Mutations are reversible.** Before a write, the original is
  snapshotted under `~/.inspect/audit/`. `inspect revert <id>` replays
  the snapshot.
- **Secrets are redacted.** Output is filtered through a deterministic
  redactor before printing or logging.

---

## Documentation

| Doc | Audience | What's in it |
|---|---|---|
| [docs/MANUAL.md](docs/MANUAL.md) | end users | Hands-on user manual: install, register hosts, every verb, search DSL, recipes, troubleshooting. |
| [docs/RUNBOOK.md](docs/RUNBOOK.md) | maintainers / on-call | Release rollout, incident response, hotfix flow, support matrix. |
| [docs/RELEASING.md](docs/RELEASING.md) | maintainers | How to cut a tag, what the release workflow does, how to update the Homebrew tap. |
| [CHANGELOG.md](CHANGELOG.md) | everyone | Per-release changes (Keep a Changelog format). |
| [CONTRIBUTING.md](CONTRIBUTING.md) | contributors | Dev setup, lint/test gates, PR rules. |
| [SECURITY.md](SECURITY.md) | reporters | How to report a vulnerability. |
| `inspect help <topic>` | end users | The same manual content, embedded in the binary, no network. |

In-binary topics: `quickstart`, `selectors`, `aliases`, `search`,
`formats`, `write`, `safety`, `fleet`, `recipes`, `discovery`, `ssh`,
`examples`.

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

---

## Known limitations

The following are intentionally out of scope for v0.1.0 and tracked as
v2 work in `archives/INSPECT_BIBLEv6.2.md` §27:

- No TUI mode.
- No Kubernetes discovery (Phase 12 ships docker + systemd only).
- No distributed tracing integration.
- No OS keychain integration for SSH passphrases.
- No per-user policy enforcement (single global safety gate).
- No russh-based fallback when the system `ssh` is unavailable.
- No parameterized aliases (`@logs(svc=$x)` is reserved syntax).
- No password authentication (key-based only).
- No remote agent — `inspect` is strictly local-first.
- Windows host is unsupported (target hosts can be anything reachable
  over SSH).

See [docs/RUNBOOK.md](docs/RUNBOOK.md) §6 for the always-current list.

---

## Contributing

Bug reports and PRs are welcome. Please read
[CONTRIBUTING.md](CONTRIBUTING.md) for the dev loop, coding rules
(`-D warnings`, `dead_code = "deny"`, no module-wide lint suppressions),
and the test contract that gates merges.

---

## Security

If you find a vulnerability, please **do not** open a public issue.
See [SECURITY.md](SECURITY.md) for the disclosure process.

---

## License

[Apache-2.0](LICENSE).

---

## Development history

The implementation plans, design "bibles", audit checklists, and
field-pitfall catalogs that drove the v0.1.0 build live under
[archives/](archives/) for reference. They are historical and not part
of the maintained surface; the active docs are everything in `docs/`
and the in-binary `inspect help`.
# inspect

[![ci](https://github.com/jpbeaudet/inspect/actions/workflows/ci.yml/badge.svg)](https://github.com/jpbeaudet/inspect/actions/workflows/ci.yml)
[![release](https://github.com/jpbeaudet/inspect/actions/workflows/release.yml/badge.svg)](https://github.com/jpbeaudet/inspect/actions/workflows/release.yml)
[![license](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

Operational debugging CLI for cross-server search and safe hot-fix
application. Resolve a fleet of namespaces, fan out over SSH + Docker,
emit a stable `summary | data | next` envelope, and stay rollback-safe
end-to-end.

## Install

### One-line

```sh
curl -fsSL https://raw.githubusercontent.com/jpbeaudet/inspect/main/scripts/install.sh | sh
```

Pin a version, install root, or skip cosign verification:

```sh
curl -fsSL .../install.sh | sh -s -- --version v0.1.0 --prefix /usr/local
```

The installer downloads the right tarball for your host triple,
verifies sha256, optionally verifies the cosign keyless signature when
`cosign` is on `$PATH`, and installs atomically.

### Homebrew (custom tap)

```sh
brew tap jpbeaudet/tap
brew install inspect
```

### Cargo

```sh
cargo install inspect-cli
```

### Pre-built binaries

Download from [GitHub Releases](https://github.com/jpbeaudet/inspect/releases) —
each archive ships with `.sha256`, `.sig`, and `.pem` for cosign
keyless verification. See [docs/RUNBOOK.md](docs/RUNBOOK.md) §1.2.

## Quickstart

```sh
# Capture a profile for a namespace defined in ~/.ssh/config + ~/.inspect/
inspect setup arte

# What's running, what's healthy
inspect ps arte
inspect status arte

# LogQL-style streaming search across the fleet
inspect search '{server="arte", source="logs"} |= "error"' --tail 100 --json

# Dry-run a hot-fix; --apply to enact, revert by audit id
inspect edit /etc/atlas.conf arte/atlas
inspect edit /etc/atlas.conf arte/atlas --apply
inspect revert <audit-id>

# Built-in help — no network, no man pages
inspect help
inspect help logs
inspect help search 'pattern'
```

See [docs/RUNBOOK.md](docs/RUNBOOK.md) for the GA runbook (release rollout,
incident response, hotfix flow, support matrix, known limitations).

## Building from source

Requirements: Rust 1.75+ (see [rust-toolchain.toml](rust-toolchain.toml)).

```sh
cargo build --release
cargo test
```

## License

Apache-2.0 — see [LICENSE](LICENSE).

## Development history

The implementation plans, bibles, audit checklists, and field-pitfall
catalogs that drove the v0.1.0 build live under
[archives/](archives/) for reference.
# inspect
CLI to inpect multiple logs and output srouce with serach and transform POXIS command and ssh handling for multiserver
