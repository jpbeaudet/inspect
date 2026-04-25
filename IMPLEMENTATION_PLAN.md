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

- Parser with `chumsky`
- Selector unions (`or`), filters, standard stages, field comparisons
- Log-query vs metric-query separation
- Alias substitution before parse

Deliverables:

- AST model and parser error diagnostics
- Grammar compliance tests from canonical examples
- Query type validator and planner input contract

Exit criteria:

- All documented query examples parse correctly
- Invalid queries produce actionable errors
- Log and metric query separation strictly enforced

Estimated duration: 1 sprint

## Phase 7 - Source Readers and `map` Stage Execution

Goal: execute parsed queries across all supported mediums, including cross-medium chaining.

Scope:

- Reader backends: logs, file, dir, discovery, state, volume, image, network, host
- Unified record model with source metadata
- `map` stage with Splunk-style `$field$` interpolation
- Parallel fanout and result merging semantics

Deliverables:

- Reader trait layer and backend implementations
- `map` executor with safety limits and diagnostics
- Streaming merger with stable JSON schema

Exit criteria:

- Multi-source `or` queries work across mixed mediums
- `map` stage works on unique-label fanout and returns merged outputs
- Streaming behavior documented, including ordering caveats

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
