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
