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

- **L5 — `inspect audit gc` retention + orphan-snapshot sweep, plus
  optional lazy GC trigger via `[audit] retention` in
  `~/.inspect/config.toml`.** v0.1.2 retrospective: `~/.inspect/audit/`
  grew unbounded; a team running 50 bundle ops per day would
  accumulate years of JSONL plus orphaned snapshot directories with no
  cleanup verb and no retention policy. After F11/F14/F15/F16/F17 each
  added new audit fields and (in F17's case) per-(step, target) and
  per-revert entries that multiply the per-mutation footprint by an
  order of magnitude, the maintenance gap had to close before the
  next field rollout.
  - **New `inspect audit gc --keep <X>` subcommand.** `<X>` accepts
    duration suffixes `90d` / `4w` / `12h` / `15m` (days, weeks,
    hours, minutes) or a bare integer (newest N entries kept *per
    namespace*, where namespace is parsed from the entry's
    `selector` field — `arte/atlas-vault` → `arte`; entries with no
    namespace prefix group under the sentinel `_`). `--dry-run`
    previews counts and freed bytes without touching the
    filesystem; the same code path computes the deletion set so
    the dry-run report is byte-for-byte what an `--apply` run
    would produce. `--keep 0` is rejected loudly with a chained
    hint — refusing to silently delete every entry is the only
    safe default.
  - **Stable `--json` envelope.** Fields: `dry_run`, `policy`
    (canonical formatted form: `90d`, `4w`, `12h`, `15m`, or the
    integer count), `entries_total`, `entries_kept`,
    `deleted_entries`, `deleted_snapshots`, `freed_bytes`,
    `deleted_ids` (full list, not truncated), and
    `deleted_snapshot_hashes` (without the `sha256-` prefix). The
    envelope sits at top level (no `data` wrapper) and includes
    the standard `_source: "audit.gc"` / `_medium: "audit"` /
    `server: "local"` discriminators so a fleet of GCs can be
    streamed through one log pipeline.
  - **Snapshot orphan sweep.** Walks
    `~/.inspect/audit/snapshots/sha256-<hex>` and deletes any file
    whose hash is not referenced by a *retained* audit entry.
    "Referenced" means: `previous_hash`, `new_hash`, the file-name
    portion of `snapshot`, and — critically for v0.1.3 — the
    `revert.payload` of a `state_snapshot` revert AND nested
    `state_snapshot` payloads inside a `composite` revert (F17's
    parent `run --steps` entries store per-step inverses as a
    JSON array; the GC must recurse into that array or it would
    treat F17 step snapshots as orphans). A snapshot pinned by
    any retained entry is **never** deleted — that is the F11
    revert contract and the GC enforces it as the only invariant
    that cannot be relaxed by config. Pre-existing acceptance test
    `l5_gc_keeps_snapshot_referenced_by_retained_entry` is the
    headline regression guard.
  - **Atomic JSONL rewrite.** Each affected JSONL file is
    rewritten via `tmp.gctmp.<pid>` → `rename(2)` (mode 0600 from
    the start on unix). A file whose entries are *all* deleted is
    `unlink(2)`-ed entirely — the GC never leaves zero-byte
    JSONLs lying around. `freed_bytes` covers BOTH JSONL
    shrinkage AND snapshot file sizes, so an operator can size
    their next retention window from the report alone.
  - **`[audit] retention` config block.** New
    `~/.inspect/config.toml` with `[audit] retention = "90d"`
    (or any value `--keep` accepts) opts an installation in to
    automatic lazy GC. The trigger fires on every successful
    `AuditStore::append` — i.e. every write verb that produced an
    audit record — guarded by a once-per-minute cheap-path
    marker (`~/.inspect/audit/.gc-checked`). Within the cheap
    path: only the oldest JSONL file's mtime is checked against
    the retention threshold; if it is fresher, the GC no-ops
    immediately (no full FS scan). Count-based policies always
    run a full pass per check window since mtime alone cannot
    decide them. Errors from lazy GC are deliberately swallowed
    so a transient GC failure can never break the just-appended
    audit record (the `let _ = ...maybe_run_lazy_gc()` call site
    in `AuditStore::append`).
  - **`~/.inspect/config.toml` is a fresh global-policy file.**
    Distinct from `servers.toml` (per-namespace runtime config) —
    L5 is the first item to land it; future cross-cutting
    behavior toggles (cache TTLs, history rotation) can plug in
    here without polluting per-server schema. Missing file is
    not an error: it deserializes to `GlobalConfig::default()`
    and lazy GC stays off.
  - **Help-text discoverability.** `inspect audit --help` gains
    a `GC + RETENTION (L5, v0.1.3)` section listing `--keep`
    syntax, the `[audit] retention` config hook, and the
    cheap-path-marker semantics. `inspect audit gc --help`
    documents `--keep`, `--dry-run`, and the full `FormatArgs`
    flags.
  - **11 acceptance tests in `tests/phase_f_v013.rs::l5_*`** plus
    9 unit tests in `safety::gc::tests` and 3 in
    `config::global::tests`: dry-run-doesn't-delete, apply
    deletes old entries + orphan snapshots, retained-entry
    snapshots are NEVER deleted (the F11 contract guard), JSON
    envelope schema, count-policy keeps newest per namespace,
    invalid `--keep` value chains a hint, `--keep 0` rejected,
    `--help` documents the contract surface, empty audit dir
    yields zero counts, lazy GC marker prevents double-scan
    within a minute, lazy GC fires on next audit append when the
    oldest file's mtime crosses the threshold (uses
    `std::fs::File::set_modified` to backdate the seed file).

- **F17 — `inspect run --steps <file.json>` multi-step runner with
  per-step exit codes, structured per-step output, F11 composite
  revert, and `--revert-on-failure` auto-unwind (migration-operator
  field feedback: *"When I run a 5-step heredoc, all 5 steps are
  one 'run' with one exit code. If step 3 fails I see it in the
  output but the SUMMARY still says `1 ok` because the **outer
  ssh** succeeded. That made me build defensive `set +e; … || echo
  MARKER` patterns. A `--steps` mode that took an array of commands
  and returned per-step exit codes would be amazing for migration
  scripts."*).** Promotes the defensive `set +e; … || echo MARKER`
  pattern from a widespread workaround to a first-class verb mode
  with **structured per-step output that an LLM-driven wrapper can
  reason about step-by-step**. Without F17, agentic callers cannot
  reliably build "run these N steps, stop on first failure, give
  me the per-step result table" workflows on top of `inspect run`
  — the outer `bash -c`'s exit code masks per-step failures, and
  the SUMMARY trailer says `1 ok` even when step 3 of 5 failed.
  - **`--steps <PATH>` flag on `inspect run`.** PATH is a JSON
    manifest file (or `-` for stdin). Each manifest step has
    `name` (required, unique), `cmd` (required unless `cmd_file`
    is set), `cmd_file` (alternative — local script path shipped
    via `bash -s`; F14 composition), `on_failure` (`"stop"`
    default | `"continue"`), optional `timeout_s` (per-step
    wall-clock cap, default 8 hours), optional `revert_cmd`
    (declared inverse for the F11 composite revert; absent ⇒
    `revert.kind = "unsupported"` for that step).
  - **Per-step structured output.** Human format: one
    `STEP <name> ▶` / `STEP <name> ◀ exit=N duration=Ms` block
    per step, with the existing `<ns> | …` line-prefixing inside
    each block, then a STEPS summary table with ✓/✗/⏱/· markers
    and a count line `STEPS: N total, K ok, M failed, S skipped`.
    JSON format (`--json`): one structured object containing
    `steps: [{name, cmd, exit, duration_ms, stdout, stderr,
    status: "ok"|"failed"|"skipped"|"timeout", audit_id}]` plus
    `summary: {total, ok, failed, skipped, stopped_at,
    auto_revert_count}` plus `manifest_sha256` + `steps_run_id`
    + `verb: "run.steps"`. **This is the contract LLM-driven
    wrappers can reason about** — no prose parsing, no defensive
    markers.
  - **`AuditEntry` gains four F17 fields.** `steps_run_id:
    Option<String>` links every per-step entry plus the parent
    composite entry (same UUID-shaped id, in `<ms>-<4hex>`
    format matching the rest of the audit log); `step_name:
    Option<String>` is stamped on per-step entries only;
    `manifest_sha256: Option<String>` is the canonical
    sha256 of the JSON manifest body, stamped on the parent only;
    `manifest_steps: Option<Vec<String>>` is the ordered name
    list, stamped on the parent only. All `Option<T>` with
    `skip_serializing_if` so pre-F17 entries deserialize
    unchanged. Plus a new `Revert::composite(payload_json,
    preview)` constructor since F11's composite variant was
    declared but had no constructor; `RevertKind::Composite`
    payload shape is now nailed down: a JSON-encoded ordered
    list of `{step_name, kind, payload}` records executed in
    reverse order.
  - **F11 composite revert (`inspect revert <steps_run_id>`).**
    `src/commands/revert.rs` gains `revert_composite()` (replaces
    the v0.1.3-as-of-F11 "not yet implemented" stub) — walks the
    parent's composite payload list in reverse manifest order,
    dispatching each `command_pair` inverse against the same
    selector as the original `--steps` invocation and writing one
    `run.step.revert` audit entry per inverse, all linked back to
    the parent via `reverts: <parent-id>`. Items with `kind:
    "unsupported"` (steps with no declared `revert_cmd`) are
    skipped without aborting the unwind. Dry-run by default;
    `--apply` executes; the dry-run preview lists the inverses in
    reverse-manifest order so the operator sees exactly what
    `--apply` will run.
  - **`--revert-on-failure` flag (requires `--steps`).** When a
    step fails with `on_failure: "stop"`, the runner walks the
    inverses of the steps that already ran in reverse manifest
    order **in the same invocation** and dispatches each as its
    own audit-logged auto-revert entry stamped with
    `auto_revert_of: <original-step-id>`. Exactly the
    migration-operator's missing primitive: a 5-step manifest
    where step 3 fails with `--revert-on-failure` correctly
    unwinds steps 1 and 2 without a separate `inspect revert`
    invocation.
  - **F14 composition via `cmd_file`.** A step can declare
    `"cmd_file": "./step3.sh"` instead of `"cmd": "..."`; the
    runner reads the local file, ships its body via `bash -s`,
    and stamps `script_sha256` + `script_bytes` + `script_path`
    on the per-step audit entry — the same fields F14 stamps for
    `inspect run --file`. Lets multi-step migrations whose
    individual steps are non-trivial scripts compose F14 + F17
    cleanly without the operator hand-rolling a wrapper.
  - **F12 / F13 composition.** Each step inherits the namespace
    env overlay (F12) automatically — overlays validated once
    before the per-step loop so a typo short-circuits the whole
    run. Stale-socket failures during a step trigger F13's
    auto-reauth path identically to bare `inspect run` (the
    composite payload is only walked after every step completes,
    so a transparent reauth mid-pipeline is invisible at the F17
    layer).
  - **Single-target requirement (v0.1.3 scope).** The selector
    must resolve to exactly one target; fanout selectors exit 2
    with a chained hint pointing at single-host narrowing.
    Multi-host fanout is deferred to v0.1.5 since the per-step
    audit-link semantics get murky once N hosts each produce
    their own per-step entries.
  - **YAML input (`--steps-yaml <PATH>`).** Same manifest schema
    as `--steps`, just YAML-encoded — convenient for operators
    who maintain migration manifests alongside CI/CD pipelines.
    Mutex with `--steps`. Both flags participate in a clap
    `manifest_source` ArgGroup so `--revert-on-failure` accepts
    either spelling.
  - **`--steps --stream` per-step PTY allocation.** Forwards
    `args.stream` into each per-(step, target) `RunOpts` via the
    F16 `with_tty(true)` builder. The remote process for each
    step line-buffers (real-time output instead of 4 KB bursts)
    and Ctrl-C propagates through the PTY layer to the active
    step's remote process group. Per-step audit entries record
    `streamed: true`; the parent `run.steps` entry records
    `streamed: true` once for the whole pipeline so post-hoc
    audit can tell streaming-mode pipelines apart from buffered
    ones.
  - **Multi-target fanout.** When the selector resolves to N>1
    targets, each manifest step fans out across all N targets
    sequentially within the step. Per-step aggregate `status` is
    `ok` only when every target succeeded; `failed` when any
    target's exit was non-zero; `timeout` when any target overran
    `timeout_s`. `on_failure: "stop"` applies globally — any
    target's failure aborts the next manifest step on every
    target. Each (step, target) pair writes its own `run.step`
    audit entry with the target's label as the entry's
    `selector`; `--revert-on-failure` fans the inverse out
    across every target the step ran on. The JSON output's per-
    step record carries a `targets[]` array of per-target results
    (`label`, `exit`, `duration_ms`, `stdout`, `stderr`,
    `output_truncated`, `status`, `audit_id`, `retried`); the
    summary's new `target_count` field exposes N. Multi-target is
    sequential within each step (parallel fan-out is intentionally
    out of scope for v0.1.3 — output interleaving + audit-link-
    ordering races would need a separate design pass).
  - **F13 stale-session auto-reauth per (step, target).** Each
    per-target dispatch is wrapped in `dispatch_with_reauth`, so
    a stale-socket failure mid-pipeline triggers the F13
    transparent reauth + retry path on that exact (step, target)
    pair without aborting the rest of the pipeline. Retried
    entries stamp `retry_of` + `reauth_id` for cross-correlation
    with the inserted `connect.reauth` entry; the per-target
    result's `retried: true` flag surfaces in the JSON output.
  - **Per-step output cap (10 MiB per (step, target)).** Live
    printing is unaffected (the operator always sees every line);
    the captured copy that feeds the audit + JSON output stops
    growing past 10 MiB and stamps `output_truncated: true` on
    the per-target result. Protects the local process from OOM
    on a step that emits many GB. Cap matches the F9 `--stdin-max`
    default for consistency.
  - **`--reason` plumb-through.** `inspect run --steps --reason
    "JIRA-1234 atlas vault migration"` echoes the reason to
    stderr (matches bare `inspect run` semantics) **and** stamps
    it onto the parent `run.steps` audit entry's `reason` field
    so a 4-hour migration's operator intent is recoverable from
    the audit log alone, no terminal scrollback required.
  - **Out of scope for v0.1.3.** Parallel multi-target fan-out
    within a single step (sequential is shipped; parallel is
    deferred because output interleaving + audit-link-ordering
    races need a separate design pass).
  - **Test coverage.** 23 acceptance tests in
    `tests/phase_f_v013.rs` (`f17_*`) covering: 3-step
    stop-on-failure produces the correct STEPS table + audit
    shape with `steps_run_id` linkage; `on_failure=continue` runs
    every step; `--json` output matches the documented schema
    (per-step records with `targets[]` array + summary +
    `manifest_sha256` + `target_count`); `--revert-on-failure`
    walks inverses in reverse manifest order with `auto_revert_of`
    linkage; unsupported steps are skipped during auto-revert
    rather than aborting; `--steps` + `--file` clap mutex,
    `--steps` + `--steps-yaml` mutex; `--revert-on-failure`
    requires either manifest source via the `manifest_source`
    ArgGroup (accepts `--steps-yaml` too); `inspect revert
    <parent-id>` walks the composite payload in reverse (dry-run
    preview ordering verified); `cmd_file` F14 composition stamps
    `script_sha256` on the per-step entry; YAML manifests parse
    and dispatch identically; `--steps --stream` records
    `streamed: true` on every per-step + parent entry;
    `--reason` is echoed to stderr AND stamped on the parent
    audit; per-step output cap leaves small payloads
    untruncated; per-step `timeout_s` is accepted and reflected
    in audit; F13 mid-pipeline reauth fires on a stale-socket
    failure between steps and the pipeline continues; multi-
    target fanout writes one entry per (step, target) all sharing
    `steps_run_id`; multi-target step status aggregates to
    `failed` when any target fails; multi-target
    `--revert-on-failure` unwinds per-target; help-text gate
    enforces that `--steps`, `--steps-yaml`, and
    `--revert-on-failure` appear in `inspect run --help`;
    invalid manifest exits 2 with a JSON-mentioning error.
    Plus 8 unit tests in `src/verbs/steps.rs::tests` for the
    manifest parser + validator (JSON + YAML round-trip, empty
    manifest, duplicate names, neither cmd nor cmd_file, both
    cmd and cmd_file, revert_cmd round-trip, timeout round-trip).

- **F16 — `inspect run --stream` / `--follow` line-streaming for
  long-running remote commands (migration-operator field feedback:
  *"`docker compose up -d --force-recreate aware aware-embedder
  2>&1 | tail -30` is fine, but I'd kill for `inspect run --stream
  arte -- 'docker logs -f aware'` that line-streams back to the
  client until I Ctrl-C. I worked around with `inspect logs
  arte/<service>` (which exists) but it's a separate command shape
  and doesn't compose with arbitrary scripts."*).** Adds a
  `--stream` flag (alias `--follow`) on `inspect run` for the long
  tail of non-logs commands that produce output indefinitely until
  SIGINT (`tail -f /var/log/syslog`, `journalctl -fu vault`,
  `python -m monitor`, etc.). Pre-F16, every such command either
  buffered until exit (silent until the operator gave up and
  Ctrl-C'd the local `inspect`, which often killed the process
  locally without notifying the remote) or worked only by accident
  if the remote happened to flush eagerly. F16 wires the existing
  line-streaming SSH executor (already used by `inspect logs
  --follow`) into the bare `inspect run` path, plus the SSH PTY
  trick that makes the remote process line-buffer and propagates
  Ctrl-C end-to-end.
  - **`--stream` / `--follow` flag on `inspect run`.** Forces SSH
    PTY allocation (`ssh -tt`) on the dispatch. Two effects flow
    from the PTY: (1) the remote process flips from block-buffered
    to line-buffered output, so lines arrive locally in real time
    instead of in 4 KB bursts; (2) local Ctrl-C propagates through
    the PTY layer to the remote process, so the command actually
    dies instead of being orphaned on the remote host. The
    `<ns> | …` line-prefixing is unchanged from the existing
    streaming path (`inspect logs --follow` already used the same
    `run_streaming` path); the only new behavior is the `-tt`
    flip and the audit-field stamp.
  - **Default timeout bumped to 8 hours under `--stream`.**
    Streaming runs are expected to terminate via Ctrl-C, not by
    reaching the per-target timeout. The non-streaming default
    stays at 120 s; both can be overridden with
    `--timeout-secs <N>`. The 8 h matches the existing
    `inspect logs --follow` default so operators do not have to
    learn two timeout regimes.
  - **`RunOpts.tty: bool` builder hook in
    `src/ssh/exec.rs`.** New field on the executor's per-call
    options struct, threaded through all three SSH dispatch paths
    (`run_remote`, `run_remote_streaming`,
    `run_remote_streaming_capturing`). Off by default for
    non-streaming runs because PTY allocation can change command
    behaviour (CRLF endings, color output, prompt suppression);
    `--stream` is the only call site that flips it today. Builder
    style: `RunOpts::with_timeout(secs).with_tty(true)`.
  - **`AuditEntry.streamed: bool` field (audit-schema, behavior
    flag).** New field, `Option<T>`-shaped via
    `skip_serializing_if = "is_false"` so pre-F16 entries
    deserialize unchanged. Stamped `true` on every `--stream` /
    `--follow` invocation and absent otherwise. Recorded so
    post-hoc audit can tell `tail -f`-shaped invocations apart
    from short-lived commands in the same audit log without
    parsing the args text — the same separation the F15
    `transfer_direction` field provides for uploads vs downloads.
  - **`inspect run` is now audited on every `--stream`
    invocation.** Pre-F16 the verb was un-audited unless stdin
    was forwarded (F9) or the dispatch wrapper retried under F13.
    `--stream` joins those triggers: every streaming run produces
    an audit entry with `streamed: true`, `failure_class`, the
    rendered command, and the wall-clock duration, so a
    multi-hour migration's `--stream` blocks are recoverable from
    the audit log alone.
  - **Mutex with `--stdin-script`** (clap-enforced). Streaming a
    script body over local stdin while also streaming output back
    is a half-duplex protocol headache deferred to v0.1.5;
    `--stream --file <script>` is fine (the body is delivered in
    one shot, then the running script's output streams back).
    `--stream --stdin-script` exits 2 with a clean clap message
    naming both flags.
  - **`inspect logs` interop (no overlap).** F16 does **not**
    replace `inspect logs --follow`; the dedicated logs verb keeps
    its existing semantics (selector-aware, source-tier-aware,
    structured). F16 is for the case when the operator wants
    streaming for a non-logs command. The `inspect logs`
    discoverability hint added in F10.6 remains the canonical
    pointer for log tailing.
  - **First-Ctrl-C → SIGINT-via-PTY, second-Ctrl-C-within-1s →
    channel-close-SIGHUP escalation (added late in v0.1.3).**
    The streaming SSH executor now writes the ASCII INTR byte
    (`\x03`) into the SSH stdin pipe on the first Ctrl-C — the
    remote PTY's terminal driver sees ETX and delivers SIGINT to
    the remote process group, which is the OpenSSH-idiomatic way
    to forward SIGINT through a PTY (no reliance on the unreliable
    SSH `signal` channel-request protocol message). The local
    cancel flag is then cleared so the verb surfaces the remote's
    real exit code (matches the field-validation gate's
    "Ctrl-C terminates `docker logs -f`, exit code is the
    docker-logs exit code"). On the second Ctrl-C within 1
    second, the executor escalates to channel close (`child.kill()`
    on the local SSH process), which triggers the remote sshd to
    deliver SIGHUP to the remote process group via PTY teardown
    — covering the corner case of a remote process that ignores
    SIGINT but exits on SIGHUP. Counter-based detection in
    `src/exec/cancel.rs` (new `signal_count()` + `reset_cancel_flag()`
    APIs) so a third signal cannot be lost between polls.

- **F16 follow-up — SIGHUP escalation in the streaming SSH
  executor.** First-Ctrl-C → SIGINT-via-PTY (write `\x03` into
  the remote PTY's terminal driver, which delivers SIGINT to the
  remote process group); second-Ctrl-C-within-1s → channel close
  (kill the local SSH child, which triggers sshd to deliver
  SIGHUP to the remote process group via PTY teardown). New
  signal-count API in `src/exec/cancel.rs` (`signal_count()`,
  `reset_cancel_flag()`) so the streaming executor can detect a
  *new* signal between polls without losing the trip. The local
  SSH child's stdin is now `Stdio::piped()` (instead of
  `Stdio::null()`) when `opts.tty` is set so we have a write
  handle for the `\x03`. Test coverage: 2 new unit tests in
  `src/exec/cancel.rs::tests` (`signal_count_increments_per_cancel`,
  `reset_cancel_flag_clears_only_the_flag`); the real-SSH SIGHUP
  escalation behaviour is exercised by the field-validation gate
  (a remote command that ignores SIGINT but exits on SIGHUP
  receives SIGHUP via channel close on the second Ctrl-C within
  1 second).
  - **Test coverage.** 6 acceptance tests in
    `tests/phase_f_v013.rs` (`f16_*`) covering: `--stream`
    records `streamed: true` on success, `--follow` alias is
    accepted and produces the same audit shape, non-streaming
    runs omit the `streamed` field entirely (the F9 audit path
    is exercised to prove this), `--stream` on a failing command
    still records `streamed: true` + `failure_class:
    "command_failed"`, `--stream --stdin-script` is clap-rejected
    with both flag names in the error, and `inspect run --help`
    documents `--stream`, the `--follow` alias, and the PTY
    (`-tt`) mechanism. Real-SSH SIGINT propagation and the
    line-by-line *timing* of the flush are exercised by the
    field-validation gate (the migration-operator's destructive-
    migration smoke test) since the in-process mock medium
    cannot model PTY semantics.

- **F15 — `inspect put` / `inspect get` / `inspect cp` native file
  transfer over the persistent ControlPath master (migration-operator
  field feedback: *"I had to context-switch between
  `inspect run arte -- 'cat > /tmp/x.sh'` (works for tiny payloads,
  breaks on quoting) vs `scp /tmp/x ...:arte:...` (works but bypasses
  the inspect connection). For a tool whose job is 'be the way I
  touch this server,' not having `inspect put`/`inspect get` is a
  notable hole. Especially during compose-file rewrites."*).**
  Replaces the v0.1.2 base64-in-argv `cp` implementation (4 MiB
  cap, audit-thin) with a streaming-stdin pipeline that has no
  fixed size cap, captures pre-existing remote content as
  `revert.kind = state_snapshot` (or a delete inverse for
  brand-new files), and records direction / bytes / sha256 in the
  audit log on every transfer. Rides the same multiplexed SSH
  master used by every other namespace verb, so it inherits F11
  revert capture, F12 env overlay, F13 stale-session auto-reauth.
  - **`inspect put <local> <selector>:<path>`** — uploads via
    `cat > <tmp>; chmod/chown --reference; mv <tmp> <path>` with
    the local body streamed through SSH stdin (F9). Container
    targets dispatch via `docker exec -i <ctr> sh -c '...'`.
    Captures the prior remote file content as `revert.kind =
    state_snapshot` so `inspect revert <id>` restores byte-for-byte;
    when the target did not exist, captures a `command_pair` rm
    inverse so revert deletes the freshly-created file.
  - **`inspect get <selector>:<path> <local>`** — downloads via
    `base64 -- <path>` (binary-safe over the SSH text pipe) and
    decodes locally. `<local>` of `-` writes to stdout for piping.
    `inspect get` is read-only on the remote, so `revert.kind` is
    `unsupported` (the operator deletes the local file to undo);
    the audit entry still records `transfer_bytes` +
    `transfer_sha256` so a later `put` of the same content is
    verifiable byte-for-byte from the audit log.
  - **`inspect cp <source> <dest>`** — bidirectional convenience.
    Inspects arg shape and routes to `put` (push) or `get` (pull)
    based on which side carries the `<selector>:<path>`. Operator
    types `cp`, audit records the canonical verb (`put` or `get`).
    The pre-F15 base64-in-argv backend is removed; the 4 MiB hard
    cap is gone with it.
  - **Selector forms.** Both verbs accept the existing selector
    grammar: `<ns>/_:/path` (host filesystem; F7.2 shorthand
    `<ns>:/path` resolves to host), and `<ns>/<svc>:/path`
    (container filesystem, dispatched via `docker exec -i`).
    Host vs container is decided unambiguously by the selector,
    never by a flag.
  - **Flags on `put` / `cp`.** `--mode <octal>` chmod the remote
    after upload; `--owner <user[:group]>` chown the remote after
    upload (requires the SSH user have permission); `--mkdir-p`
    create missing parent dirs on the remote before writing
    (without this, a missing parent surfaces as `error: remote
    parent directory does not exist` and the transfer aborts).
    Operator overrides land *after* the mode/owner mirror from
    the prior file, so the override always wins.
  - **Audit shape.** `AuditEntry` gains five F15 fields
    (`transfer_direction: "up"|"down"`, `transfer_local`,
    `transfer_remote`, `transfer_bytes`, `transfer_sha256`), all
    `Option<T>` with `skip_serializing_if` so pre-F15 entries
    deserialize unchanged. The existing `previous_hash` /
    `new_hash` / `snapshot` / `diff_summary` fields continue to
    record what they did for v0.1.2 cp, so audit-driven revert
    keeps working for every entry.
  - **Hint surface.** `inspect run`'s F9 stdin-cap hint and
    F14's `--file` size-cap hint now point at `inspect put`
    (the canonical name) for bulk transfer instead of the
    pre-F15 `inspect cp`. `inspect exec --apply` still references
    `cp` in its "structured-write-verb" hint, now interpreted as
    the bidirectional alias to `put` / `get`.
  - **Out of scope for v0.1.3 (deferred to v0.1.5).**
    `--since <duration>` / `--max-bytes <size>` on `get`
    (log-retrieval ergonomics; the dedicated `inspect logs`
    verb already covers that domain), `--resume` for partial
    transfers (chunked-protocol design pass).
  - **Test coverage.** 12 acceptance tests in
    `tests/phase_f_v013.rs` (`f15_*`) covering: host-fs upload
    via stdin with audit fields recorded, container-fs upload
    via `docker exec -i`, state_snapshot revert when target
    exists, command_pair (rm) revert when target does not
    exist, dry-run no-dispatch, `--mode` override path,
    `--mkdir-p` parent creation path, host-fs download via
    `base64 --` decode, `-` local writes to stdout, `get`
    audit records `transfer_direction = down` +
    `revert.kind = unsupported`, `cp` regression dispatching to
    `put` / `get` by arg shape (audit verb canonicalised).
    Plus 8 unit tests in `src/verbs/transfer.rs::tests` for the
    atomic-write script builder and the `looks_remote`
    selector-vs-local-path detector.

- **L7 — Header / PEM / URL credential redaction in stdout
  (v0.1.2 retrospective: agent workflows pipe remote stdout into
  LLM context windows; the existing P4 line-oriented `KEY=VALUE`
  masker missed the three common shapes — `Authorization: Bearer
  …` headers in `curl -v`, PEM private-key blocks in
  `cat /etc/ssl/private/*.pem`, and credentials embedded in URLs
  like `postgres://user:pass@host/db` — so a single `inspect run`
  could leak a live token into a prompt).** The single-file
  `src/redact.rs` is replaced with a four-masker pipeline under
  `src/redact/` that runs on every line emitted by `inspect run`,
  `inspect exec`, `inspect logs`, `inspect cat`, `inspect grep`,
  `inspect search`, `inspect why`, `inspect find`, and the merged
  follow stream.
  - **PEM masker (`src/redact/pem.rs`).** Multi-line state
    machine. Recognized BEGIN forms: `PRIVATE KEY` (PKCS#8),
    `ENCRYPTED PRIVATE KEY` (PKCS#8 enc), `RSA PRIVATE KEY`
    (PKCS#1), `EC PRIVATE KEY` (SEC1), `DSA PRIVATE KEY`,
    `OPENSSH PRIVATE KEY`, and `PGP PRIVATE KEY BLOCK`. The
    BEGIN line emits a single `[REDACTED PEM KEY]` marker;
    every interior line plus the matching END line is suppressed
    by the composer. Public certificates
    (`-----BEGIN CERTIFICATE-----`) and public keys
    (`-----BEGIN PUBLIC KEY-----`) pass through unchanged.
  - **Header masker (`src/redact/header.rs`).** Case-insensitive
    word-bounded match on `Authorization`, `X-API-Key`, `Cookie`,
    `Set-Cookie` followed by `:`. Replaces the entire value
    portion with `<redacted>` so a `Cookie:` value containing
    its own URL credential is also covered. Word boundary on the
    name prevents false positives on prose like `MyAuthorization`.
  - **URL credential masker (`src/redact/url.rs`).** Masks the
    password portion of `scheme://user:pass@host` patterns to
    `user:****@host`, preserving scheme, username, and host so
    the diagnostic is still readable. Covers `postgres`, `mysql`,
    `redis`, `mongodb`, `mongodb+srv`, `amqp`, `http`, `https`,
    and any other scheme matching the userinfo grammar. Lines
    without the pattern are zero-allocation pass-through.
  - **Env masker (`src/redact/env.rs`).** The pre-existing P4
    line-oriented `KEY=VALUE` masker, preserved verbatim — same
    `head4****tail2` partial-mask shape, same suffix list, same
    `SECRETS_REDACT_ALL` opt-in, same audit-args `[secrets_masked]`
    text tag. Existing tests pass unchanged.
  - **Composer ordering.** Maskers run PEM → Header → URL → Env
    on every line. Inside a PEM block, the gate suppresses the
    other three so an interior line that happens to look like a
    header or URL credential is replaced by the single marker
    rather than partially leaked. Header and URL compose on a
    single line (a `Cookie:` value containing a URL credential is
    masked once by the header masker; the URL masker has nothing
    to do).
  - **`--show-secrets` bypass.** A single boolean on every read
    verb already wired in v0.1.2 now bypasses **all four** maskers
    in one place (`OutputRedactor::mask_line` short-circuits at
    the top), so the existing operator opt-in shape is unchanged
    for end users.
  - **Audit linkage.** `AuditEntry` gains
    `secrets_masked_kinds: Option<Vec<String>>` recording the
    deterministic ordered list of masker kinds that fired for a
    given step (`["pem", "header", "url", "env"]` — subset, in
    canonical order). The text tag `[secrets_masked=true]` on
    `inspect run` / `inspect exec` audit-args is preserved; the
    new field lets post-hoc reviewers tell two redacted runs
    apart by *which* pattern almost leaked. Pre-L7 entries omit
    the field via `skip_serializing_if`.
  - **API.** New `OutputRedactor::new(show_secrets, redact_all)`
    constructor; `mask_line(&str) -> Option<Cow<str>>` (returns
    `None` for suppressed PEM-interior lines); `was_active()`
    and `active_kinds()` for audit stamping. Public constants
    `REDACTED = "<redacted>"` and `PEM_REDACTED_MARKER`. One
    redactor is constructed per remote step so PEM gate state
    cannot leak across SSH dispatches.
  - **Performance.** All regexes compiled once via
    `once_cell::sync::Lazy`. Lines that match no masker return
    `Cow::Borrowed` with no allocation (verified by the
    `l7_redactor_unit_no_alloc_for_clean_lines` test).
  - **Test coverage.** 20 acceptance tests in
    `tests/phase_f_v013.rs` (`l7_*`) covering: PEM block
    collapse-to-marker on `cat` / `logs`, PGP private-key block,
    PKCS#8 unencrypted block, certificate pass-through,
    `Authorization` header on `curl -v` output via `grep` and
    `run`, `Set-Cookie` and `X-API-Key` case-insensitivity, URL
    credentials in path / `DB_URL` env var (double-masked by URL
    + env), URL-credentials-inside-prose on `find`,
    `--show-secrets` bypass on `cat`, `logs`, and `run` for all
    four patterns, audit-args `kinds=…` recording the canonical
    ordered subset, `secrets_masked_kinds` field absent when no
    masker fires, env-masker behavior unchanged from P4, the
    PEM-gate-suppresses-other-maskers contract, and the
    no-allocation pass-through invariant.

- **F14 — `inspect run --file <script>` / `--stdin-script` script
  mode (field feedback: *"the biggest individual time-sink was
  nested quoting. Every layer (your shell → ssh → bash →
  docker exec → psql/python -c) needs its own escape pass… I'd pay
  almost any feature in tradeoffs to never quote-escape across
  layers again."*).** Reads the entire bash payload from a file or
  stdin and ships it as the remote command body via `bash -s`
  (or the interpreter declared in the script's shebang) so nested
  `psql -c "..."`, `python -c '...'`, and `cypher-shell <<CYPHER`
  heredocs reach the remote interpreter byte-for-byte without
  any local escape pass.
  - **`--file <path>`.** Reads the script body from the local
    filesystem; rejects directories and missing paths with a
    chained recovery hint. Honors the F9 `--stdin-max` cap
    (default 10 MiB; raise with `--stdin-max <SIZE>`, set to `0`
    to disable, or use `inspect cp` for bulk transfer + remote
    execution).
  - **`--stdin-script`.** Reads the script body from local stdin
    (heredoc form: `inspect run arte --stdin-script <<'BASH' …
    BASH`). Mutually exclusive with `--file`, `--no-stdin`, and
    a tty stdin (clap-rejected or runtime-rejected with chained
    `--file`-pointing hint).
  - **Args after `--` become positional.** `inspect run <ns>
    --file s.sh -- alpha beta` runs `bash -s -- alpha beta` on
    the remote so the script's `$1` / `$2` are `alpha` / `beta`.
    Args are POSIX-shell-quoted before crossing the SSH boundary;
    the script body itself is never re-quoted.
  - **Shebang-driven interpreter dispatch.** A leading
    `#!/usr/bin/env <interp>` or `#!/path/to/<interp>` line
    selects the remote interpreter (`bash` / `sh` / `zsh` / `ksh`
    / `dash` use `-s`; `python3` / `node` / `ruby` / etc. use
    POSIX `-`). Without a shebang, defaults to `bash -s`.
    Interpreter names are sanitized to `[A-Za-z0-9_.-]`; anything
    else falls back to `bash` (defense against malformed or
    hostile shebangs).
  - **Container-targeted dispatch.** Selectors that resolve to a
    container render as `docker exec -i <ctr> <interp> -s …` so
    the script body flows in via the docker-exec stdin pipe.
  - **Audit shape.** Every script-mode invocation writes a
    per-step audit entry with `script_path`, `script_sha256`,
    `script_bytes`, and `script_interp`. With
    `--audit-script-body`, the full body is inlined under
    `script_body`; without it, the body is dedup-stored once at
    `~/.inspect/scripts/<sha256>.sh` (mode 0600 inside the 0700
    home) so audit reconstruction works even after the operator
    deletes the local file.
  - **Composes with F9 / F12 / F13.** Script mode dispatches
    through the same SSH executor as bare `inspect run`, so the
    namespace env overlay (F12) is applied, stale-session
    auto-reauth (F13) fires identically, and the size cap shares
    F9's `--stdin-max` byte budget.
  - **`AuditEntry` schema.** Five new optional fields
    (`script_path`, `script_sha256`, `script_bytes`,
    `script_body`, `script_interp`). Pre-F14 entries omit the
    fields via `skip_serializing_if`.
  - **Test coverage.** 14 acceptance tests in
    `tests/phase_f_v013.rs` (`f14_*`) covering: byte-for-byte
    heredoc fidelity, `--file` ↔ `--stdin-script` equivalence,
    mutual-exclusion clap rejections, positional-args dispatch,
    audit shape (path / sha / bytes / interp / inline body /
    dedup store), shebang-driven `python3 -` dispatch,
    container-targeted `docker exec -i` form, missing-path /
    directory / size-cap / empty-stdin rejection, and the
    "no `--` required for script mode" regression guard.

### Added (prior in v0.1.3)

- **F13 — Stale-session auto-reauth + distinct transport exit class
  (field feedback: the primary operator's most painful pattern was
  a stale `ControlPersist` socket returning `exit 255` minutes into
  a multi-host migration with stderr indistinguishable from a real
  command failure — every wrapper script's
  `if inspect run …; then` branch lit up the wrong way, requiring
  manual `inspect disconnect && inspect connect` and a from-scratch
  retry).** `inspect run` and `inspect exec` now classify every
  dispatch failure into one of three transport buckets and, by
  default, re-auth + retry once on stale sessions transparently.
  - **New exit codes.** `12` = `transport_stale`,
    `13` = `transport_unreachable`, `14` = `transport_auth_failed`.
    These three never collide with `ExitKind::Inner` (clamped to
    `1..=125`) so wrappers can branch on
    `case $? in 12) …;; 13) …;; 14) …; esac` reliably. Mixed
    multi-target failures still collapse to the existing
    `ExitKind::Error`.
  - **Auto-reauth.** When a step's stderr classifies as
    `transport_stale` (OpenSSH's `Connection closed`,
    `Control socket … unavailable`, `master process … exited`,
    or `mux_client_request_session: session request failed`),
    the dispatch wrapper writes a one-line
    `note: persistent session for <ns> expired —
    re-authenticating…` notice to stderr, records a
    `connect.reauth` audit entry, calls the same code path as
    interactive `inspect connect <ns>`, and re-runs the original
    step exactly once. A failed reauth escalates to
    `transport_auth_failed` (exit 14) with a chained
    `inspect connect <ns>` recovery hint.
  - **SUMMARY trailer.** When every failed step shares the same
    transport class, the human-format summary appends
    `(ssh_error: <class> — <recovery hint>)` after
    `run: N ok, M failed`.
  - **JSON contract.** Every `run` / `exec` invocation now emits
    a final `phase=summary` envelope with `ok`, `failed`, and
    `failure_class ∈ {ok, command_failed, transport_stale,
    transport_unreachable, transport_auth_failed,
    transport_mixed}`. Streaming line envelopes earlier in the
    run remain unchanged.
  - **Audit linkage.** `AuditEntry` gains three optional fields
    (`retry_of`, `reauth_id`, `failure_class`). The
    `connect.reauth` entry's id is stamped onto the post-retry
    audit entry's `reauth_id` so a downstream consumer can
    trivially correlate the failed attempt and its retry across
    the audit log.
  - **Opt-out.** Per-invocation `--no-reauth` on `run` / `exec`
    surfaces stale failures as exit 12 instead of retrying.
    Per-namespace `auto_reauth = false` in
    `~/.inspect/servers.toml` does the same persistently.
  - **Implementation.** New module `src/ssh/transport.rs` houses
    the pure classifier and `summary_hint` strings; new
    `src/exec/dispatch.rs` houses the reauth-aware dispatch
    wrapper that both `verbs/run.rs` and `verbs/write/exec.rs`
    flow through. The `RemoteRunner` trait gains a `reauth`
    method; `LiveRunner::reauth` delegates to a new
    `commands::connect::reauth_namespace` helper that tears
    down the dead master socket and re-runs `ssh::start_master`
    with the same `AuthSelection` as interactive connect.

- **F12 — Per-namespace remote env overlay (field feedback: the
  primary operator repeatedly hit
  `bash: cargo: command not found` /
  `LANG: cannot set locale` after `inspect connect arte` because
  the SSH non-login shell PATH on the target jumped over
  `~/.cargo/bin` and locale was unset, forcing a per-session
  `export PATH=…` ritual before every `inspect run`).** Each
  namespace can now persist a small environment overlay that is
  prefixed onto every remote command issued by `inspect run` and
  `inspect exec` for that namespace.
  - **Config.** New optional `[servers.<ns>.env]` table in
    `~/.inspect/servers.toml` (string→string map). Keys are
    POSIX-validated (`[A-Za-z_][A-Za-z0-9_]*`); invalid keys
    fail the dispatch boundary in `verbs/runtime::resolve_target`
    so every verb path catches them, not just `connect`.
  - **Dispatch.** `NsCtx` now carries an `env_overlay: BTreeMap`
    populated from `resolved.config.env`. The overlay is rendered
    deterministically as
    `env KEY1="VAL1" KEY2="VAL2" -- <cmd>` and prepended to the
    remote command before quoting/transport. Values are
    double-quoted so `$VAR` still expands on the remote, but
    `;`/`&`/`|` stay literal; `"`, `\`, and backtick are
    escaped.
  - **Per-call overrides.** `inspect run` and `inspect exec` gain
    `--env KEY=VAL` (repeatable; user wins on collision),
    `--env-clear` (drop the namespace overlay for this call only),
    and `--debug` (prints the rendered command to stderr before
    transport — useful when you don't yet trust the overlay).
  - **`inspect connect` overlay management.**
    - `--show` — print the current overlay (`PATH:` line +
      `ENV:` block; exits 0 when none).
    - `--set-path <p>` — pin remote PATH for this namespace.
    - `--set-env KEY=VAL` (repeatable) and
      `--unset-env KEY` (repeatable) — atomic 0600 round-trip
      through `write_atomic_0600`. The `[env]` table is dropped
      from the TOML when the last entry is removed, so the file
      stays tidy.
    - `--detect-path` — opens an SSH probe, compares login vs
      non-login PATH, and offers to pin the login PATH when it
      adds entries the non-login shell is missing. Prompts on a
      tty; auto-declines (with a one-line note) when stdin is not
      a tty so CI runs are deterministic.
  - **Audit.** `AuditEntry` gains `env_overlay` and `rendered_cmd`
    so the JSONL log captures exactly what shipped to the remote.
  - **Tests.** 18 acceptance tests in `phase_f_v013.rs` cover:
    overlay applied to `run` and `exec`, audit fields recorded,
    `--env-clear` clears, `--env` user-wins-collision merge,
    no-overlay path stays clean (no `env --` prefix), `--debug`
    stderr, semicolon stays literal in values, invalid keys
    rejected at config and CLI boundaries, `connect --show`
    output, `--set-path` idempotency, `--set-env`/`--unset-env`
    round-trip, last-entry drops the `[env]` table, invalid
    `KEY=VAL` rejected, unknown namespace rejected,
    `--show`/`--set-env` mutual exclusion at clap.

- **F10 — 4th-user polish bundle (seven first-hour friction points
  surfaced by a fresh operator on a partly-discovered namespace).
  All seven sub-items shipped.**
  - **F10.1 — Namespace-flag-as-typo chained hint.** Operators with
    `kubectl -n <ns>` muscle memory commonly type
    `inspect why atlas-neo4j --on arte` (also `--in`/`--at`/
    `--host`/`--ns`/`--namespace`). The pre-clap detector in
    `main.rs` now emits
    `error: --on is not a flag — selectors are <ns>/<service>. Did
    you mean 'inspect why arte/atlas-neo4j'?` and exits 2.
    Conservative shape detection scoped to known selector-taking
    verbs.
  - **F10.2 — `inspect cat --lines L-R` server-side line slice.**
    Inclusive 1-based range with `--start`/`--end` synonym
    (mutually exclusive). JSON path emits structured `{n, line}`
    envelopes per kept line so agents get line numbers without
    parsing prose.
  - **F10.3 — `why <ns>/<container>` chained hint when the token
    is a running container but not a registered service.** Catches
    `SelectorError::NoMatches` / `EmptyProfile`, looks the literal
    token up against the runtime inventory, and on a hit emits a
    three-line chained hint (`inspect logs … / inspect run …
    docker inspect / inspect setup --force`). Exit 0
    (informational). Genuine typos still exit 2 unchanged.
  - **F10.4 — `inspect grep` / `inspect search` MODEL/EXAMPLE/NOTE
    help preface.** Both `--help` outputs now declare their model
    (grep shells out to remote `grep -r`; search runs a LogQL
    query against the profile-side index) and cross-reference
    each other so the right verb can be picked from `--help`
    alone.
  - **F10.5 — `--quiet` pipeline regression test.** F7.4's
    `status --quiet` (no envelope trailers) and
    `logs --tail N --quiet | wc -l == N` contracts are now
    explicit acceptance tests in `phase_f_v013.rs`.
  - **F10.6 — `inspect logs` on the top-level `--help` index.**
    `LONG_ABOUT` reorganized into COMMON / DIAGNOSTIC + READ /
    WRITE + LIFECYCLE blocks with
    `inspect logs arte/atlas-vault --since 5m --match 'panic'` as
    a worked example.
  - **F10.7 — `inspect run --clean-output` (alias `--no-tty`).**
    Strips ANSI CSI/OSC escape sequences from captured output and
    prepends `TERM=dumb` to the remote command's env. Mutually
    exclusive with `--tty`. ANSI strip is allocation-free when no
    ESC byte is present.

### Added (prior in v0.1.3)

- **F7 — Selector + output ergonomic papercuts (field feedback:
  five separate small papercuts collected over a v0.1.2 destructive
  migration session that each cost an extra round-trip to the docs
  or to `--help` to recover from). Five of six sub-items shipped;
  one (F7.6 `inspect connect` publickey ordering) is deferred to
  v0.1.5 because the interactive ssh path is not amenable to a
  reliable, mock-driven contract test.**
  - **F7.1 — Empty-profile hint redirects to `inspect setup`, not
    `inspect profile`.** When a service-specific selector
    (`inspect why arte/atlas-vault`, `inspect logs arte/onyx-vault`)
    targets a registered namespace whose cached profile contains
    zero services, the diagnostic now leads with
    `hint: run 'inspect setup <ns>' to discover services on this
    namespace` instead of the generic refresh-the-cache hint. The
    pre-existing `inspect profile / inspect setup --force` hint
    is preserved for the genuine "selector typo against a
    populated profile" case (`SelectorError::NoMatches`).
  - **F7.2 — `arte:/path` shorthand is no longer rejected by the
    selector parser.** The shape `arte:/etc/hostname` (sugar for
    `arte/_:/etc/hostname` — host-level read against the
    namespace's SSH host) used to surface as
    `invalid selector character ':' ...` because the parser
    tokenized the absolute path's leading slash as the
    server/service separator. `parse_selector` now detects the
    shorthand by checking whether the first colon (outside any
    regex `/.../`) precedes the first slash; if so, the colon
    is the service/path separator and the implicit `_` host
    service is supplied automatically.
  - **F7.3 — `inspect ports` server-side port filters:
    `--port <n>` and `--port-range <lo-hi>`.** Filter each
    output line through a token-aware port matcher (handles
    both `0.0.0.0:8200` and `8200/tcp` shapes); the SUMMARY's
    listener count reflects the filtered total. Flags are
    mutually exclusive at the clap level.
  - **F7.4 — Global `--quiet` flag suppresses the SUMMARY/NEXT
    envelope on the Human path.** Pipeline-friendly: data rows
    are emitted without the leading two-space indent prefix so
    output is safe to feed directly into `grep`, `awk`, `tail`,
    `head`, `wc -l`. Mutually exclusive with `--json` /
    `--jsonl`. Wired through `OutputDoc::with_quiet` and
    `Renderer::quiet` so every read verb inherits the contract.
  - **F7.5 — `inspect status` empty-state phrasing + explicit
    `state` JSON field.** Three states distinguished instead of
    folding into `0 service(s): 0 healthy, 0 unhealthy, 0 unknown`:
    `state: "ok"` (≥1 service classified), `"no_services_matched"`
    (non-empty inventory but zero profile entries; SUMMARY phrases
    as a config condition and NEXT chains
    `inspect ps <ns>` + `inspect setup <ns> --force`),
    `"empty_inventory"` (host clean / docker down). A small
    `dispatch::plan` change ensures a wildcard selector
    (`arte/*`) against an empty profile still binds the
    namespace's `NsCtx` so the verb can still call `docker ps`.
  - **F7.7 — `inspect status --json` carries the new `state`
    field.** Already supported via the standard `FormatArgs`
    flatten.

- **F4 — `inspect why` deep-diagnostic bundle (field feedback: the
  primary operator's multi-hour Vault triage where `why` said
  "unhealthy <- likely root cause" and stopped, forcing a hand-rolled
  walk through `logs`, `inspect`, and `ports` to learn that the
  entrypoint wrapper was injecting `-dev-listen-address=0.0.0.0:8200`
  on top of a config-declared listener; the same port bound twice).**
  When the target service of `inspect why <selector>` is unhealthy or
  down, three artifacts are now attached inline under `DATA:`:
  - **`logs:`** — the recent log tail (default 20 lines, configurable
    via `--log-tail <N>`, hard-capped at 200 with a one-line stderr
    notice — protects redaction + transport).
  - **`effective_command:`** — the container's effective `Entrypoint`
    + `Cmd` from `docker inspect`, plus a `wrapper injects:` line
    when the entrypoint script contains a flag-injection pattern
    (`-dev-listen-address=`, `-listen-address=`, `-bind-address=`,
    `-api-addr=`, `--listen-address=`).
  - **`port_reality:`** — per-port table cross-referencing
    `PortBindings` + `ExposedPorts` from `docker inspect`,
    entrypoint-injected listeners, and host listener state from
    `ss -ltn` (or `netstat -ltn` fallback). Ports declared by both
    config *and* a wrapper-injected flag are flagged
    `container: bound (twice!)` — the headline reproducer pattern.
  Hard-capped at **≤4 extra remote commands per service per bundle
  invocation** (`docker logs --tail`, one combined `docker inspect`,
  `docker exec ... cat /docker-entrypoint.sh`, `ss -ltn`). All four
  fail independently — partial bundles still surface what worked.
  New flags: `--no-bundle` (suppress; restores the v0.1.2 terse
  output) and `--log-tail <N>` (default 20, capped at 200).
  Smart `NEXT:` hints derived from the bundle: a "bound (twice!)"
  port pushes `inspect run <ns>/<svc> -- 'cat /docker-entrypoint.sh'`,
  and `address already in use` in the logs pushes `inspect ports
  <ns>`. JSON adds three fields to each per-service object —
  `recent_logs[]`, `effective_command{entrypoint, cmd, wrapper_injects}`,
  `port_reality[{port, host, container, declared_by}]` — always
  present (empty arrays / `null` on healthy services so agents don't
  need optional-chaining gymnastics). Healthy services are
  byte-for-byte unchanged: no extra round-trips, no bundle headers.
  Eight acceptance tests in `tests/phase_f_v013.rs` covering
  unhealthy/healthy paths, `--no-bundle`, `--log-tail` clamp, JSON
  schema stability, wrapper-injection detection, and smart-NEXT
  emission.

- **F5 — Container-name vs compose-service-name uniform resolution
  (field feedback: 2nd v0.1.2 user typed `arte/luminary-onyx-onyx-vault-1`
  — the docker name from `docker ps` — got "no targets", then
  `arte/onyx-vault` — the compose service name — worked; both forms
  appeared in the discovered inventory).** Selector resolver in
  `src/selector/resolve.rs::resolve_services_for_ns` now accepts the
  docker `container_name` as an exact-match synonym for the canonical
  compose service `name`, and globs match against either axis. The
  resolved target is always the canonical name so every downstream
  verb (`logs`, `run`, `restart`, …) addresses the same profile row
  regardless of which form the operator typed. When the typed form
  was the docker container name, `inspect` emits a one-line
  breadcrumb on stderr — `note: '<typed>' is the docker container
  name; the canonical selector is '<ns>/<canonical>'` — so the
  operator learns the canonical form. Hint is suppressible via
  `INSPECT_NO_CANONICAL_HINT=1` for strict-stderr JSON consumers.
  `inspect status --json` now exposes a stable `aliases: [<docker
  container name>]` field on every service row (empty array when the
  canonical name already matches the docker name) so agents can
  enumerate equivalences without trial-and-error. 6 acceptance tests
  in `tests/phase_f_v013.rs` cover both selector forms, the
  canonical-form breadcrumb, the silent-when-no-distinct-alias case,
  the JSON `aliases` field shape (present + populated, present +
  empty), and globs across either axis.

- **F3 — `inspect help <command>` is now a true synonym for
  `inspect <command> --help` (carry-over field-feedback ergonomic gap;
  every operator coming from `git help log` / `cargo help build` /
  `kubectl help get` hit it on first try).** The argv rewrite happens
  in `src/main.rs::rewrite_help_synonym` *before* clap parses,
  guaranteeing the rendered output is **byte-for-byte identical** to
  the `--help` form (no drift on `Usage:` line, no missing
  `-V, --version` row). Editorial topics keep precedence: `inspect
  help search` still renders the curated LogQL DSL guide, and
  `inspect search --help` still shows clap's flag list — the new
  rewrite only fires when the token is a verb *and not* a topic.
  `tests/phase_f_v013.rs::f3_help_verb_byte_for_byte_matches_dash_dash_help`
  pins the contract for 22 representative top-level verbs (read /
  write / lifecycle / discovery / safety / diagnostic).
- Unknown `inspect help <foo>` now exits **2** (Error) with `error:
  unknown command or topic: '<foo>'` and the canonical `see: inspect
  help examples` chained hint. Pre-F3 it was exit 1 + `unknown help
  topic` — the change aligns with `git`/`cargo`/`kubectl` and reflects
  that `<foo>` could be either a verb *or* a topic. The
  `ERROR_CATALOG` `UnknownHelpTopic` row was updated in lockstep so
  the see-line still attaches automatically.

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
