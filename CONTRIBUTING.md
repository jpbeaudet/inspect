# Contributing to `inspect`

Thanks for considering a contribution. This file is the short version
of the dev loop and the rules every PR has to clear before it merges.

## Dev setup

Requirements: Rust 1.75 or newer (pinned in `rust-toolchain.toml`),
plus an OpenSSH client at runtime if you want to exercise the SSH
paths.

```sh
git clone https://github.com/jpbeaudet/inspect
cd inspect
cargo build
cargo test
```

The full test suite is fast and offline (no live SSH required).

## Quality gates

Every PR has to pass on Linux and macOS:

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets`
- `cargo build --locked`
- `cargo test --locked`
- The MSRV (1.75) build job.

Additional contract tests enforce the project's invariants. They live
in `tests/` and run as part of `cargo test`:

- `tests/no_dead_code.rs` — `#[allow(dead_code)]` is denied except
  with a `// v2: <tag>` comment, no module-wide
  `#![allow(dead_code)]` is allowed, and the total surviving
  suppressions stay at or below 1.
- `tests/help_contract.rs`, `tests/help_json_snapshot.rs` — the
  in-binary help surface is an API; changes have to update the
  snapshot deliberately.
- `tests/phase*` — phase-by-phase regression suites for selectors,
  discovery, SSH lifecycle, the LogQL parser, exec, formats, fleet,
  diagnostics. New behavior should be covered here.

`Cargo.toml` carries `[lints.rust] dead_code = "deny"`. CI builds with
`RUSTFLAGS=-D warnings`. Don't introduce warnings.

## Coding rules

- **Stable surfaces are stable.** The CLI flag set, the JSON envelope
  schema, exit codes (`0` ok, `1` no-match, `2` error), and the
  in-binary help topics are the public contract. Any change to them
  needs an entry in `CHANGELOG.md` and, for breaking changes, a major
  version bump.
- **Dry-run is the default for any new write verb.** The safety
  contract documented in `inspect help safety` is non-negotiable.
- **No secrets at rest.** Profile files are mode 0600 and contain no
  credentials. The redactor stays deterministic.
- **No silent no-op.** A selector that matches nothing prints what is
  available.
- **No new top-level dependencies without a reason.** The release
  artifacts are static-musl; size and supply-chain surface matter.

## Commit + PR shape

- Small, focused PRs. One concern per PR.
- Conventional-style subject lines are appreciated but not required.
- Reference an issue if there is one.
- Update `CHANGELOG.md` under `## [Unreleased]` for any user-visible
  change.
- For docs-only changes, mention `[skip-changelog]` in the PR
  description.

## What is in scope vs. v2

The "Known limitations" section of the README lists features
intentionally out of scope for v0.1.0 (TUI, k8s discovery, password
auth, parameterized aliases, etc.). PRs implementing those are very
welcome but should be flagged as v2 work, target a `v2/<feature>`
branch, and come with a design note in `archives/` first.

## Reporting bugs

For ordinary bugs: open a GitHub issue with a minimal repro and the
output of `inspect --version`. For security issues: see
[SECURITY.md](SECURITY.md).

## Code of conduct

Be excellent to each other. We follow the spirit of the
[Contributor Covenant](https://www.contributor-covenant.org/). Personal
attacks, harassment, and discriminatory language are not welcome.
