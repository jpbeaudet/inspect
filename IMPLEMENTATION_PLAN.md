# INSPECT Production Implementation Plan (Phased)

Version: 1.0  
Date: 2026-04-25  
Source of truth: INSPECT_BIBLEv6.2.md

## 1) Delivery Objective

Deliver a production-grade `inspect` CLI as a single static Rust binary that supports:

- Tier 1 direct operational verbs (read + write)
- Tier 2 LogQL search DSL (with reserved labels + `map` extension)
- Tier 3 JSON-first automation composition
- Safe mutation workflow (dry-run default, `--apply`, diff preview, audit log, revert)

The release must satisfy the success criteria and constraints defined in the bible, especially startup/latency goals, secure SSH posture, and cross-environment consistency (laptop, CI, Docker, Codespaces).

## 2) Delivery Model

## 2.1 Cadence

- Sprint length: 2 weeks
- Release branch cut: end of each phase
- Demo + decision gate: each phase exit

## 2.2 Definition of Done (global)

Each phase is complete only when all are true:

- Scope deliverables implemented and documented
- Unit tests for core logic
- Integration tests for CLI behavior and remote execution paths
- Security checks for credential handling and file permissions
- Performance checks against phase-relevant budgets
- Human-friendly output and `--json` schema validated
- Operator docs updated (`inspect help` + phase docs)

## 2.3 Quality Gates (global)

- Build: release binary for linux/darwin x86_64/aarch64
- Reliability: no P0/P1 open defects at phase exit
- Security: no plaintext secret persistence; config/audit/socket mode 600
- Compatibility: CLI behavior stable for completed verbs
- Regression: prior-phase acceptance tests green

## 3) Workstreams (run in parallel)

- Core CLI and command surface (`clap`, routing, UX contract)
- Remote execution and SSH session lifecycle (`openssh` native-mux)
- Discovery/profile and selector resolution
- Read/write verb engines and safety framework
- LogQL parser/planner/execution pipeline
- Output contracts (`SUMMARY/DATA/NEXT` + versioned `--json`)
- Packaging and distribution
- QA, benchmark harnesses, and release governance

## 4) Phase Plan

## Phase 0 - Foundation and Namespace Management

Goal: establish project skeleton and secure namespace credential model.

Scope:

- Rust workspace and crate layout
- CLI bootstrap with command tree placeholders
- Namespace resolver (env + `~/.inspect/servers.toml` precedence)
- `add`, `list`, `remove`, `test`, `show`
- Secure local storage and validation of credentials

Deliverables:

- Compilable CLI binary (`inspect`)
- Namespace config read/write library
- Config schema and migration stubs
- Error taxonomy for setup/connectivity failures

Exit criteria:

- Add/test/show lifecycle works end-to-end
- Env override precedence proven in tests
- Sensitive values redacted in output
- Config file mode enforced to 600

Estimated duration: 1 sprint

## Phase 1 - Persistent SSH and Connection Lifecycle

Goal: production-ready SSH session model with passphrase-once behavior.

Scope:

- `openssh` integration with `native-mux`
- ControlMaster socket management per namespace
- connect/disconnect/connections/disconnect-all
- Resolution order: inspect socket -> existing mux -> agent -> env passphrase -> interactive prompt
- TTL handling and Codespace-aware defaults

Deliverables:

- Connection manager abstraction
- Socket lifecycle and cleanup policies
- Interactive/non-interactive auth behavior

Exit criteria:

- Passphrase requested once per terminal session for active mux
- Existing user ControlMaster sessions reused
- Host key trust behavior delegated to OpenSSH without bypasses
- Connection command suite stable under repeated use

Estimated duration: 1 sprint

## Phase 2 - Discovery Engine and Profile System

Goal: auto-learn server topology and persist profile cache with drift model.

Scope:

- `setup`/`discover` implementation
- Source probes (`docker`, `ss`/`netstat`, `systemctl`, tooling probes)
- Profile YAML schema and cache management
- Async drift checks on command invocation

Deliverables:

- Discovery collector framework with best-effort degradation
- Profile persistence with local edit preservation
- Drift-check scheduler and warning surface

Exit criteria:

- Usable profile generated under 30 seconds in baseline environment
- Missing permissions produce explicit degradations, not silent failure
- Cache TTL and forced rediscovery behavior validated

Estimated duration: 1 sprint

## Phase 3 - Selector and Alias System

Goal: one universal addressing grammar across commands and DSL.

Scope:

- Parse/resolve `<server>/<service>[:<path>]`
- server/service globs, regex, groups, subtraction, `_` host-level
- alias CRUD and persistence (`~/.inspect/aliases.toml`)
- type compatibility checks (verb selector vs LogQL selector aliases)

Deliverables:

- Selector parser + resolver with deterministic precedence
- Friendly no-match diagnostics and suggestions
- Alias expansion engine (pre-parse substitution)

Exit criteria:

- Same selector behavior across at least `status`, `logs`, `grep`, `cat`
- Ambiguous resolution and collisions produce explicit warnings
- Alias misuse returns corrective guidance

Estimated duration: 1 sprint

## Phase 4 - Tier 1 Read Verbs

Goal: high-confidence read operations for common debugging workflows.

Scope:

- `logs`, `grep`, `cat`, `ls`, `find`, `ps`, `status`, `health`
- `volumes`, `images`, `network`, `ports`
- Flag parity (`--since`, `--tail`, `-f`, grep-compatible filters)
- Smart-case behavior and output consistency

Deliverables:

- Read verb dispatcher and per-medium adapters
- Remote tooling fallback (`rg` -> `grep`) with hints
- Structured human output and JSON output for each verb

Exit criteria:

- First-result latency target met for baseline 5-server scenario
- JSON schemas stable and documented
- Exit code contract adhered to (0/1/2 semantics)

Estimated duration: 1 sprint

## Phase 5 - Tier 1 Write Verbs and Safety Contract

Goal: enable production-safe mutation flow with complete auditability.

Scope:

- Write verbs: `restart`, `stop`, `start`, `reload`, `cp`, `edit`, `rm`, `mkdir`, `touch`, `chmod`, `chown`, `exec`
- Dry-run default + `--apply`
- `--diff` for content mutation paths
- Interactive confirms (`--yes`, `--yes-all`) and large-fanout interlock
- Local audit log + snapshot storage + revert command
- Atomic file edit semantics

Deliverables:

- Safety gate middleware used by all mutating verbs
- Diff renderer and preflight mutation plan
- Audit subsystem (`audit ls/show/grep`, `revert`)

Exit criteria:

- Every mutating verb blocked without explicit `--apply`
- Applied mutations always recorded with hashes and metadata
- Failed atomic edit leaves remote file unchanged
- Revert dry-run/apply flow works, including mismatch handling with `--force`

Estimated duration: 2 sprints

## Phase 6 - LogQL Parser and Query Types

Goal: implement standards-faithful LogQL query parsing for log and metric modes.

Scope:

- Parser: hand-written recursive-descent (lexer + parser) over a tokenized stream with explicit byte spans
- Selector unions (`or`), filters, standard stages, field comparisons
- Log-query vs metric-query separation
- Alias substitution before parse

Implementation note - parser strategy:

We deliberately do **not** use a parser-combinator crate (e.g. `chumsky`).
The grammar in bible §9.13 is small, regular, and stable; a hand-written
recursive-descent parser gives us:

- Precise byte-spans on every AST node and every error site, enabling
  rich code-frame diagnostics with caret + hint without an external
  reporter crate.
- Full control over keyword vs identifier disambiguation
  (`or`/`and`/`by`/`without`) and over the log-vs-metric top-level
  decision via finite lookahead.
- Targeted, context-aware error messages. Each call site can attach an
  actionable `hint` (e.g. "label values must be double-quoted, e.g.
  `service=\"api\"`") rather than relying on a generic "expected one
  of \[...\]" combinator output.
- Zero added third-party dependencies on the parsing critical path.

Module layout under `src/logql/`:

- `lexer.rs` - tokenizer with explicit spans (durations, alias refs,
  string escapes, multi-char ops)
- `ast.rs` - typed AST (`Query::{Log,Metric}`, selectors, pipeline
  stages incl. `map`, range/vector aggregations, field-filter
  boolean tree)
- `parser.rs` - recursive-descent parser with finite lookahead
- `alias_subst.rs` - pre-parse `@name` substitution (rejects chaining)
- `validate.rs` - reserved-label semantics (`server`/`service`/`source`)
- `error.rs` - `ParseError { message, span, hint }` with line-numbered
  code-frame rendering

Deliverables:

- AST model and parser error diagnostics (line-numbered code frame +
  caret + actionable hint on every error)
- Grammar compliance tests from canonical examples
- Query type validator and planner input contract

Exit criteria:

- All documented query examples parse correctly
- Invalid queries produce actionable errors with carets and hints
- Log and metric query separation strictly enforced

Estimated duration: 1 sprint

## Phase 7 - Source Readers and `map` Stage Execution

Goal: execute parsed queries across all supported mediums, including cross-medium chaining.

Scope:

- Reader backends: logs, file, dir, discovery, state, volume, image, network, host
- Unified record model with source metadata
- `map` stage with Splunk-style `$field$` interpolation
- Parallel fanout and result merging semantics

Implementation note - execution architecture:

The execution layer lives under `src/exec/` and is wired into the
existing `RemoteRunner` abstraction (Phase 4) so all readers shell out
through one swappable interface. That gives us free CLI-level mockability
via `INSPECT_MOCK_REMOTE_FILE`.

Module layout under `src/exec/`:

- `engine.rs` - top-level entry; alias-expand, parse, dispatch log vs
  metric, run selector union, apply pipeline, truncate to record limit.
- `record.rs` - the unified `Record { labels, fields, line, ts_ms }`
  model. `lookup()` resolves `$field$` and field-filter operands by
  consulting fields first, then labels.
- `medium.rs` - parser for the `source=` label; one `Medium` variant
  per backend, with stable round-tripping and parser/printer tests.
- `reader/{logs,file,dir,discovery,state,volume,image,network,host}.rs`
  one backend per medium, each implementing the `Reader` trait. Logs +
  file readers push line-filters down to remote `grep -F/-E` to reduce
  bytes-on-the-wire. Discovery is the only reader with no remote round
  trip (it materializes records from the cached profile).
- `parsers.rs` - `json`, `logfmt`, `pattern`, `regexp` stage parsers.
- `format.rs` - `{{.name}}` mini-template renderer for `line_format`
  and `label_format`.
- `field_filter.rs` - boolean-tree evaluator for `| status >= 500`,
  with numeric coercion and regex compare.
- `pipeline.rs` - dispatcher for the 10 pipeline stages.
- `map_stage.rs` - `| map { ... }` executor; collects unique-tuple
  parent records (capped by `map_max_fanout`), interpolates `$name$`
  with `"`/`\` escaping, runs the sub-query per tuple, concatenates
  outputs.
- `metric.rs` - range aggregations (`count_over_time`, `rate`,
  `bytes_over_time`, `bytes_rate`, `absent_over_time`) and vector
  aggregations (`sum`, `avg`, `min`, `max`, `count`, `stddev`,
  `stdvar`, `topk`, `bottomk`) with `by`/`without` grouping. Parsed
  fields are promoted to labels for grouping purposes (Loki "parsed
  labels" semantics).

Streaming and ordering:

- Today's executor is **eager fan-out + concatenate**: each selector
  branch resolves its targets, every reader runs to completion, then
  results are unioned and pushed through the pipeline. Ordering is
  branch-major then namespace-major then reader-emission order. There
  is no global timestamp merge yet.
- `--follow` parses but does not yet stream incrementally; truly
  incremental streaming with backpressure is **deferred to Phase 8**,
  where `Reader` will gain a `read_stream` companion to today's
  blocking `read`.
- Safety knobs: `ExecOpts.map_max_fanout` (default 256) caps the
  unique-tuple set the `map` stage forks across, preventing runaway
  sub-query fan-out.

Deliverables:

- Reader trait layer and 9 backend implementations
- `map` executor with `$field$` interpolation, escaping, and fanout cap
- Metric executor (range + vector + topk/bottomk + by/without)
- Stable JSON envelope: `data.kind="log"` with `records[]` carrying
  `_source`, `_medium`, `labels`, `fields`, `line`, `ts_ms`;
  `data.kind="metric"` with `samples[]` carrying `labels` + `value`.
- CLI surface (`commands/search.rs`) wired to the engine, emitting
  SUMMARY/DATA/NEXT for both modes.

Exit criteria:

- Multi-source `or` queries work across mixed mediums (covered in
  `tests/phase7_exec.rs::multi_source_or_mixes_logs_and_file`)
- `map` stage works on unique-label fanout and returns merged outputs
  (`map_stage_runs_subquery_per_unique_field`)
- `count_over_time`, `topk(by)`, parsed-field filters, and discovery
  short-circuit all have explicit integration coverage.
- Streaming behavior documented (above), with the eager-vs-streaming
  trade-off captured here for Phase 8.

Estimated duration: 2 sprints

## Phase 8 - Pushdown, Streaming, and Performance Tuning

Goal: hit operational performance targets with optimized execution.

Scope:

- Filter pushdown to remote commands
- Time range and tail pushdown
- Concurrency controls and backpressure
- Benchmark harnesses (cold start, status latency, search time-to-first-result)

Deliverables:

- Planner optimizations and telemetry-free instrumentation
- Performance regression tests in CI
- Tuned defaults for fanout and retries

### Implementation notes (Phase 8 landed)

**Filter pushdown (bible §9.10).** The engine extracts the leading run
of `Filter` ops from a log query's pipeline (until the first parsing or
formatting `Stage`) and translates each into a reader-level
`LineFilter { negated, regex, pattern }`. Readers that issue remote
commands (`logs`, `file`, `host`) append a shared `grep` chain via
`reader::push_line_filters_grep`:

- `|=` → `| grep -F '<pattern>' || true`
- `!=` → `| grep -v -F '<pattern>' || true`
- `|~` → `| grep -E '<pattern>' || true`
- `!~` → `| grep -v -E '<pattern>' || true`

The `|| true` guard preserves the bible's "no matches is a clean exit"
contract. Pushdown filters are **not** stripped from the in-memory
pipeline — re-applying contains/regex on already-filtered records is
idempotent, and readers that do not honor pushdown (state, image,
network, volume, dir, discovery) fall back to in-memory filtering with
no semantic drift. Pushdown stops at the first `| json`/`| logfmt`/
`line_format` stage because line content is rewritten beyond that
point.

**Time-range and tail pushdown.** `--since`, `--until`, and `--tail`
flow through `ExecOpts → ReadOpts` and are wired in Phase 7 to
`docker logs --since/--until/--tail` and `journalctl --since/--lines`.
File reader honors `--tail` via `tail -n`. No additional Phase 8 work
required here.

**Concurrency.** Branch and step fan-out runs through
`engine::parallel_map`, an `std::thread::scope`-based work queue with
order-preserving slot collection (atomic next-index dispatch + per-slot
`Mutex<Option<Result<R>>>`). Order preservation keeps the existing
multi-source `or` test surface deterministic. The pool size is capped
by `ExecOpts.max_parallel`, which defaults to 8 and is overridable via
`INSPECT_MAX_PARALLEL`. The fast path runs inline when there is one
input or `max_parallel == 1`.

The `Reader` trait is bounded `Send + Sync`; `for_medium_arc` returns
`Arc<dyn Reader + Send + Sync>` so the same reader instance can be
shared across worker threads.

**No new dependencies.** All concurrency uses `std::thread::scope`
plus `std::sync::{Arc, Mutex, atomic}`; all pushdown uses the existing
`shellexpand` and `regex` crates already vendored from earlier phases.

**Streaming deferred.** True incremental `--follow` with backpressure
remains deferred. The current eager fan-out + concatenate model meets
the bible's <2s search-across-5-servers target on mocked I/O (verified
by `tests/phase8_perf.rs::search_across_five_namespaces_first_results_under_2s`).

### Exit-criteria test references

- Pushdown semantics: `tests/phase8_perf.rs::line_filter_pushdown_appends_grep_to_remote_command`,
  `negated_line_filter_uses_grep_minus_v`,
  `regex_line_filter_uses_grep_minus_e`,
  `pushdown_stops_at_first_parsing_stage`.
- Time-range pushdown: `tests/phase8_perf.rs::since_until_tail_get_pushed_to_docker_logs`.
- Parallel correctness: `tests/phase8_perf.rs::parallel_or_query_produces_one_record_per_branch`,
  `max_parallel_env_knob_is_honored`.
- Performance budgets: `tests/phase8_perf.rs::cold_start_version_under_500ms`,
  `search_across_five_namespaces_first_results_under_2s`.
- No regressions: full Phase 0–7 test surface (218 prior tests) continues to pass.

### Bonus fix landed in Phase 8

The engine's `matcher_to_selector_atom` previously produced an invalid
`re:<pattern>` atom for `=~` matchers, which the verb selector parser
rejected with `invalid selector character ':'`. Phase 8 fixes this:
`server=~"..."` now resolves to `*` (and the regex is enforced by
`match_label` post-resolution after a small follow-up still allows
namespace short-circuit), and `service=~"..."` produces the verb
parser's `/.../` regex form. Caught and regression-tested by the new
multi-namespace fan-out tests.

Exit criteria:

- Cold start and status/search targets met or variance documented with mitigation
- No blocking drift checks
- Remote fallback behavior remains correct under missing tools

Estimated duration: 1 sprint

## Phase 9 - Diagnostics and Recipes

Goal: deliver guided diagnostics and repeatable runbooks.

Scope:

- `why` dependency-walk diagnostics
- `connectivity` matrix rendering + optional probes
- Recipe engine (default + user recipes)
- Mutating recipe safeguards (`mutating: true`, dry-run default)

Deliverables:

- Dependency analysis module
- Recipe parser/executor with command sandboxing rules
- Built-in recipe pack

Exit criteria:

- Built-in recipes produce deterministic outputs in fixture environments
- Mutating recipes obey same safety gate as write verbs
- `why` recommendations map to discovered dependency state

Estimated duration: 1 sprint

## Phase 10 - Output Contract and Correlation Layer

Goal: make machine and human output equally reliable and composable.

Scope:

- Enforce `SUMMARY/DATA/NEXT` for every command
- Versioned JSON envelopes for all commands
- Correlation rules (time-clustered errors, dependency cascades, drift signals)

Deliverables:

- Shared output rendering library
- JSON schema docs + validation suite
- Correlation rule registry with cost guards

Exit criteria:

- Any command returns stable JSON envelope with schema version
- Correlation rules only emit when confidence/cost thresholds pass
- Backward-compatibility tests for schema versions

Estimated duration: 1 sprint

## Phase 11 - Fleet Operations

Goal: safe, concurrent multi-namespace operations across verbs.

Scope:

- `fleet` command family
- namespace group support (`~/.inspect/groups.toml`)
- per-namespace credential heterogeneity
- fanout concurrency cap and partial-failure semantics

Deliverables:

- Fleet orchestration layer with target accounting
- Aggregated reporting for success/failure by namespace
- Fleet safety interlock integration

Exit criteria:

- Fleet read/write operations handle mixed namespace health without full abort
- Large fanout safeguards trigger based on total target count
- JSON output includes per-namespace result granularity

Estimated duration: 1 sprint

## Phase 12 - Distribution, Hardening, and GA Release

Goal: production release pipeline and operator-grade packaging.

Scope:

- Release automation (GitHub Releases artifacts)
- `cargo install` publish path
- Homebrew tap and curl installer
- Docker image packaging
- Final docs, quick reference, and upgrade notes

Deliverables:

- Signed release artifacts and checksums
- Installer scripts with rollback-safe behavior
- GA runbook for incident handling and hotfix patching

Exit criteria:

- Install experience validated on linux/darwin targets
- Binary size and static-link constraints met
- GA checklist signed off (security, performance, docs, recoverability)

Estimated duration: 1 sprint

## 5) Test Strategy by Layer

- Unit tests: selector parsing, alias typing, query AST, diff generation, audit serialization
- Integration tests: ephemeral SSH targets, dockerized fixtures, multi-service profiles
- E2E tests: operator workflows (setup -> diagnose -> dry-run fix -> apply -> verify -> revert)
- Chaos tests: partial namespace failure, missing remote tools, flaky network, stale profiles
- Security tests: permission modes, secret redaction, host key handling, no secret logs
- Performance tests: startup latency, time-to-first-result, fanout degradation curves

## 6) Security and Safety Controls

- No secret-at-rest for passphrases
- Strict file permissions (600) for config, sockets, aliases, audit files
- Dry-run default for all mutating commands and mutating recipes
- Interactive confirmation for destructive operations
- Immutable local audit trail with snapshots and hash chain metadata
- Revert safety checks with explicit force on divergence

## 7) Operational Readiness Checklist

Pre-GA checklist:

- On-call runbook for command failures and recovery
- Known limitations documented (v1 out-of-scope boundaries)
- Backward compatibility statement for JSON schema
- Support matrix (OS/arch/container constraints)
- Incident simulation completed for failed production edit and revert

## 8) Program Risks and Mitigations

- SSH edge-case complexity (ProxyJump, host policies): validate early in Phase 1 with representative environments
- Query-engine scope creep: lock grammar to bible and defer enhancements to v2
- Performance regressions under fleet fanout: benchmark gates in CI from Phase 8 onward
- Safety bypass pressure for speed: keep middleware-enforced gate non-optional
- Schema churn affecting automation users: formal versioning and compatibility tests in Phase 10

## 9) Suggested Timeline (14 sprints)

- Sprints 1-2: Phases 0-1
- Sprints 3-4: Phases 2-3
- Sprint 5: Phase 4
- Sprints 6-7: Phase 5
- Sprint 8: Phase 6
- Sprints 9-10: Phase 7
- Sprint 11: Phase 8
- Sprint 12: Phases 9-10
- Sprint 13: Phase 11
- Sprint 14: Phase 12 + GA stabilization

## 10) Immediate Next Actions

- Create issue epics and acceptance-test checklists per phase
- Scaffold CI jobs for unit/integration/performance/security lanes
- Stand up fixture environments for docker + host-level service discovery
- Implement Phase 0 deliverables and gate review template
