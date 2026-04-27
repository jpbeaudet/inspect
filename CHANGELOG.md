# Changelog

All notable changes to `inspect` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.1] — Phase C field-feedback patches

Phase C of `INSPECT_v0.1.1_PATCH_SPEC.md`. Builds on Phases A + B;
same v0.1.1 release.

### Added

- **P4 — Secret masking on `run`/`exec` stdout.** Lines that look like
  `KEY=VALUE` and whose key matches a known secret pattern (suffixes
  `_KEY`, `_SECRET`, `_TOKEN`, `_PASSWORD`, `_PASS`, `_CREDENTIAL(S)`,
  `_APIKEY`, `_AUTH`, `_PRIVATE`, `_ACCESS_KEY`, `_DSN`,
  `_CONNECTION_STRING`; exact: `DATABASE_URL`, `REDIS_URL`, `MONGO_URL`,
  `POSTGRES_URL`, `POSTGRESQL_URL`) are masked to `head4****tail2` by
  default. Values shorter than 8 characters become `****`. The
  `export ` prefix and matching quote pairs are preserved so the
  output remains paste-friendly. Opt-out flags: `--show-secrets`
  (verbatim, also stamps `[secrets_exposed=true]` into the audit
  args on `exec`) and `--redact-all` (mask every KEY=VALUE line, not
  just keys we recognize). When masking actually fired during an
  `exec`, the audit args is stamped with `[secrets_masked=true]` so
  reviewers can tell verbatim runs apart from masked ones.
- **P5 — `--merged` multi-container log view.** `inspect logs <sel>
  --merged` interleaves output from every selected service into a
  single `[svc] <line>`-prefixed stream sorted by RFC3339 timestamp
  (we inject `--timestamps` into the underlying `docker logs`
  invocation). Batch mode does a parallel fan-out via
  `std::thread::scope` then k-way merges the per-source buffers
  through a `BinaryHeap<Reverse<MergeLine>>`. Follow mode pipes every
  stream through a single `mpsc::channel` and prints in arrival
  order. Lines without a parseable timestamp sink below dated lines
  but preserve their per-source order.
- **P9 — Progress spinner on slow log/grep fetches.** Hand-rolled
  100ms-frame Unicode spinner drawn to stderr after a 700ms warm-up.
  Suppressed automatically in JSON mode, when stderr is not a TTY,
  and when `INSPECT_NO_PROGRESS=1` is set (used by CI and the
  acceptance tests). No new dependencies.
- **P11 — Inner exit code surfacing.** `ExitKind::Inner(u8)` is the
  fourth process exit category, alongside `Success`/`NoMatches`/
  `Error`. `inspect run -- 'exit 7'` and `inspect exec --apply --
  'exit 9'` now propagate the remote command's exit code to the
  shell. Multi-target invocations with mixed inner exits still fall
  back to the generic `Error` (=2). High bits of the inner code are
  clamped to the low 8 via `clamp_inner_exit`.
- **P13 — Discovery `docker inspect` per-container fallback.** A
  single wedged container used to take the entire host's
  `inspect setup` down with it: the batched `docker inspect` would
  hit the 30s budget and the whole result was discarded. We now
  budget the batch call at 10s and on failure (timeout, partial JSON,
  daemon hiccup) re-probe each container individually with a 5s
  budget. Containers whose individual probe also fails are recorded
  as a warning AND the corresponding `Service.discovery_incomplete`
  bit is set in the persisted profile so later verbs can detect
  partial data. `inspect setup --retry-failed` re-runs discovery and
  merges in only the previously-incomplete services, leaving the
  rest of the profile cached.

### Acceptance

- New `tests/phase_c_v011.rs`: 6 acceptance tests pinning P4 (Anthropic
  key masking + `--show-secrets` audit breadcrumb), P9 (no spinner in
  JSON mode), P11 (run + exec inner-exit propagation), P13
  (`discovery_incomplete` round-trips through YAML).

## [0.1.1] — Phase B field-feedback patches

Phase B of `INSPECT_v0.1.1_PATCH_SPEC.md`. Builds on Phase A; same
v0.1.1 release. No deprecation paths -- v0.1 is a single-user
pre-release, see the README banner.

### Added

- **P6 — `inspect run <sel> -- <cmd>` (read-only).** New verb for the
  90% case where operators want a one-shot remote command (`ps`,
  `cat /proc/...`, `redis-cli info`) without paying for the
  write-verb interlock. Streams stdout line-by-line via the P1
  streaming primitive. No `--apply`, no audit log, no fanout
  threshold. Accepts `--filter-line-pattern <regex>` for server-side
  pushdown of the same `grep -E` logic logs/grep use.
- **P3 — `--match` / `--exclude` line filters.** Both `inspect logs`
  and `inspect grep` now accept `--match <regex>` (`-g`) and
  `--exclude <regex>` (`-G`), each repeatable. We push them down to
  the remote host as a `grep -E` / `grep -vE` pipeline suffix, with
  `--line-buffered` in `--follow` mode so live streams aren't
  block-buffered behind the filter. Multiple `--match` flags OR
  together (`(?:p1)|(?:p2)`).
- **P10 — `--since-last` resumable cursor.** `inspect logs --since-last`
  and `inspect grep --since-last` resume from the previous run's
  start time, persisted under `~/.inspect/cursors/<ns>/<svc>.kv`
  (mode 0600, dir 0700). Cold-start fallback: `INSPECT_SINCE_LAST_DEFAULT`
  (default `5m`). `--reset-cursor` deletes the file. `--since` and
  `--since-last` are mutually exclusive.
- **P12 — `--reason <text>` on every write verb.** Added to
  `restart`/`stop`/`start`/`reload`, `exec`, `cp`, `edit`, `rm`,
  `mkdir`, `touch`, `chmod`, `chown`. Recorded in the audit log as
  `AuditEntry.reason`, rendered as a trailing column in
  `inspect audit ls` and on its own line in `inspect audit show`.
  `inspect audit ls --reason <substr>` filters case-insensitively.
  240-character cap; oversize values rejected up-front with a clean
  error.

### Changed

- **P7 — `--allow-exec` removed from `inspect exec`.** The double-gate
  rationale dissolved with P6: read-only ad-hoc commands belong to
  `inspect run`, write-y ones belong to `inspect exec --apply`. The
  apply gate still fires; the second confirmation flag is gone.

### Module-level diff

- New: `src/verbs/run.rs`, `src/verbs/line_filter.rs`,
  `src/verbs/cursor.rs`, `tests/phase_b_v011.rs`.
- New paths: `paths::cursors_dir()`, `paths::cursor_file()`.
- `safety::AuditEntry.reason: Option<String>` (serde-skipped when None).
- `safety::validate_reason()` shared validator (≤ 240 chars).

## [0.1.1] — Phase A field-feedback patches

Phase A of `INSPECT_v0.1.1_PATCH_SPEC.md`: three patches that came out
of the first real ~60-call production debugging session.

### Fixed

- **P2 — phantom service names.** Discovery now records the real
  `docker ps` name in `Service.container_name` alongside the
  user-facing `name`. Every `docker logs|exec|restart|stop|start|kill`
  call site uses the container name; selectors and labels keep using
  the friendly name. Eliminates the v0.1.0 footgun where
  `inspect logs arte/api` produced `docker logs api` on a host whose
  actual container was `luminary-api`.
  - Schema: `Service.container_name: String` is **required**. Old
    profiles fail with a clean "run `inspect setup <ns>` to regenerate"
    error.
  - New helper `Step::container()` chooses the right token in one
    place; `cp`, `edit`, `mkdir`, `touch`, `chmod`, `chown`, `rm`,
    `exec`, `logs`, lifecycle (`restart`/`stop`/`start`/`reload`), and
    the lower-level `exec/reader/{logs,file}.rs` all switched.

### Added

- **P1 — streaming `--follow`.** New `ssh::exec::run_remote_streaming`
  pumps stdout line-by-line from the SSH child instead of waiting for
  the command to exit. `inspect logs --follow` now renders every line
  the moment it crosses the wire. The verb wrapper retries the SSH
  call up to three times with 1s/2s/4s backoff so a transient drop
  doesn't end the operator's session; Ctrl-C still cancels promptly.
  The `RemoteRunner` trait grew a default `run_streaming` method so
  mock-backed tests keep working unmodified.
- **P8 — `inspect help <verb>` fallback.** When the named topic has no
  editorial body, the dispatcher falls through to clap's long-help
  renderer for the matching subcommand. Users can type either
  `inspect help logs` or `inspect logs --help` and get help. The
  "did you mean" suggester now also considers verb names, so
  `inspect help serch` hints `did you mean: search?`.

### Tests

- New `tests/phase_a_v011.rs` (6 cases) pins the P2 round-trip and
  P1 streaming wire-up against regressions.
- `tests/help_contract.rs` gained 3 P8 guards including a
  one-test-per-verb fallback assertion.

## [Unreleased]

### Added — documentation

- `docs/MANUAL.md`: end-user manual covering install, concepts, every
  verb, the LogQL DSL, recipes, fleet ops, configuration, and
  troubleshooting. Mirrors the in-binary `inspect help <topic>` content.
- `docs/RELEASING.md`: maintainer notes for cutting a tag, hosting the
  install script, hotfix flow, and updating the Homebrew tap.
- `CONTRIBUTING.md`, `SECURITY.md`: standard public-repo files
  documenting the dev loop, quality gates, and the vulnerability
  disclosure process.
- `archives/README.md`: marks the planning archive as historical and
  points readers at the active docs.

### Changed — documentation

- Root `README.md` rewritten for the public release: clearer pitch,
  table of contents, "How it works" diagram, documentation map, and
  an explicit note that the install URL is served by GitHub directly
  (no separate server to deploy).
- `.gitignore` extended to cover common editor/OS artifacts.

## [0.1.0] — 2026-04-26

First public release.

### Added — capabilities (bible §1)

- Fleet-wide selector grammar: `@alias`, `ns/svc`, regex (`^pulse-.*$`),
  unions (`a,b`), groups (`@storage`), host steps (`_`).
- Read verbs: `ps`, `status`, `health`, `logs`, `cat`, `grep`, `find`,
  `ls`, `network`, `images`, `volumes`, `ports`.
- Write verbs (dry-run by default, `--apply` to enact): `cp`, `edit`,
  `chmod`, `chown`, `mkdir`, `rm`, `touch`, `restart`, `stop`, `start`,
  `exec`. Diff preview, atomic writes, audit trail with snapshot
  rollback.
- LogQL-style query engine: `inspect search '{server="arte"} |= "x"'`
  with stages `json | logfmt | pattern | regexp | line_format |
  label_format | drop | keep | <field op value> | map { ... }`,
  and metric forms `count_over_time`, `rate`, `bytes_over_time`,
  `bytes_rate`, `absent_over_time`, plus vector aggregations
  (`sum`, `avg`, `min`, `max`, `topk`, `bottomk`, `quantile_over_time`)
  with `by`/`without` grouping.
- Discovery + profile cache (`~/.inspect/profiles/<ns>.yaml`, mode 0600,
  TTL 7d). Drift detection with non-blocking probe and `setup --force`
  remediation.
- Recipe system (`inspect recipe <name>`) with builtin and YAML user
  recipes; `--apply` lifts dry-run gates on mutating steps only.
- Help system: in-binary topic catalog (`inspect help <topic>`),
  keyword search (`inspect help search <query>`), pager-aware rendering,
  `--json` machine-readable variant, no-network guarantee.
- Output contract: `--json`, `--jsonl`, `--csv`, `--table`, `--md`,
  `--format` (Go-template), `--raw`. Stable schema versioned in JSON
  envelope (`schema_version`).
- Safety: secret redaction (RFC-style, deterministic), 0600 file modes,
  no secrets-at-rest, SIGINT/SIGTERM-aware cancel with partial-result
  envelope.

### Added — distribution (Phase 12)

- GitHub Actions release workflow producing static-musl Linux
  (`x86_64`, `aarch64`) and Apple Darwin (`x86_64`, `aarch64`) tarballs,
  per-artifact `sha256`, aggregate `SHA256SUMS`, and keyless cosign
  signatures via GitHub OIDC.
- One-shot installer at `scripts/install.sh` with checksum + cosign
  verification, atomic install, and rollback-safe behavior.
- Static musl `Dockerfile` (two-stage build).
- Homebrew formula template at `packaging/homebrew/inspect.rb` (publish
  to a custom tap; not homebrew/core for v0.1.0).
- `cargo install inspect-cli` path (gated behind `vars.PUBLISH_CRATE`).
- CI workflow with fmt + clippy (`-D warnings`) + test on Linux and
  macOS, plus an MSRV (1.75) build job.

### Quality gates locked

- `cargo build` and `cargo test` are warning-free.
- `[lints.rust] dead_code = "deny"` in `Cargo.toml`.
- Contract test `tests/no_dead_code.rs` enforces:
  - H3: every `#[allow(dead_code)]` carries `// v2: <tag>`.
  - H4: zero module-wide `#![allow(dead_code)]`.
  - H5: total surviving suppressions ≤ 1.
- Test count: 488 passing across 18 suites.

### Out of scope for v0.1.0

Items deferred to v2 (tracked in `archives/INSPECT_BIBLEv6.2.md` §27):
TUI mode, k8s discovery, distributed tracing, OS keychain integration,
per-user policies, russh fallback, parameterized aliases, password
auth, remote agents.

[0.1.0]: https://github.com/jpbeaudet/inspect/releases/tag/v0.1.0
