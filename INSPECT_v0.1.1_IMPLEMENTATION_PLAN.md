# Inspect CLI — v0.1.1 Implementation Plan

**Companion to:** [INSPECT_v0.1.1_PATCH_SPEC.md](INSPECT_v0.1.1_PATCH_SPEC.md)
**Source feedback:** [inspec-clifeedback.md](inspec-clifeedback.md) (2026-04-27, two real-world sessions, ~60 calls on `arte`)
**Goal:** Translate every patch in the spec into concrete file/struct/function-level code changes, in the order they should land.

This plan is the dev-facing twin of the spec. Spec says *what* and *why*; this says *where* and *how*. Nothing here is deferred — every patch ships.

---

## 0. Ground rules

1. **No new dependencies in Phase A.** `indicatif`, `regex`, `serde_json`, `chrono`, `dirs` are already in the tree (see [Cargo.toml](Cargo.toml)) — reuse them.
2. **No backward compatibility concerns.** Single-user pre-release; we are free to break the CLI surface, profile schema, audit schema, and config format. `--allow-exec` is **removed outright** (not deprecated). Old cached profiles get **regenerated**, not migrated — `inspect setup <ns>` is the upgrade path. No deprecation notices, no `#[serde(default)]` carry-overs from v0.1.0.
3. **Every patch ships with tests.** Existing test phases (`phase4_read_verbs.rs`, `phase5_write_verbs.rs`, `phase7_exec.rs`, `phase9_diagnostics.rs`) are extended; no new top-level test phases are needed. Old tests that pinned the deprecated surface are deleted, not updated.
4. **Help & docs are part of the patch.** A patch is not done until [docs/MANUAL.md](docs/MANUAL.md), the relevant [src/help/content/](src/help/content/) topic, and `inspect help --json` reflect it.
5. **Bump:** `Cargo.toml` version → `0.1.1`, `CHANGELOG.md` gains a v0.1.1 section flagged as **breaking**, [packaging/homebrew/inspect.rb](packaging/homebrew/inspect.rb) sha256 refreshed at release time.

---

## 1. Phase A — Critical fixes (land first)

These are the trust-and-discoverability fixes. Without them users hit "the docs lie / the tool is broken" within minutes.

### Patch 2 — Fix phantom service names

**Files:**
- [src/profile/schema.rs](src/profile/schema.rs) — add `container_name: String` to `Service`, keep `name` as user-facing.
- [src/discovery/probes.rs](src/discovery/probes.rs) — `probe_docker_containers()` (lines 148-230 region) is the producer.
- [src/selector/resolve.rs](src/selector/resolve.rs) — must resolve to `container_name` for docker commands.
- [src/verbs/dispatch.rs](src/verbs/dispatch.rs) — `step.service()` returns `name`; add `step.container()` returning `container_name`.
- [src/verbs/logs.rs](src/verbs/logs.rs), [src/verbs/write/exec.rs](src/verbs/write/exec.rs), [src/verbs/write/lifecycle.rs](src/verbs/write/lifecycle.rs), [src/verbs/ps.rs](src/verbs/ps.rs), [src/verbs/health.rs](src/verbs/health.rs) — change every `docker logs|exec|restart <svc>` call site to use `step.container()`.

**Code shape:**

```rust
// src/profile/schema.rs
pub struct Service {
    pub name: String,                    // user-facing (compose service or container name)
    pub container_name: String,          // ALWAYS the real `docker ps --format {{.Names}}` value
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compose_service: Option<String>, // informational only
    // ...existing fields...
}
```

**Discovery rules** (in `probe_docker_containers`, after the `docker ps` parse):

1. For every row, `container_name = row.names`.
2. If `row.compose_service` is `Some(s)` AND `s` does not collide with another running container in the same namespace AND `s` is not already used by another `Service.name` → set `name = s`, `compose_service = Some(s)`. Else `name = container_name`.
3. **Drop the existing nickname-derivation logic** that produced `api`, `worker`, `pulse`, `backend` from image-name fragments. That code is the bug; delete it.
4. Add a discovery warning row for every service where compose-label collision forced a fallback to the long name (already partially done — extend wording).

**Selector resolver:** `selector::resolve` already maps short tokens → service. Add a post-resolve normalisation that returns the matched `Service` reference (not just its name); call sites use `service_def().container_name`.

**Schema:** `container_name` is **required** (no `#[serde(default)]`). Old cached profiles fail to deserialise with a clear error: `error: profile schema is from v0.1.0; run 'inspect setup <ns>' to regenerate`. Add a one-line check in [src/profile/cache.rs](src/profile/cache.rs) that catches the deserialisation error and rewrites it to that message. No migration code.

**Tests:**
- [tests/phase2_discovery.rs](tests/phase2_discovery.rs) — fixture with two compose containers same service name → only one canonical entry, second has warning.
- [tests/phase4_read_verbs.rs](tests/phase4_read_verbs.rs) — round-trip: every name in `inspect ps` must `inspect logs` without error.

**DoD:** Zero entries in a fresh `arte` profile resolve to a non-existent container.

---

### Patch 1 — `--follow` / `-f` streaming on `inspect logs`

**Status check:** `LogsArgs.follow` already exists in [src/cli.rs](src/cli.rs#L1161); the inner `build_docker_logs` already handles `-f` with `stdbuf -oL` and a reconnect loop ([src/verbs/logs.rs](src/verbs/logs.rs#L138-L168)). **The plumbing is mostly there.** What's missing is the streaming I/O path: `runner.run()` returns a `CmdOutput` (buffered), so the operator never sees a line until SSH closes — which in follow mode is never.

**Files:**
- [src/ssh/exec.rs](src/ssh/exec.rs) — add `run_streaming(...)` that returns an iterator/callback over stdout lines instead of buffering. Signature:

  ```rust
  pub fn run_streaming<F: FnMut(&str)>(
      ns: &str, target: &SshTarget, cmd: &str, opts: RunOpts, on_line: F,
  ) -> Result<i32 /*exit*/>;
  ```

  Implementation: spawn `ssh` via `tokio::process::Command` (already pulled in by audit/discovery code), set `stdout(Stdio::piped())`, wrap in `BufReader`, call `on_line` per line. Cancellation via the existing [src/exec/cancel.rs](src/exec/cancel.rs) cancellation token + Ctrl-C handler.

- [src/verbs/logs.rs](src/verbs/logs.rs) — when `args.follow`: switch from `runner.run()` → `runner.run_streaming()`. Each line goes straight to stdout (or JSON envelope). Drop the SUMMARY line; print a final summary on Ctrl-C.

- [src/exec/cancel.rs](src/exec/cancel.rs) — register a SIGINT handler that flips a `CancellationToken`; the streaming loop checks it between lines.

**Reconnect:** the server-side `while :; do ... sleep 1; done` loop already handles file-rotation reconnects on the *remote* side. Add a *client*-side reconnect: when SSH itself drops (exit ≠ 0 in follow mode), retry up to 3 times with 1s/2s/4s backoff and print `note: reconnected to arte/svc` on stderr.

**Interaction with `--match`/`--exclude` (Patch 3):** filtering happens in the `on_line` callback, so they compose naturally.

**Tests:**
- [tests/phase4_read_verbs.rs](tests/phase4_read_verbs.rs) — mock SSH runner that emits 5 lines spaced 200ms apart; assert the test sees lines arrive incrementally, not all at end.
- A unit test forcing a fake SSH exit during follow → assert reconnect message + stream resumes.

**DoD:** Spec test #1 of v0.1.1 exit criteria passes.

---

### Patch 8 — `inspect help <command>` fallback

**Files:**
- [src/commands/help.rs](src/commands/help.rs) — `unknown_topic()` (line 118) is the hook.
- [src/help/topics.rs](src/help/topics.rs) — add `is_known_verb(name) -> bool` reading the `VERB_TOPICS` table (line 135).

**Logic** in `commands/help.rs::run`:

```rust
match args.topic.as_deref() {
    Some(name) if help::find(name).is_some() => /* current path */,
    Some(name) if topics::is_known_verb(name) => {
        // Re-dispatch as `inspect <name> --help`.
        return delegate_to_command_help(name);
    }
    Some(name) => unknown_topic_or_command(name),
    None => /* index */,
}
```

`delegate_to_command_help`: build a fresh `clap::Command` from `Cli::command()` and call `.find_subcommand_mut(name)?.print_long_help()`. clap supports this directly — no shell-out.

`unknown_topic_or_command`: reuses `help::suggest()` but searches both `TOPICS` and verb names with the same Levenshtein scorer.

**Tests:** [tests/help_contract.rs](tests/help_contract.rs) — `inspect help add`, `inspect help logs`, `inspect help edit` all exit 0 with non-empty stdout containing `EXAMPLES`. `inspect help serch` exits 1 with `did you mean: search`.

**DoD:** No "unknown help topic" for any name in `Cli` or `TOPICS`.

---

## 2. Phase B — Agent-workflow tier

These are the high-leverage features that turn inspect from "ssh+docker wrapper" into "actual SRE tool" for agent loops.

### Patch 6 — Split `exec` into `run` (read) + `exec` (write)

**Files:**
- [src/cli.rs](src/cli.rs) — add a new `Run(RunArgs)` variant to `Command` enum (line ~470 region). `RunArgs` is `ExecArgs` minus `apply`, `allow_exec`, `yes`, `yes_all`. Plus a `--reason` field (Patch 12 lands at the same time).
- [src/main.rs](src/main.rs) — dispatch new `Run` arm.
- New file: [src/verbs/run.rs](src/verbs/run.rs) — read-only execution. Copy [src/verbs/write/exec.rs](src/verbs/write/exec.rs) as starting point, strip the gate/audit/dry-run sections.
- [src/verbs/mod.rs](src/verbs/mod.rs) — register `pub mod run;`.
- [src/help/topics.rs](src/help/topics.rs) — add `("run", &["selectors", "formats", "examples"])` to `VERB_TOPICS`.
- New file: [src/help/content/run.md](src/help/content/run.md) (or extend existing `write.md` with a "Read-only execution" section + add a small `run` topic body).

**`RunArgs` shape:**

```rust
#[derive(Debug, Args)]
pub struct RunArgs {
    pub selector: String,
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub cmd: Vec<String>,
    #[arg(long)] pub timeout_secs: Option<u64>,
    #[arg(long)] pub show_secrets: bool,    // Patch 4
    #[arg(long)] pub redact_all: bool,      // Patch 4
    #[arg(long, value_name = "REGEX")] pub filter_line_pattern: Option<String>, // server-side line filter
    #[command(flatten)] pub format: crate::format::FormatArgs,
}
```

**Behaviour of `inspect run`:**
1. No `--apply` / `--allow-exec` / dry-run.
2. No `AuditStore.append()` call. (Reads aren't audited.)
3. Output goes through the secret-masking pipeline (Patch 4).
4. Process exit code mirrors the inner command's exit code (Patch 11).
5. `--filter-line-pattern <re>` appends `| grep -E <re>` to the remote command before execution (Patch 10 server-side filter; light-touch implementation).

**Removal of `--allow-exec`:**
- Delete the `pub allow_exec: bool` field from `ExecArgs` outright.
- Delete the `if gate.should_apply() && !args.allow_exec { ... }` block in [src/verbs/write/exec.rs](src/verbs/write/exec.rs).
- `LONG_EXEC` rewritten: drop the "doubly gated" wording, recommend `inspect run` for reads.
- Any test that passed `--allow-exec` is updated to drop the flag (search-and-replace in `tests/`).

**Tests:**
- New [tests/phase7_exec.rs](tests/phase7_exec.rs) cases:
  - `inspect run arte/api -- env` → exit 0, no audit entry created.
  - `inspect exec arte/api -- 'echo hi' --apply` → works with one flag.
  - `inspect exec ... --apply --allow-exec` → **fails** with clap's standard "unexpected argument" error (proves the flag is gone).

**DoD:** Spec test #3 and #10 of v0.1.1 exit criteria pass.

---

### Patch 7 — Collapse `--apply --allow-exec`

Implementation note: this is the same edit landed under Patch 6's "Removal of `--allow-exec`" section above. No separate work item — Patch 7 is the cleanup half of Patch 6, but called out separately so test coverage is explicit.

**Tests:** assert single-flag works; assert `--allow-exec` is rejected by clap; assert audit entry recorded with `verb=exec`.

---

### Patch 3 — `--match` / `--exclude` on `logs` and `grep`

**Files:**
- [src/cli.rs](src/cli.rs) — extend `LogsArgs` and `GrepArgs`:

  ```rust
  /// Filter to lines matching this regex. Repeatable; OR semantics.
  #[arg(long = "match", short = 'g', value_name = "REGEX")]
  pub match_re: Vec<String>,
  /// Hide lines matching this regex. Repeatable; OR semantics.
  #[arg(long = "exclude", short = 'G', value_name = "REGEX")]
  pub exclude_re: Vec<String>,
  ```

- New helper module: [src/verbs/line_filter.rs](src/verbs/line_filter.rs) — compiles patterns once into a `LineFilter` struct, exposes `keep(&self, line: &str) -> bool`. Uses the `regex` crate (already in stack via `redact.rs`).

- [src/verbs/logs.rs](src/verbs/logs.rs) — apply filter in two places:
  1. **Server-side pushdown:** when both `--match` is non-empty and `--exclude` is non-empty, append `| grep -E '<combined-match>' | grep -vE '<combined-exclude>'` to the remote command. Use `shquote` for safety.
  2. **Client-side fallback:** if pushdown is disabled (e.g. `--json` mode where structured records flow), re-apply the filter on each parsed record's `line` field. This guarantees correctness when grep isn't available remotely or when records are multi-line JSON.

- [src/verbs/grep.rs](src/verbs/grep.rs) — extend the existing pattern with `--match`/`--exclude` as additive filters layered over the primary pattern.

**Pushdown contract:** when `--follow` + `--match` are combined, the filter goes server-side via `grep --line-buffered -E` so streaming stays line-by-line. Skip pushdown only when remote `grep` isn't probed-available (fall back to client-side).

**Tests:** [tests/phase4_read_verbs.rs](tests/phase4_read_verbs.rs) — fixture log of 10 mixed lines, assert `--match error` keeps 3, `--match error --exclude healthcheck` keeps 2, `--match a --match b` keeps a-OR-b.

---

### Patch 10 — `--since-last` cursor

**Files:**
- [src/paths.rs](src/paths.rs) — add:

  ```rust
  pub fn cursors_dir() -> PathBuf { inspect_home().join("cursors") }
  pub fn cursor_file(ns: &str, service: &str) -> PathBuf {
      cursors_dir().join(format!("{ns}-{service}.txt"))
  }
  ```

  Plus `ensure_cursors_dir()` mode 0700.

- New module: [src/verbs/cursor.rs](src/verbs/cursor.rs) — read/write a small struct:

  ```rust
  pub struct Cursor {
      pub ns: String,
      pub service: String,
      pub last_ts: DateTime<Utc>,
      pub last_call: DateTime<Utc>,
  }

  impl Cursor {
      pub fn load(ns: &str, service: &str) -> Result<Option<Self>>;
      pub fn save(&self) -> Result<()>;
      pub fn reset(ns: &str, service: &str) -> Result<()>;
  }
  ```

  File format: 4 lines of `key=value`, mode 0600 (use existing `set_file_mode_0600`).

- [src/cli.rs](src/cli.rs) `LogsArgs`:

  ```rust
  #[arg(long, conflicts_with = "since")]
  pub since_last: bool,
  #[arg(long, conflicts_with_all = ["since", "since_last"])]
  pub reset_cursor: bool,
  ```

- [src/verbs/logs.rs](src/verbs/logs.rs):
  - On entry, if `args.since_last`: load cursor → set effective `since` to `cursor.last_ts.to_rfc3339()` (or default 5m if absent).
  - On stream completion / each emitted line: track max timestamp seen.
  - On exit (or every 30s in `--follow`): persist new `last_ts` to cursor file.
  - On `--reset-cursor`: delete cursor file, exit 0 with note.

- [src/verbs/grep.rs](src/verbs/grep.rs) — same flag wiring.

**Default first-call window:** `INSPECT_SINCE_LAST_DEFAULT` env var (default `5m`).

**Tests:** [tests/phase4_read_verbs.rs](tests/phase4_read_verbs.rs) — write fake cursor, run logs, assert second call's `since` arg matches first call's recorded ts. Use a temp `INSPECT_HOME`.

**DoD:** Spec test #5 of v0.1.1 exit criteria.

---

## 3. Phase C — Quality and safety

### Patch 4 — Secret masking on `run` / `exec` stdout

**Files:**
- [src/redact.rs](src/redact.rs) — already exists for credential redaction in profile output; **extend, do not duplicate.** Add:

  ```rust
  pub struct EnvSecretMasker { /* compiled patterns + flags */ }
  impl EnvSecretMasker {
      pub fn new(show_secrets: bool, redact_all: bool) -> Self;
      pub fn mask_line(&self, line: &str) -> Cow<'_, str>;
      pub fn was_active(&self) -> bool; // for audit
  }
  ```

  Pattern list (case-insensitive, suffix match on the key):
  ```
  _KEY, _SECRET, _TOKEN, _PASSWORD, _PASS, _CREDENTIAL, _CREDENTIALS,
  _APIKEY, _AUTH, _PRIVATE, _ACCESS_KEY,
  DATABASE_URL, REDIS_URL, MONGO_URL, _DSN, _CONNECTION_STRING
  ```

  Mask format: keep first 4 + last 2 chars of value, middle replaced with `****`. Values <8 chars become `****`.

- [src/verbs/run.rs](src/verbs/run.rs) — wrap the streaming `on_line` callback with `masker.mask_line(...)` before printing.
- [src/verbs/write/exec.rs](src/verbs/write/exec.rs) — same wrapping on output.
- `--show-secrets` adds `secrets_exposed: true` to the `AuditEntry.args` JSON for `exec`.

**Limitation (documented):** masking only catches `KEY=VALUE` form. Anything inside a JSON blob or free text passes through. Note this in [src/help/content/write.md](src/help/content/write.md).

**Tests:** [tests/phase7_exec.rs](tests/phase7_exec.rs) — feed `printf 'ANTHROPIC_API_KEY=sk-abcdefghk3\nFOO=bar\n'` through `inspect run`, assert output contains `sk-a****k3` and `FOO=bar`.

---

### Patch 5 — `--merged` multi-container log view

**Files:**
- [src/cli.rs](src/cli.rs) `LogsArgs` — `#[arg(long)] pub merged: bool`.
- [src/verbs/logs.rs](src/verbs/logs.rs) — when `merged && steps.len() > 1`:
  - **Batch mode:** spawn N parallel `runner.run()` calls, collect their lines into per-step Vec<(timestamp, line)>, k-way merge via `BinaryHeap<Reverse<(ts, src_idx, line)>>`. Print with colored `[svc]` prefix using `nu-ansi-term` if already in tree, else plain `[svc]`.
  - **Follow mode:** spawn N `run_streaming()` tasks, each pushes lines into a single `mpsc::channel<(svc, line)>`; consumer prints in arrival order (no sort; document the clock-skew caveat).

- [src/verbs/output.rs](src/verbs/output.rs) — add `MergedRenderer` that handles the prefix.

**Timestamp parsing:** docker logs `--timestamps` flag adds RFC3339 prefix. Add `--timestamps` to the remote command when `--merged` is on; parse off the leading token; on parse failure, fall back to arrival order.

**Tests:** unit test for the BinaryHeap merger with three pre-tagged streams of timestamped lines; assert output is ts-sorted with correct prefixes.

---

### Patch 11 — Inner exit code surfacing

**Files:**
- [src/verbs/output.rs](src/verbs/output.rs) — `Envelope`/`Renderer` already accept arbitrary fields. Add `_exit_code: i32` consistently to exec and run output.
- [src/verbs/run.rs](src/verbs/run.rs) and [src/verbs/write/exec.rs](src/verbs/write/exec.rs):
  - Capture the inner `out.exit_code`.
  - Set `ExitKind::ExitCode(out.exit_code)` for single-target runs.
  - For multi-target / fleet: if all same exit, propagate; else exit 1 with per-target table.

- [src/error.rs](src/error.rs) — `ExitKind` already has `Success`, `NoMatches`, `Error`. Add:

  ```rust
  pub enum ExitKind { Success, NoMatches, Error, Inner(i32) }
  impl ExitKind { pub fn code(self) -> i32 { match self { Inner(n) => n, ... } } }
  ```

  Update [src/main.rs](src/main.rs) exit-code mapping.

**Tests:** [tests/phase7_exec.rs](tests/phase7_exec.rs) — `inspect run arte/api -- 'exit 7'` returns exit 7. `inspect run ... -- 'grep nonexistent /etc/hosts'` returns 1.

---

### Patch 9 — Progress indicator on slow log fetches

**Files:**
- New module: [src/verbs/progress.rs](src/verbs/progress.rs) — uses `indicatif::ProgressBar::new_spinner()` with `set_draw_target(stderr())`, started on a tokio task. Cancellable via tokio oneshot.
- [src/verbs/logs.rs](src/verbs/logs.rs), [src/verbs/grep.rs](src/verbs/grep.rs), [src/commands/search.rs](src/commands/search.rs):
  - Wrap the `runner.run()` / `run_streaming()` call in `with_progress(label, fut)` that:
    - Sleeps 2s.
    - If the inner future hasn't produced first output yet, starts the spinner.
    - Cancels spinner on first byte.
  - **Skip when** `args.format.is_json()` or stderr is not a TTY (`atty::isnt(Stderr)`) — tests run without a TTY by default.

**`indicatif` already used?** Check [Cargo.toml](Cargo.toml). If not, this is the one allowed new dep. Otherwise, hand-roll a 30-line spinner with `\r` overwrites.

**Tests:** smoke test that a `RUST_LOG`-instrumented mock with a 3s delay produces a stderr line containing `Scanning logs`. Also assert no progress in `--json` mode.

---

### Patch 12 — `--reason` audit comment

**Files:**
- [src/safety/audit.rs](src/safety/audit.rs) `AuditEntry`:

  ```rust
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub reason: Option<String>,
  ```

- [src/cli.rs](src/cli.rs) — add `#[arg(long, value_name = "TEXT")] pub reason: Option<String>` to **every write Args struct**: `LifecycleArgs`, `ExecArgs`, `PathArgArgs`, `ChmodArgs`, `ChownArgs`, `CpArgs`, `EditArgs`. Do NOT add to read verbs.
- Each write verb (in [src/verbs/write/*.rs](src/verbs/write/)) sets `entry.reason = args.reason.clone()` before `store.append()`.
- [src/commands/audit.rs](src/commands/audit.rs):
  - `audit ls`: render reason as 4th column when non-null. Truncate to 60 chars with `…`.
  - Add `#[arg(long, value_name = "PATTERN")] pub reason: Option<String>` to `AuditLsArgs`; filter rows by substring.
  - `--json`: include `reason` field unconditionally.

- Length limit: 240 chars. Reject longer with `error: --reason must be ≤ 240 characters`.

**Tests:** [tests/phase5_write_verbs.rs](tests/phase5_write_verbs.rs) — apply edit with `--reason "test"`, then run `inspect audit ls`, assert reason appears.

---

### Patch 13 — Discovery `docker inspect` per-container fallback

**Files:**
- [src/discovery/probes.rs](src/discovery/probes.rs) `probe_docker_containers()` (the section starting line ~189 with `let inspect_cmd = ...`):

  ```rust
  let details = match try_batch_inspect(ns, target, &ids) {
      Ok(d) => d,
      Err(BatchErr::TimedOut) | Err(BatchErr::Failed(_)) => {
          // Per-container fallback.
          inspect_per_container(ns, target, &ids, &mut r.warnings)
      }
  };
  ```

  - `try_batch_inspect`: existing call but with 10s timeout (down from 30s).
  - `inspect_per_container`: loop over IDs with 5s each. On per-container timeout, push `format!("Timed out inspecting container {name}. Info may be incomplete for this service.")` to warnings and skip.

- [src/commands/setup.rs](src/commands/setup.rs) — at end of discovery, if `warnings` contains any "Timed out inspecting", print:

  ```
  Warning: <N> container(s) timed out during inspection.
    Affected: <names>
    Run 'inspect setup arte --retry-failed' to retry.
  ```

  Add `#[arg(long)] pub retry_failed: bool` to `SetupArgs`. When set, the discovery pass starts from the cached profile and only re-inspects containers whose entry has a "incomplete" marker (new `Service.discovery_incomplete: bool` field on the schema, default false, set when fallback skipped).

**Tests:** [tests/phase2_discovery.rs](tests/phase2_discovery.rs) — mock SshRunner that times out for one container ID; assert: profile still produced, warning recorded, `Service.discovery_incomplete = true` for that one, others fine.

---

## 4. Cross-cutting work

### 4.1 Help & docs sweep

After all patches:

- [src/help/content/write.md](src/help/content/write.md) — section "`run` vs `exec`": when to use each, secret masking, `--reason`, deprecation of `--allow-exec`.
- [src/help/content/examples.md](src/help/content/examples.md) — replace every `inspect exec ... --apply --allow-exec -- "docker logs ..."` example with `inspect run ... -- "docker logs ..."`.
- [src/help/content/search.md](src/help/content/search.md) — note `--match` on `logs` is the Tier 1 sugar for LogQL `|=`.
- New: [src/help/content/run.md](src/help/content/run.md) — short topic body for the new verb.
- [src/help/topics.rs](src/help/topics.rs) `TOPICS` — append `run` topic entry; add `("run", &["selectors", "formats", "examples"])` to `VERB_TOPICS`.
- [docs/MANUAL.md](docs/MANUAL.md) — sections: `--follow` with examples, `--merged`, `--match`/`--exclude`, `--since-last`, `inspect run`, `--reason`, secret masking, deprecation notice.
- [docs/RUNBOOK.md](docs/RUNBOOK.md) — agent-debugging recipe section using the new flags.
- [README.md](README.md) — feature table updated.

### 4.2 Test contract additions

- [tests/help_contract.rs](tests/help_contract.rs) — pin `run` verb in the `VERB_TOPICS` list; assert `inspect help run` works; assert `inspect help <every-verb>` works.
- [tests/help_json_snapshot.rs](tests/help_json_snapshot.rs) — regenerate snapshot for new verb + new topic.
- [tests/no_dead_code.rs](tests/no_dead_code.rs) — confirm new modules are wired in.

### 4.3 Versioning, changelog, packaging

- [Cargo.toml](Cargo.toml): `version = "0.1.1"`.
- [CHANGELOG.md](CHANGELOG.md): new section per the patch list.
- [packaging/homebrew/inspect.rb](packaging/homebrew/inspect.rb): bump version, sha256 placeholder for release CI.
- [scripts/install.sh](scripts/install.sh): version bump if hardcoded.

---

## 5. Implementation order (concrete checklist)

| # | Patch | Touches | Blocker for |
|---|---|---|---|
| 1 | **P2** Phantom services | profile schema, discovery, resolver, dispatch | nothing — go first |
| 2 | **P8** help fallback | help, cli | docs reference future verbs cleanly |
| 3 | **P11** exit code surfacing | error.rs, output.rs | P6 needs Inner(i32) variant |
| 4 | **P1** `--follow` streaming | ssh/exec, verbs/logs, exec/cancel | P3, P5, P10 |
| 5 | **P3** `--match` / `--exclude` | line_filter, logs, grep | P5 (merged + match composes) |
| 6 | **P4** secret masking | redact.rs | P6 |
| 7 | **P6** split run/exec | new run.rs, cli.rs, write/exec.rs | P7, P12 |
| 8 | **P7** collapse `--allow-exec` | write/exec.rs (cleanup tail of P6) | — |
| 9 | **P12** `--reason` | audit.rs, cli.rs, all write verbs | — |
| 10 | **P10** `--since-last` cursor | paths, new cursor.rs, logs, grep | — |
| 11 | **P5** `--merged` | logs, output | needs P1 + P3 |
| 12 | **P9** progress indicator | new progress.rs, logs, grep, search | — |
| 13 | **P13** discovery fallback | discovery/probes, commands/setup | — |
| 14 | docs/help/changelog/version | help content, MANUAL, README, Cargo, CHANGELOG | release |

Steps 1-3 unblock 4. Steps 4-6 unblock 7-8. After 8, the rest (9, 10, 11, 12, 13) are independent and can land in any order.

---

## 6. Exit criteria (mirror of spec §10, made executable)

Each criterion below maps to a specific test that must pass on `main` before tagging `v0.1.1`:

| # | Criterion | Test |
|---|---|---|
| 1 | `inspect logs arte/ws-bridge --follow --match "error"` streams in real time | `phase4_read_verbs::follow_with_match_streams_incrementally` |
| 2 | `inspect logs arte/a,b,c --follow --merged` interleaves | `phase4_read_verbs::merged_follow_interleaves_three_streams` |
| 3 | `inspect run arte/api -- env` runs, masks secrets, no `--apply` | `phase7_exec::run_masks_api_keys` |
| 4 | `inspect exec arte/db -- 'psql ...' --apply` works without `--allow-exec` | `phase7_exec::exec_apply_alone_is_sufficient` |
| 5 | `inspect logs arte/worker --since-last` polls without dupes/gaps | `phase4_read_verbs::since_last_persists_cursor` |
| 6 | Every `inspect ps` name resolves in `inspect logs` | `phase2_discovery::ps_names_round_trip_to_logs` |
| 7 | `inspect help logs` shows logs help (not "unknown topic") | `help_contract::help_command_fallback_works_for_every_verb` |
| 8 | Slow log fetches show progress within 2s | `phase9_diagnostics::slow_logs_show_progress_indicator` |
| 9 | `inspect audit ls` shows `--reason` text | `phase5_write_verbs::reason_flag_appears_in_audit_ls` |
| 10 | Zero `--allow-exec` needed in workflow | grep audit log of CI smoke test for absence of the flag |

When all 10 pass on a clean checkout, tag `v0.1.1`.
