# CLAUDE.md — agent guide for working on `inspect`

This file is loaded automatically by Claude Code in every session
on this repo. It captures durable invariants — things that are true
across releases, not the current sprint's status.

For human contributors, see [`CONTRIBUTING.md`](CONTRIBUTING.md).

---

## Operating context

`inspect` is in active production use:

- Real operators run it against real servers (the primary author plus
  multiple v0.1.2 field users) for real-time diagnostics, audited
  mutations, and multi-hour destructive migrations. Every CHANGELOG
  entry traces back to a field-feedback incident or a retrospective
  finding from a real operations session.
- Agentic callers (LLMs invoking `inspect` from a developer
  workstation or codespace) are a **first-class consumer**: stable
  JSON envelopes, explicit exit codes, redacted stdout, chained
  recovery hints, and `-h`-discoverable contracts all exist because
  of them.
- **You** (Claude Code, this session) will drive `inspect` against a
  real server during end-to-end smoke tests at release time. Any bug
  shipped is a bug you may step on personally.

Two consequences shape every policy below:

- **Production grade only.** No shortcuts, no "good enough for a
  demo", no commented-out tests, no `TODO: revisit later`, no
  `.unwrap()` on a fallible boundary. If a contract is not ready to
  meet a real operator at 2am, it is not ready to ship. Every commit
  must leave the tree in a state that could be released as-is.
- **No backward compatibility until v0.2.0.** Until the v0.2.0
  contract-freeze release, *break what needs breaking* — CLI flags,
  JSON schemas, config formats, on-disk artifact layouts, audit
  schema fields, exit-code semantics. v0.1.x is the last window for
  that. Migration shims, deprecation aliases, and "for backwards
  compat" branches are technical debt: fix the design, don't paper
  over it. From v0.2.0 onward, any contract breakage requires a
  major version bump.

---

## No silent deferrals

Never stub a function with a "deferred to vX" message without
explicit approval. If a feature is in the current backlog,
implement it fully.

Run before every commit (alongside the fmt / clippy / test gates):

```sh
grep -rE "defer|stub|todo|unimplemented|exit\(2\)" src/
```

Inspect every hit. Legitimate matches (e.g. `std::process::exit(2)`
on a clap usage-error path, the existing `exit-code 2` documentation
in `LONG_*` help constants) are fine — but a `// deferred to v0.1.5`
or an `unimplemented!()` in a backlog-scoped code path is a policy
violation, not a follow-up.

Approved deferrals are tracked here. Adding an entry requires the
maintainer's explicit "ok defer X" in the conversation; agents do
not self-authorize.

### Authorized deferrals

*(none yet for v0.1.3)*

---

## Pre-commit gates (mandatory)

Run before *every* commit, in this order:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test 2>&1 | grep -E "^test result|^running" | tail -40
```

A clean run is ~28 suites, 900+ tests, < 90s. Treat any failure as a
hard stop, never `--no-verify`. The flake on
`verbs::cache::tests::ttl_zero_makes_every_cached_snapshot_stale` is
an env-var parallel-execution race that does not reproduce when run
in isolation — re-run the full suite once if you see it.

**WSL `cargo` PATH (Claude-Code session).** This dev environment is
WSL (`/mnt/c/Users/hp/inspect`) and the harness's default shell does
**not** have `~/.cargo/bin` on `PATH`. Every cargo invocation must
prefix:

```sh
export PATH="$HOME/.cargo/bin:$PATH"; cargo …
```

Bare `cargo …` will hit `command not found`. Apply the prefix to
fmt / clippy / test / build / run / anything cargo.

**Test output discipline.** Always pipe `cargo test` through
`| grep -E "^test result|^running" | tail -40` (per the gate
above) — the full output is hundreds of lines per suite and burns
context for nothing. Running the suite a second time *just to
change the output format* is wasted compute (28 suites × ~30s).
If the first run was clean, the gate is green; do not re-run.
The grep form shows one `running N tests` + one `test result:`
line per suite, which is enough to spot any non-zero `failed`
count.

## Per-backlog-item update sweep (mandatory)

When a backlog item ships, every commit that closes it must update
**all five** surfaces. Missing one is a policy violation, not a
follow-up:

1. **Source code + tests.** Acceptance tests live in
   `tests/phase_f_v013.rs` named `<id>_*` (e.g. `f14_*`, `l7_*`).
   Every sub-item gets at least one test.
2. **`CHANGELOG.md`.** New bullet at the top of the v0.1.3 `Added`
   section. Style is verbose and uses field-feedback quotes when
   relevant — match the surrounding entries (F14/F13/L7 are recent
   examples). Behavior changes / new audit fields / new exit codes
   get explicit flags in the bullet.
3. **`INSPECT_v0.1.3_BACKLOG.md`.** Mark the item's row `✅ Done`
   and replace the placeholder Notes column with a short technical
   summary (modules touched, key flags, test counts, doc files
   updated). Match the F12/F13/F14/L7 style.
4. **`docs/MANUAL.md` (and `docs/RUNBOOK.md` when applicable).**
   New user-facing contracts get a section. The release-readiness
   gate at the bottom of the backlog enumerates which sections each
   item must touch — read it before drafting.
5. **Interactive `-h` help.** This is the load-bearing surface:
   **LLM agents learn the CLI from `-h` first**, not from
   `MANUAL.md` or the README. Every new flag must have descriptive
   help text; every new JSON field / exit code / state value must
   appear in the relevant verb's `LONG_*` constant in `src/cli.rs`
   or in an editorial topic under `src/help/content/`. When in
   doubt, audit `inspect <verb> --help` and `inspect help <topic>`
   for the new contract before committing.

## Help-text discoverability

The help surface is **first-class API for agentic callers**, not a
nicety:

- New flag → docstring on the clap arg with the rationale, the
  feature ID (`F<n>` / `L<n>`), and version (`v0.1.3`).
- New JSON field → document the discriminator values inline in the
  verb's `LONG_*` (see `LONG_STATUS` for the F7.5 `state` field
  pattern).
- New exit code → mention in the verb's `LONG_*` and in
  `inspect help safety` if it's a forensic concern.
- New cross-cutting model (redaction, env overlay, auto-reauth) →
  add a section to the relevant editorial topic under
  `src/help/content/` (`safety.md`, `write.md`, `formats.md`, etc.).
- The help search index has a size cap in `src/help/search.rs`. If
  prose additions push it over, raise the cap (precedent: 50→64 KB
  in v0.1.2, 64→80 KB in v0.1.3) — do not trim documentation to fit.

## Commit conventions

- **One commit per backlog item.** Don't bundle. If a sub-item
  surfaces during another item's implementation, finish the current
  one first and stage the side-find as a separate commit.
- **Subject:** `<ID>: <short description>` — e.g. `L7: header / PEM
  / URL credential redaction in remote command output`.
- **Body:** sub-section breakdown (modules, key API additions, audit
  shape changes, help-text changes, test counts). Match recent
  commits like `48a760d` (F14) or `99f4cb3` (L7).
- **Footer:** `Closes <ID> in INSPECT_v0.1.3_BACKLOG.md.` plus the
  `Co-Authored-By:` trailer.
- **Never commit without explicit user request.** Ask before staging
  if the user has not said "commit" in the current turn.
- **Never push without explicit user request.** Pushing is a
  separate authorization from committing.

## Audit schema

`AuditEntry` (`src/safety/audit.rs`) is on a path to freeze in
v0.2.0. Until then:

- New fields must be `Option<T>` with
  `#[serde(skip_serializing_if = "Option::is_none")]` so pre-existing
  JSONL entries deserialize without migration.
- Text-side audit args use bracketed tags:
  `[secrets_masked=true]`, `[secrets_exposed=true]`, etc. Add new
  tags to the same family rather than inventing new shapes.
- Every write verb must capture a revert (F11 contract). New write
  verbs declare `revert.kind` (`command_pair` / `state_snapshot` /
  `composite` / `unsupported`) and a capture function
  before-apply.

## CLI surface invariants

- **JSON envelopes carry state discriminators** so agents branch
  without parsing prose: `state`, `failure_class`, `revert.kind`,
  `secrets_masked_kinds`, etc.
- **Error messages chain to recovery.** Every error that exits != 0
  ends with a `hint:` or `see: inspect help <topic>` line pointing
  at the next operator action.
- **Mutual exclusion is enforced at clap.** Use `conflicts_with`
  / `conflicts_with_all` rather than runtime checks where possible.
- **Exit codes are stable contract.** `0` ok, `1` no-match, `2`
  argument/usage error, `12-14` transport (F13), inner exit code
  passthrough on `run`/`exec`. Don't reuse a code for a new meaning.

## Dependency Policy

Prefer native Rust implementations over external crates. Only add a dependency when:
1. The domain is genuinely unsafe to reimplement (SSH, cryptography)
2. The crate has years of production use and active maintenance
3. There is no reasonable way to implement it in under 500 lines of our own code

Current approved dependencies exist for strong reasons (openssh, tokio, clap, serde, sha2, rpassword, zeroize, similar, comfy-table, crossterm, indicatif, ratatui). Everything else — parsers, formatters, pipeline stages, template engines, protocol handlers — we write ourselves.

When in doubt, write it native. Open source implementations are reference material, not imports.

## Naming + scope

- `F<n>` items are field-feedback (operator pain). `L<n>` items are
  pre-existing limitations from the roadmap. **Do not conflate**
  the prefixes — they live in different sections of the backlog and
  ship in different orders. Test names use the lowercase prefix
  (`f14_*`, `l7_*`).
- v0.1.3 is **OPEN, FROZEN** — final scope is the 25 items in
  `INSPECT_v0.1.3_BACKLOG.md`. Don't expand mid-implementation;
  surface scope creep as a question to the user.
- v0.1.4 = Kubernetes only. v0.1.5 = stabilization sweep. v0.2.0 =
  contract freeze. Anything docker/compose/SSH that doesn't ship in
  v0.1.3 will not be touched again until v0.1.5+.

## Working with mid-state working trees

A working tree handed off from a previous session may have
implementation code with no documentation, or documentation with no
implementation, or partial work. **Verify before assuming**:

- Read the actual diff against `HEAD` — don't trust hand-off
  descriptions.
- Test names (`l7_*` vs `f7_*`) and module paths
  (`src/redact/` vs `src/redact.rs`) are authoritative signals of
  *which* item is in flight.
- If the working-tree code does not match the user's stated item,
  flag the mismatch loudly **before** taking any action that
  documents or commits it. The most expensive class of bug in this
  workflow is "documented the wrong item under the right ID".

## Reference paths

- Source: `src/` (verbs in `src/verbs/`, write verbs in
  `src/verbs/write/`, editorial help in `src/help/content/`)
- Tests: `tests/phase_f_v013.rs` for v0.1.3 work; `tests/phase_*`
  for older phases; in-tree unit tests next to the code.
- Docs: `docs/MANUAL.md` (operator), `docs/RUNBOOK.md` (release
  + maintenance), `INSPECT_v0.1.3_BACKLOG.md` (current scope).
- Audit log path: `~/.inspect/audit/<YYYY-MM>-<user>.jsonl`.
- Profile / config: `~/.inspect/servers.toml` (mode 0600).
