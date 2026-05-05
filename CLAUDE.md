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

Never stub a function or paper over a feature gap with a "deferred
to vX" / "out of scope" / "out-of-scope" / "postponed" / "punted"
message without explicit approval. If a feature is in the current
backlog (any status row not yet `✅ Done`), implement it fully —
sequencing it later in the same release is **not** the same as
deferring it.

Phrases like "out of scope for v0.1.3" are particularly tempting
escape hatches because they read as scoping decisions rather than
deferrals. They are deferrals. Treat them the same way.

Run before every commit (alongside the fmt / clippy / test gates):

```sh
grep -rEi "defer|stub|todo|unimplemented|exit\(2\)|out of scope|out-of-scope|postponed|punted" src/ docs/ CHANGELOG.md INSPECT_v0.1.3_BACKLOG.md
```

Inspect every hit. Legitimate matches (e.g. `std::process::exit(2)`
on a clap usage-error path, the existing `exit-code 2` documentation
in `LONG_*` help constants, "out of scope for v0.1.5" pointing at a
*future* release that is not the current one, doc-comment `stub`
referring to the help system's intentional fallback rendering) are
fine. But a `// deferred to v0.1.5` in a v0.1.3-backlog code path,
an `unimplemented!()` in an L<n> sub-feature, or a CHANGELOG
"Out of scope for v0.1.3" line about an item that is in the v0.1.3
backlog is a policy violation, not a follow-up.

**Sequencing inside a release is not deferring.** L2 ships after L4
in v0.1.3 because L4 is its prerequisite — that is sequencing.
Writing "OS keychain is out of scope for v0.1.3" while L2 is
unticked in the backlog is a deferral, and dishonest besides.
Don't do it. If a sequencing note is genuinely useful for a
reader, write "L2 lands in a follow-up commit (next in the
implementation order)" — accurate and not a deferral claim.

Any line that implies a deferral — in code, in CHANGELOG, in
MANUAL, in the BACKLOG Notes column — must either:

1. point at a future release that is not the current one
   (`v0.1.5+`, `v0.2.0+`), or
2. correspond to an entry in the **Authorized deferrals** registry
   below.

Adding a registry entry requires the maintainer's explicit "ok
defer X" in the conversation; agents do not self-authorize.

### Authorized deferrals

- **L1 — TUI mode (`inspect tui`).** Authorized deferral to v0.5*
  (or whenever a strong human-operator use case surfaces). The
  inspect user base today is LLM-driven; a read-only `ratatui`
  dashboard is keyboard-driven interactive by design and offers
  no value to agentic callers — every L1 capability (status pane,
  log follow, drill-into-why, service detail card) is already
  exposed as a composable JSON-emitting verb that's strictly
  better for an agent than scraping screen output. The few
  human operators who do use inspect work via JSON envelopes +
  shell scrollback rather than a dashboard. Authorized on
  2026-05-03 in conversation; re-open if a real human-operator
  complaint about interactive triage ergonomics surfaces. The
  L1 row in `INSPECT_v0.1.3_BACKLOG.md` is preserved for
  traceability with status "🟦 Deferred (authorized)" rather
  than being deleted.

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

## Field-validated invariants (operational lessons)

These are operational truths burnt into the tree by real release-smoke
sessions. Every line below corresponds to a bug shipped to production
or a recipe that wasted multiple agent turns. Treat them as
non-negotiable: re-discovering them is wasted compute.

### Signal handling

- **SIGPIPE must be reset to `SIG_DFL` at startup.** Rust's stdlib
  installs `SIG_IGN`, so `println!` / `writeln!` to a closed pipe
  panics with `failed printing to stdout: Broken pipe`. For an
  agent-facing CLI piped through `head`, `grep -m1`, `jq` etc.,
  every short-circuited pipeline ended in exit 101 + backtrace
  instead of the conventional silent 141. Fixed in
  `src/exec/cancel.rs::install_sigpipe_default()`, called from
  `install_handlers()` which `main.rs` invokes early. Regression
  test `smoke_sigpipe_no_panic_on_early_pipe_close` in
  `tests/phase_f_v013.rs`. **Never remove or condition this.**

### F11 universal-revert capture-site contract

The `Revert` enum has four kinds (`Unsupported`, `CommandPair`,
`StateSnapshot`, `Composite`). Capture sites are **authoritative**:

- **`command_pair(payload, preview)` argument order is load-bearing.**
  `payload` = the literal shell command the runner will dispatch on
  the remote. `preview` = human prose for `audit show` / `revert
  --dry-run`. Reversing them is a 100% silent failure that exits 127
  at revert time. Inline comments at every capture site state the
  contract; new sites must follow.
- **In-container verbs pre-wrap their own `docker exec`.** chmod /
  chown / mkdir / touch revert payloads are the literal
  `docker exec <ctr> <cmd>` string, not a bare command that the
  runner would have to re-wrap. The runner runs payloads as-is.
- **CLI-only inverses are `Unsupported`, not `command_pair`.** If the
  inverse is "run the same `inspect` binary with different flags"
  (e.g. `ssh add-key`'s revoke, `bundle compose:up`'s down), the
  runner cannot dispatch `inspect` on the remote target. Use
  `Revert::unsupported(<manual command in preview>)` so the operator
  sees the inverse but `revert --apply` doesn't try to run prose.
- **Audit hash IDs use `sha256:HEX` (colon).** The on-disk store uses
  `sha256-HEX` (dash). Strip *both* forms in any path-builder; see
  `SnapshotStore::strip_sha256_prefix`.
- **Avoid GNU-only flags in capture-site commands.**
  `chmod --reference=PATH` / `chown --reference=PATH` are GNU-only;
  Alpine/BusyBox spew usage. Use POSIX-portable
  `stat -c '%a'` / `stat -c '%u:%g'` and substitute the value into
  `chmod NNNN` / `chown UID:GID`.
- **Targeted `revert <id> --apply` must NOT prompt.** All three
  revert paths (`revert_command_pair`, `revert_state_snapshot`,
  `revert_composite`) use `Confirm::LargeFanout` rather than
  `Confirm::Always` — agents cannot answer `[y/N]`.

### JSON output contract

- **`audit ls/show/grep/gc/verify --json` emit the standard envelope**
  `{schema_version, summary, data, next, meta}`, same as every other
  envelope verb. Pre-fix shape was bare-NDJSON / bare-object and
  caused `.[0]` / `| length` jq recipes to fail with "Cannot index
  object with number". Don't regress.
- **`compose ls --json` envelope path is `.data.compose_projects[]`,
  field is `.name`.** `compose ps --json` payload path is
  `.data.services[]` (object-keyed `.data`, not array). The shared
  `--json` flag help-string says "line-delimited JSON" but envelope
  verbs emit a *single* envelope — the help string is misleading
  for envelope verbs and accurate only for true NDJSON streams
  (audit history-text streams, run --stream stdout, etc.). When in
  doubt, probe with `inspect <verb> --json | wc -l` (1 = envelope,
  N = NDJSON) and `jq -c '. | type, keys?'`.
- **Audit ordering is newest-first.** `audit ls` sorts via
  `sort_by_key(Reverse(e.ts))`. The most recent entry is `head -1`
  / `.data.entries[0]`, **never** `tail -1`. The `audit ls`
  projection omits the `revert` block — round-trip via
  `audit show <id> --json` to inspect `revert.kind` / payload /
  preview. `LONG_AUDIT_LS` and the clap `///` docs on `Ls` / `Show`
  / `Grep` enforce this; don't dilute them.

### Build + test gating

- **Always build with the real release profile.** `Cargo.toml`
  ships `lto = "thin"` + `codegen-units = 1` because that's what
  end users get from `cargo install` and from tagged binaries.
  Smoke-validate against *that* binary, not a debug or
  LTO-disabled variant — runtime semantics are equivalent in
  theory, but optimizer-level codegen has surfaced real bugs in
  this codebase before (LTO inlining changed a panic location;
  codegen-units=16 reordered a dropck path). **Do not override**
  `CARGO_PROFILE_RELEASE_LTO` or `CARGO_PROFILE_RELEASE_CODEGEN_UNITS`
  to fit a constrained environment; if a host OOMs on
  `cargo build --release`, run on a roomier host. Validating on a
  reduced binary and shipping the optimized one is how surprise
  bugs reach production.
- **WSL/codespace `cargo` PATH.** Default shell does not have
  `~/.cargo/bin` on `PATH`. Prefix every cargo invocation with
  `export PATH="$HOME/.cargo/bin:$PATH"; cargo …` or it hits
  `command not found`.
- **Pre-commit gate is `cargo fmt --check && cargo clippy
  --all-targets -- -D warnings && cargo test`.** Targeted
  `cargo test --test <name>` is fine for iteration; the full gate
  must pass before every commit.

### SSH ControlMaster reuse

- **`ssh_precheck` must short-circuit when the master socket is
  alive.** Before `7d588d2`, precheck spawned a fresh `BatchMode`
  ssh that fails on encrypted keys *even with a master alive*,
  blocking every verb. The fix: short-circuit on
  `socket_exists_and_is_fresh(ns)`. With a live master, an agent
  can run any read/write/lifecycle verb without the user's
  passphrase env var present in the spawn.

- **Every ssh-spawn site sets `StrictHostKeyChecking=accept-new`.**
  inspect's askpass is a passphrase helper — it returns the value
  of an env var, blindly. OpenSSH's default
  `StrictHostKeyChecking=ask`, combined with our
  `SSH_ASKPASS_REQUIRE=force`, routes the host-key
  confirmation prompt (`Are you sure you want to continue
  connecting (yes/no/[fingerprint])?`) through askpass on every
  first-connect — askpass returns the passphrase value, ssh
  rejects it as "neither yes/no/fingerprint", reprompts, and
  burns turns in a tight loop until the operator ^C's. The fix
  is one `-o StrictHostKeyChecking=accept-new` per ssh-spawn
  site (`build_master_command` and `build_precheck_command`
  both have it; the dispatch path runs through the master
  socket so it inherits the already-verified channel). The
  *changed*-key case still aborts with `Host key verification
  failed.`, which `ssh_precheck::classify` catches as
  `HostKeyChanged` and routes to `host_key_changed_hint`. New
  ssh-spawn sites (verbs that build a Command for `ssh` directly
  rather than dispatching through the master socket) MUST also
  ride `accept-new` or they re-introduce the trap. The two
  unit tests in `master::accept_new_tests` and
  `ssh_precheck::tests::precheck_command_includes_accept_new`
  pin the contract.

### Smoke-runbook + recipe traps

- **Set `SMOKE_CTR` explicitly in every terminal session.** It does
  not survive `exec bash` or new VS Code terminal panes. Empty
  expansion produces `docker exec  sh` / `docker logs -f` failures
  that look like CLI bugs but are environment.
- **F5 dual-axis: `cat`/`ls`/`find`/`grep` docker dispatch must use
  `step.container()`, not `step.service()`.** When a compose
  service's `container_name` differs from the service name (e.g.
  service `api` → container `luminary-api`), dispatching by service
  name lands the verb on a non-existent container. The bug is
  silent — the verb errors with `No such container: <service>`.
- **`--quiet` is mutex with `--json`** at clap level (F7.4 contract).
  `--json` is already trailer-free; piping JSON output through
  filters does not need `--quiet`. Drop `--quiet` from any `--json`
  recipe.

### Help-surface discipline

- **Help text is API for agents, not a nicety.** Every flag,
  envelope shape, and exit code that an agent can hit through `-h`
  must be self-describing. When a smoke session burns N turns on a
  shape mismatch, the fix is *both* code and help text — agents
  read help first. Pre-existing examples: `LONG_GREP` /
  `LONG_FIND` defending against `--path` / `--name` muscle memory;
  `LONG_AUDIT_LS` "ORDERING + JSON PROJECTION" section;
  `LONG_BUNDLE` documenting which compose-step revert kinds are
  `unsupported` vs `command_pair`.
- **Verbose help text + search index.** When prose pushes the help
  search index over the cap in `src/help/search.rs`, **raise the
  cap, don't trim docs.** Precedent: 50→64 KB (v0.1.2), 64→80 KB
  (v0.1.3).

### LLM-trap fix-on-first-surface (mandatory)

When help text, runbook prose, or an error message confuses the
agent **even once** during smoke or normal driving, fix it
immediately — same turn, same commit. The threshold is **one
confused turn**, not N. Agents are supposed to drop into this tool
cold from `--help` and the runbook; if a single re-read was needed
to understand a contract, that's a bug in the surface, not a
"you'll learn it" note for the next session.

Two non-negotiable parts:

1. **Fix on first surface.** Do not "make a note to fix later." Do
   not bundle traps into a separate cleanup commit. The fix lands
   in the same commit that closes the smoke turn the trap caught,
   *or* as its own commit before continuing the smoke. Continuing
   past an unfixed trap is forbidden.

2. **Sweep the same pattern across the codebase.** If the trap is
   "`audit ls --json` emits a bare array but the help string
   implied an envelope," the fix is **not** just `audit ls`'s help
   — it's a search for every other verb whose help has the same
   ambiguity (`audit show`, `audit grep`, etc.) and a fix to all
   of them in the same commit. The same applies to runbook
   recipes, error messages, and clap doc-comments. The trap is a
   symptom of a class of confusion; the class gets the fix, not
   just the instance. Even items not in the currently-tagged
   backlog are in scope — if a v0.1.2-era help string has the
   same trap shape, it gets swept too.

Field-validated examples already in the tree:

- `34ae25d` standardized **five** audit verbs on the L7 envelope
  in one commit after `audit ls` was the first to surface the
  trap. Fix didn't stop at `ls`; it swept `show` / `grep` / `gc` /
  `verify` simultaneously.
- `8aeda74` rewrote both `LONG_GREP` and `LONG_FIND` after the
  agent hit the `--path`/`--name` muscle-memory trap on `find` —
  the same trap was latent on `grep`'s help, so both surfaces
  got the defensive section in one commit.
- `29358d8` attached the new `.data.entry` path documentation to
  `audit show`'s clap doc the moment a single `{id_prefix}`-leak
  smoke turn caught the error template — fixed both the template
  and the discoverability gap together.

When a fix would touch >5 surfaces, *still* do them all in one
commit; don't split. The point is to extinguish the trap class
in one breath, not to amortize it.

### Smoke-test scope discipline

When driving the release smoke (`docs/SMOKE_v0.1.3.md` and
successors), every command must come from the runbook. **No
free-hand exploration until every phase has passed cleanly.**

Reasons:

- The runbook's coverage is the gate; an agent that wanders off
  to "just check X" introduces unscored cycles and loses the
  systematic-coverage signal.
- Side-trips against a real production host carry blast radius
  the runbook is specifically designed to scope. The smoke's
  "all writes label `inspect-smoke=*`" / "all paths under
  `/tmp/inspect-smoke-*`" / "P7 cleanup is idempotent and
  comprehensive" invariants only hold if commands stay
  on-script.
- Finding a class of bugs nobody designed a phase for is a
  legitimate v0.1.5 input, not a v0.1.3 release-gate input.

After a clean P1→P7 PASS, the agent **may** do exploratory free
runs to surface new traps that didn't fire on the runbook
recipes. Those become P8+ candidates for the v0.1.4 / v0.1.5
smoke iteration. Until the in-scope phases are green, the
smoke is the only acceptable workload against the host.
