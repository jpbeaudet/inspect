# Changelog

All notable changes to `inspect` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased] — v0.1.3 (in progress)

Closes the v0.1.3 patch backlog (`INSPECT_v0.1.3_BACKLOG.md`). Field
feedback from four independent v0.1.2 users plus a multi-hour
destructive migration session by the primary operator. Implementation
is in progress; this section grows as items land.

### Added

- **F2 — `docker inspect` timeout warning noise eliminated (field-feedback
  regression: two of two v0.1.2 first-time users hit a spurious `warning:`
  on every healthy `inspect setup` against hosts with 30+ containers).**
  The probe now classifies its outcome into one of four buckets — `Clean`,
  `SlowButSuccessful`, `PartialTimeout`, `GenuineFailure` — and routes
  each to the right channel:
  - **Clean** (every container inspected): silent. Healthy hosts no
    longer emit a single `warning:` line on first setup.
  - **SlowButSuccessful** (batch slow, fallback rescued every container):
    debug-level only. Visible under `INSPECT_DEBUG=1` or `RUST_LOG=debug`.
  - **PartialTimeout** (`N` of `M` per-container probes failed): one
    summary line — `warning: docker inspect timed out for N/M containers;
    rerun with --force or check daemon load`. Replaces the old
    one-warning-per-failed-container noise.
  - **GenuineFailure** (zero containers inspected, daemon down): probe
    escalates `ProbeResult.fatal`; engine returns `Err`; setup exits
    non-zero with a chained hint (`inspect run … 'sudo systemctl status
    docker'` → `inspect setup --force`).

  The batched timeout is now scaled with inventory size:
  `timeout = max(10s, 250ms * container_count)` capped at 60s. Operators
  on pathological daemons can pin a fixed budget via
  `INSPECT_DOCKER_INSPECT_TIMEOUT=<seconds>` (verbatim, not re-clipped).

  Three-bucket classification + scaling formula are documented in
  `docs/RUNBOOK.md` §8 ("Probe author checklist") so the next probe
  author follows the same rule. 8 unit tests in `src/discovery/probes.rs`
  pin every contract: empty host, field-scale 37/37 = `Clean`, slow-but-
  successful = no warning, 0/N = `GenuineFailure` with chained hint, 3/50
  = `PartialTimeout` summary line, override bypasses scaling, formula
  floor / scaling / cap.

- **F11 — Universal pre-staged `--revert` on every write verb
  (load-bearing for agentic safety; non-negotiable before v0.2.0
  freezes the audit schema).** Every write verb now captures its
  inverse *before* dispatching the mutation. Each `AuditEntry`
  carries a new `revert: { kind, payload, captured_at, preview }`
  block (plus `applied`, `no_revert_acknowledged`, and
  `auto_revert_of` fields), with four kinds:
  - `command_pair` — single inverse remote command (e.g. `chmod
    0644 …`, `docker start <ctr>`); used by lifecycle stop/start,
    `chmod`, `chown`, `mkdir`, `touch`.
  - `state_snapshot` — restore from snapshot store; used by `cp`,
    `edit`, `rm` (rm now snapshots the target file before deleting
    it).
  - `composite` — ordered list of inverses; reserved for bundle
    integration in F17.
  - `unsupported` — verb has no general inverse this invocation
    (`restart`, `reload`, `exec --no-revert`, legacy v0.1.2 entries
    on read).

  **`inspect exec --apply` now refuses without `--no-revert`** with
  a chained hint pointing at `cp` / `chmod` / `restart`, since
  free-form shell payloads are not generally invertible. Operators
  acknowledge the trade-off explicitly; the audit entry records
  `no_revert_acknowledged: true` so post-hoc readers can tell
  free-form mutations apart from mutations that simply pre-date the
  contract.

  **`inspect revert` upgraded** to dispatch on `revert.kind`:
  `command_pair` runs the captured inverse remote command;
  `state_snapshot` follows the existing snapshot-restore path;
  `unsupported` exits 2 with a chained explanation. New
  `--last [N]` walks the N most recent applied entries in reverse
  chronological order. The `audit-id` argument is now optional (use
  with `--last`). Dry-run output gains a `REVERT:` block showing
  the captured `preview` and the inverse command.

  **`--revert-preview` flag** on every write verb prints the
  captured inverse to stderr before applying, so operators (and
  driving agents) can see exactly what `inspect revert <new-id>`
  will undo.

  **Backward compatibility:** v0.1.2 audit entries (which lack the
  `revert` field) are read as `kind: unsupported` and refuse with a
  loud chained hint pointing at `inspect audit show` rather than
  silently no-opping. Snapshot-style legacy entries (with
  `previous_hash`) still revert through the existing path.

  8 phase_f_v013 acceptance tests cover all four kinds, the
  `--no-revert` refusal, `--revert-preview`, `--last`, and the
  legacy-entry refusal.

- **F9 — `inspect run` forwards local stdin to the remote command.**
  3rd field user (BUG-3 follow-up): `inspect run arte 'docker exec
  -i atlas-pg sh' < ./init.sql` returned `SUMMARY: run: 1 ok, 0
  failed` and exit 0, but no SQL ran — the script's stdin never
  reached the remote `sh`. **Behavior change:** when `inspect run`'s
  own stdin is non-tty (piped or redirected from a file), it is now
  forwarded byte-for-byte to the remote command's stdin and closed
  on EOF, matching native `ssh host cmd <stdin>` semantics. When
  local stdin is a tty, behavior is unchanged from v0.1.2 — no
  forwarding, no hang.

  New flags on `inspect run`:
  - `--no-stdin` refuses to forward; if local stdin has data waiting,
    exits 2 BEFORE dispatching the remote command (never silently
    discards input). With an empty pipe (`< /dev/null`,
    `true | inspect run …`), the run proceeds normally.
  - `--stdin-max <SIZE>` overrides the default 10 MiB cap (`k`/`m`/`g`
    suffixes; `0` disables). Above the cap, exits 2 with a chained
    hint pointing at `inspect cp` for bulk transfer.
  - `--audit-stdin-hash` records `stdin_sha256` (hex SHA-256 of the
    forwarded payload) in the audit entry. Off by default for perf;
    opt-in for security-sensitive runs.

  Audit-log additions: every `inspect run` invocation that forwards
  stdin now writes a one-line audit entry with `verb=run`,
  `stdin_bytes=<N>`, and (with `--audit-stdin-hash`)
  `stdin_sha256=<hex>`. Without forwarded stdin, `inspect run`
  remains un-audited (matches v0.1.2 read-verb behavior). The audit
  schema gains optional `stdin_bytes` (skip-on-zero) and
  `stdin_sha256` (skip-on-none) fields.

  Tests: 8 new acceptance tests in `tests/phase_f_v013.rs` covering
  the field reproducer (byte-for-byte forwarding through a mock
  with `echo_stdin: true`), `--no-stdin` loud-failure with
  pre-dispatch exit, size cap with chained hint, `--stdin-max 0`
  disables the cap, no-piped-input regression guard, `stdin_bytes`
  audit field, `stdin_sha256` audit field, and `--no-stdin` with
  empty pipe being a silent pass.

- **F8 — Cache freshness, runtime-tier cache, `SOURCE:` provenance,
  `inspect cache` verb.** Three field reports converged on the same
  failure mode: `inspect status` happily served pre-mutation data
  for an unbounded window, with no way to ask for fresh data and no
  way to even tell the data was cached. v0.1.3 introduces a tiered
  cache:
  - **inventory tier** (existing): `~/.inspect/profiles/<ns>.yaml`,
    refreshed by `inspect setup`.
  - **runtime tier** (new): `~/.inspect/cache/<ns>/runtime.json`,
    populated by every read verb on a cache miss. Default TTL 10s
    via `INSPECT_RUNTIME_TTL_SECS` (`0` disables the cache, `never`
    sets infinite TTL).
  Every read verb (`status`, `health`, `why`) now:
  - prints a leading `SOURCE: <live|cached|stale> Ns ago …` line in
    human/table/markdown output (omitted for machine formats so
    JSON/CSV/TSV grammar stays clean);
  - carries a stable `meta.source` field on its JSON envelope with
    `mode`, `runtime_age_s`, `inventory_age_s`, `stale`, `reason`;
  - accepts `--refresh` (alias `--live`) to force a live fetch;
  - serves cached data with `mode=stale` and a stderr warning when
    a refresh fails on top of an existing cache, plus a
    `inspect connectivity <ns>` chained hint.
  Mutation verbs (`restart`, `stop`, `start`, `reload`, and
  `bundle apply`) automatically invalidate the runtime cache for
  every namespace they touched, so the next read is guaranteed
  live. `inspect cache show` lists every cached namespace with
  runtime age, inventory age, staleness, refresh count, and
  on-disk size; `inspect cache clear [<ns> | --all]` deletes
  cached snapshots and writes an audit entry per cleared
  namespace. Concurrent refreshers are serialized via a per-
  namespace `flock(2)` advisory lock so two parallel `status`
  calls don't double-fetch. Hot-path correctness is pinned by
  reproducer tests in `tests/phase_f_v013.rs` (cache hit issues
  zero remote commands; refresh count is monotonic; post-mutation
  reads are live; bundle apply invalidates).

### Fixed

- **F1 — `inspect status <ns>` returns 0 services on healthy hosts
  (regression).** Bare-namespace selectors (`inspect status arte`,
  `inspect status prod-*`) were resolving to a single host-level
  step and the status loop, which only renders service steps,
  dropped the host fall-through silently, yielding a misleading
  `0 service(s): 0 healthy, 0 unhealthy, 0 unknown` summary on a
  healthy 30+-container host. Status now rewrites a service-less
  selector to its all-services form (`<sel>/*`) before resolution,
  so `inspect status arte` and `inspect status arte/*` produce the
  same fan-out. Aliases (`@name`) and explicit selectors (with `/`)
  pass through unchanged. Independently confirmed by 2nd and 3rd
  field users; ship-blocker for v0.1.3 under the "one critical
  issue" rule. Regression guards in `tests/phase_f_v013.rs` cover
  1, 10, and multi-namespace cases plus parity with the explicit
  `arte/*` form.

## [0.1.2] — v0.1.2 backlog (B1-B10)

Closes the v0.1.2 backlog (`INSPECT_v0.1.2_BACKLOG.md`). Two new
top-level verbs (`watch`, `bundle`), six refinements to existing
verbs, and a tightened audit schema.

### Added

- **B9 — `inspect bundle plan|apply <file.yaml>`.** YAML-driven
  multi-step orchestration. A bundle declares preflight checks, an
  ordered list of steps (`exec`/`run`/`watch`), per-step rollback
  actions, an optional bundle-level rollback block, and postflight
  checks. Step `on_failure:` routes failures via `abort` (default),
  `continue`, `rollback`, or `rollback_to: <id>`. Steps may use
  `parallel: true` + `matrix:` to fan out across N values with
  bounded concurrency (cap 8). Templating: `{{ vars.x.y }}` and
  `{{ matrix.k }}` interpolate into any string field. Every `exec`
  step (and rollback action) writes one audit entry tagged with a
  bundle-correlation `bundle_id` and the step's `bundle_step`. Run
  `inspect bundle plan` for a no-touch dry-run; `inspect bundle apply
  --apply` to execute. CI mode: `--no-prompt` skips the rollback
  confirmation prompt. First-class checks: `disk_free`,
  `docker_running`, `services_healthy`, `http_ok`, `sql_returns`, plus
  an `exec` escape hatch.

- **B10 — `inspect watch <selector> --until-…`.** Single-target
  block-until-condition verb. Four predicate kinds, mutually
  exclusive: `--until-cmd`, `--until-log`, `--until-sql`,
  `--until-http`. `--until-cmd` accepts comparators `--equals`,
  `--matches`, `--gt`, `--lt`, `--changes`, `--stable-for <DUR>`;
  default is "exit code 0 means match". `--until-http` accepts a DSL
  via `--match` (`status == 200`, `body contains foo`,
  `$.json.path == "x"`). Status line uses TTY in-place rewrite;
  pass `--verbose` (or run non-TTY) for newline-per-poll. Audit
  entry per watch (verb=`watch`). Exit codes: 0 match, 124 timeout,
  130 cancelled, 2 error.

- **B5 — `inspect search` cross-service grep.** Single command that
  greps a pattern across logs and configs in selected containers and
  reports a per-service summary plus highlighted matches. Pushes the
  regex down to `grep -E` server-side and respects `--since`.

- **B6 — Selectors carry container kind.** `ns/svc:logs` and
  `ns/svc:config` selector suffixes route the same verb (e.g. `grep`)
  to the correct file class without per-verb flags.

- **B3 — Configurable per-line byte cap with `--no-truncate`.** Run /
  exec / search default to a 4 KiB per-line cap (sanitized for
  ANSI/C0). `--no-truncate` lifts the cap. Truncation marker now
  shows the byte count.

- **B2 — Server-side line filter pushdown.** `--filter-line-pattern`
  (alias `--match`) pushes through `grep -E` on the remote so
  irrelevant lines never cross the wire.

- **B1 — Streaming captured stdout.** `inspect exec` now streams
  output live AND captures it for audit; the audit `args` field
  reflects exactly what the operator saw.

- **B4 — `--reason` audit ergonomics.** Reason is validated up-front
  (240-char cap), echoed to stderr at run start, and stored in the
  audit log. `inspect audit ls --reason <substring>` filters by it.

- **B8 — `--no-truncate` propagation.** Threaded through `run`,
  `exec`, and `search` consistently.

- **B7 — `inspect run` exit code propagation.** Inner exit codes
  surface through `inspect run`/`exec` (clamped to 8 bits) so shell
  scripts can branch on them.

### Changed

- **Audit schema** carries two optional fields: `bundle_id` and
  `bundle_step`. Backward-compatible — entries written by 0.1.1 and
  earlier parse and render unchanged.

- **`inspect audit ls`** gains `--bundle <id>` to filter to a single
  bundle invocation.

### Defensive hardening (post-implementation audit pass)

A code-reality audit of the whole tree (independent of phase
numbering) surfaced and fixed five hardening items before release:

- **`bundle/exec.rs::run_parallel_matrix`**: matrix workers now run
  inside `std::panic::catch_unwind`. A panic inside a single branch
  converts to a normal failure recorded in `first_err` with
  `stop_flag` set, so rollback fires correctly instead of the panic
  unwinding the scope and bypassing rollback semantics.
- **`bundle/exec.rs`** (4 sites): replaced `.expect()`/`.unwrap()`
  on validated invariants (matrix presence, watch body presence)
  with `?` returning `anyhow!()` errors. Defense in depth — if
  validation ever regresses, the operator sees a clean error
  instead of a panic.
- **`safety/audit.rs::append`**: append now calls `f.sync_data()`
  after `flush()`. Audit entries survive power loss on conformant
  filesystems. Best-effort: warns and continues on filesystems that
  don't implement `fsync` (some FUSE/network mounts) rather than
  refusing to write the record.
- **`bundle/checks.rs::http_ok`**: `curl` invocation gains
  `--connect-timeout 5 --max-time 15`. A stuck HTTP endpoint can no
  longer pin SSH for the full per-check timeout budget.
- **`verbs/watch.rs::probe_http`** + `WatchArgs.insecure` +
  `WatchStep.insecure`: same `curl` timeout guards, plus an opt-in
  `--insecure` flag for self-signed staging endpoints. Disabled by
  default; documented as not-for-production.

### Notes

- New CLI surface: `inspect watch --help`, `inspect bundle --help`,
  `inspect bundle plan --help`, `inspect bundle apply --help`.
  All carry the canonical `See also: inspect help …` footer.
- 27 test suites, 555+ tests. CI gates (`cargo fmt --all -- --check`,
  `cargo build --locked`, `cargo test --locked`) all green. Clippy
  reports only the pre-existing `result_large_err` advisories,
  which are intentional for an SRE tool that maps services and
  errors as first-class values.

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
