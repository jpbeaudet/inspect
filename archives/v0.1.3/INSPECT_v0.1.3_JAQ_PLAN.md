# Inspect v0.1.3 — `jaq` integration plan (F19)

**Status:** OPEN. Crash-resilient implementation plan for the
`v0.1.3` release-smoke follow-on item. Read this top-to-bottom on
session resume; every commit is self-verifiable so a fresh agent
can detect "where am I" without re-asking the user.

**One-line goal:** make every `| jq …` recipe in `inspect`'s help
text, manuals, runbook, and smoke document work as a first-class
`--select '<filter>'` flag against the inspect binary itself, so a
fresh-install agent never needs an external `jq` to consume any
example we ship.

**Authority + decision trace:**

- The dependency itself is pre-approved — see `CLAUDE.md` →
  *Dependency Policy* → *`jaq` rationale (2026-05-05)* and the
  `Cargo.toml` comment block above the `jaq-core` / `jaq-std` /
  `jaq-json` lines.
- The implementation was originally a **v0.1.5** stabilization
  candidate. It was promoted into **v0.1.3** mid-release-smoke
  because the help system, MANUAL, RUNBOOK, and SMOKE document
  collectively contain ≈30 `| jq` recipes that an agent on a
  fresh box cannot run, which directly contradicts the
  *"help text is API for agentic callers"* invariant in
  `CLAUDE.md`. The maintainer authorized the promotion in this
  conversation (2026-05-05).
- This is **F19** — the 31st backlog item, smoke-follow-on
  branch. Same `F<n>` style as F1–F18 because the trigger was
  field-equivalent (release-smoke driving the binary on a real
  host).

**Scope rules carried over from `CLAUDE.md`:**

- No silent deferrals. Every sub-item below ships in this
  release; sequencing across the four commits is sequencing,
  not deferring.
- No backwards-compat shims. `--select` is a brand-new flag;
  no legacy `--jq` / `--filter` aliases.
- Pre-commit gate (`cargo fmt --check && cargo clippy
  --all-targets -- -D warnings && cargo test`) must be clean
  before every commit. WSL `cargo` PATH prefix
  `export PATH="$HOME/.cargo/bin:$PATH"` applies to every
  invocation in this plan.
- Help-surface discipline: every new flag, JSON shape, and
  exit-code contract added below has matching `-h` text and
  editorial topic content **in the same commit** — not a
  follow-up.
- Contracts of `OutputDoc` / `Envelope` (audit JSON shape) and
  exit codes 0 / 1 / 2 / 12–14 do not change. `--select` is
  applied **after** envelope assembly, so the existing
  envelope contract is untouched on the no-`--select` path.

---

## Pre-state (verify on session resume)

Run these before writing any code; they tell you which commit
the previous attempt got to. Do **not** assume the working tree
matches what this document says — read the actual diff.

```sh
# 0. WSL cargo path (always)
export PATH="$HOME/.cargo/bin:$PATH"

# 1. Deps already added (commit predating this plan).
grep -E '^jaq-(core|std|json)' Cargo.toml
# Expected: three lines pinned to "3" / "3" / "2".

grep -E '^name = "jaq-' Cargo.lock
# Expected: jaq-core, jaq-json, jaq-std all present.

# 2. Where are we in the four-commit sequence?
test -d src/query && echo "C1: src/query/ exists — likely landed"
grep -RnE 'pub fn eval' src/query 2>/dev/null | head -3

grep -RnE '"select"|--select' src/cli.rs | head -3
# Any hit ⇒ C2 likely started.

grep -RlE '\| jq ' src/help/content/ src/help/verbose/ \
  docs/MANUAL.md docs/RUNBOOK.md docs/SMOKE_v0.1.3.md \
  src/cli.rs 2>/dev/null
# C3 sweep is done when this list contains AT MOST the
# editorial "external jq still works" pointer in
# src/help/content/select.md (or wherever C3 lands it).

grep -nE '^\| F19' INSPECT_v0.1.3_BACKLOG.md
# C4 done ⇒ F19 row exists with status ✅ Done.
```

If a previous attempt got partway and crashed, **always**
re-read the latest `git diff HEAD` and the four post-condition
checklists below before continuing. Test names (`f19_*`) and
module paths are authoritative signals of which commit is in
flight.

---

## High-level design

### `--select` semantic contract

- Surface: a new flag on every JSON-emitting verb, named
  `--select` (long only — no short alias; jq-language argument
  is never confusable with anything else, and this leaves
  `-s` free for verb-local single-letter flags).
- Argument: a jq-language filter string, identical syntax to
  `jq` 1.7. `inspect <verb> --json --select '<filter>'` is
  contract-equivalent to `inspect <verb> --json | jq -c
  '<filter>'` for any filter that doesn't slurp.
- Output: compact JSON, one document per line — same as
  `jq -c`. Multiple yielded values become multiple lines, in
  filter-order.
- Companion flag: `--select-raw` (long only). Equivalent to
  `jq -r`: when the filter result is a JSON string, emit the
  unquoted string. Non-string yields error to stderr and exit
  1. Mutex with nothing (compatible with `--select` on its
  own — applied at format time).
- Slurp mode: `--select-slurp`. Equivalent to `jq -s`:
  collect *all* envelope inputs into an array before applying
  the filter. Only meaningful for NDJSON-streaming verbs
  (`logs`, `grep --json`, `audit ls --history`,
  `run --stream`); on single-envelope verbs it is accepted
  and treated as a no-op wrap (`[.]`) so recipes don't have
  to special-case verb shape.
- Mutex: `--select` requires `--json` (clap-level
  `requires = "json"`). `--select` is incompatible with
  `--quiet` (clap-level `conflicts_with = "quiet"`) — same
  reason the existing `--quiet`/`--json` mutex exists.
- Error mapping:
  - Filter parse error → exit **2** (clap-class usage error),
    stderr line `error: --select filter: <jaq parse message>`
    with a `hint:` pointing at `inspect help select`.
  - Filter runtime error → exit **1** (no-match class, same
    as a verb whose query produced zero rows), stderr line
    `error: --select runtime: <message>` with the same
    `hint:`.
  - Filter runs but produces zero outputs → exit **1**, no
    stdout. (Same as `jq` with no values yielded.)
  - When `--select-raw` is set and a non-string is yielded →
    exit 1 with `error: --select-raw: filter yielded
    non-string at result <n>; remove --select-raw or wrap
    with tostring`.

### Where `--select` is applied

- Single-envelope verbs (`status`, `why`, `ps`, `ls`,
  `health`, `find`, `cat` in JSON mode, `ports`, `network`,
  `volumes`, `images`, `search`, `audit ls/show/grep/gc/
  verify`, `compose ls/ps/up/down/build/pull/restart/config`,
  `bundle`, `recipe`, `connect`, `cache ls/clear`,
  `discover`, `connectivity`, `setup --json`,
  `history show --json`): apply at the single
  `OutputDoc::print_json` call site, intercepting the
  `serde_json::Value` before serialization.
- NDJSON-streaming verbs (`logs`, `grep --json` row stream,
  `run --stream` line frames, `audit grep` history-line
  stream, `bundle --stream` step frames): apply per
  emitted line, after envelope encoding but before
  `transcript::emit_stdout`. Slurp mode collects every
  line, then evaluates once at end-of-stream.
- The application points are **two files**:
  `src/verbs/output.rs::OutputDoc::print_json` (envelope
  case) and a new helper `src/query::ndjson::filter_line`
  used by the streaming verbs' emit sites. There is **no
  cross-cutting wrapper trait** — explicit call sites only.
  This matches the existing F7.4 `--quiet` plumbing pattern
  (no global state, no implicit interception).

### `src/query/` module shape

```text
src/query/
  mod.rs         — public API + QueryError
  jaq.rs         — thin jaq-core / jaq-std / jaq-json wrapper;
                   the only file that names jaq types directly
  ndjson.rs      — per-line filter helper for streaming verbs
  raw.rs         — render::value_to_raw (jq -r equivalent)
  tests.rs       — module-level integration tests
```

Public API (every other module imports only these):

```rust
pub use self::error::{QueryError, QueryErrorKind};

/// Parse + evaluate `filter` against `input`. Returns every
/// yielded value in filter order. Empty result is success.
pub fn eval(filter: &str, input: &serde_json::Value)
    -> Result<Vec<serde_json::Value>, QueryError>;

/// Slurp variant: input is a Vec; filter sees `.` as that array.
pub fn eval_slurp(filter: &str, inputs: &[serde_json::Value])
    -> Result<Vec<serde_json::Value>, QueryError>;

/// Pre-parse a filter once for streaming verbs that will
/// evaluate it line-by-line. Returns an opaque handle.
pub fn compile(filter: &str) -> Result<Compiled, QueryError>;
pub fn eval_compiled(c: &Compiled, input: &serde_json::Value)
    -> Result<Vec<serde_json::Value>, QueryError>;

/// `jq -r` rendering: JSON string → unquoted UTF-8;
/// any other type → error.
pub fn render_raw(values: &[serde_json::Value])
    -> Result<String, QueryError>;

/// Compact rendering (one value per line, `jq -c`).
pub fn render_compact(values: &[serde_json::Value]) -> String;
```

Future swap-out (jaq → libjq → handwritten subset → other):
mechanical, because every jaq type stays inside
`src/query/jaq.rs`. The `query::eval` signature is what every
verb sees.

### Help-system integration

This is the **load-bearing reason** to ship F19 in v0.1.3
rather than waiting for v0.1.5. The fix has three independent
beats:

1. **Replace `| jq` recipes** in `src/help/content/*.md`,
   `src/help/verbose/*.md`, `src/cli.rs` `LONG_*`,
   `docs/MANUAL.md`, `docs/RUNBOOK.md`, `docs/SMOKE_v0.1.3.md`,
   and `README.md` with `--select '<filter>'` recipes. Same
   filter strings — agents already know the syntax, no
   relearning. Where the original was `jq -r '<filter>'`,
   the new form is `--select '<filter>' --select-raw`.
2. **Add the editorial topic** `src/help/content/select.md`
   covering: jq language summary, ten common filters used in
   inspect recipes (`.summary`, `.data.entries[0].id`,
   `.data | length`, `[.data[] | select(.healthy)]`,
   `map(.name)`, `.data.services[] | {name, state}`,
   `keys`, `to_entries`, `..|select(.passphrase?)?`,
   `reduce …`), a "common pitfalls" block (single quotes
   on shells, `select`-the-flag vs `select`-the-jq-builtin,
   `--select-raw` only on string-yielding filters), and
   the explicit "external `jq` still works" sentence. Topic
   gets registered in `src/help/topics.rs` and indexed by
   `src/help/search.rs`.
3. **Discovery probe** (`src/discovery/probes.rs`): the
   existing `jq` probe stays — operators may still pipe to
   external `jq`, and the probe's positive result is
   informational. The probe's *negative* result no longer
   carries any "you need this for inspect recipes" implication
   in the prose; reword the diagnostic so absence is fine. The
   `discovery.md` topic narrative gets a corresponding
   one-line update ("`jq` is **optional** — every recipe in
   this manual works via `--select` on the inspect binary
   itself").

### Help-search index cap

Adding `select.md` (~6 KB) plus the `LONG_SELECT` constant
plus per-verb mentions in `LONG_*` constants will push the
help search index size up. If the v0.1.3 cap (80 KB per
`CLAUDE.md` precedent) trips, raise it to 96 KB in
`src/help/search.rs`. **Do not trim documentation to fit** —
this is the same precedent as v0.1.2 (50→64 KB) and v0.1.3
(64→80 KB) under L7 / F4. Document the bump in the commit
body.

---

## Commit plan (4 commits)

Each commit has: a one-line subject, a sub-section breakdown,
explicit acceptance tests, and a **post-conditions checklist**
that a fresh agent can run to verify completion before moving
on. Commits land in the order C1 → C1-fixup → C2 → C3 → C4
on branch `v0.1.3-jaq`. The branch is merged back to `v0.1.3`
once C4 is green. **Do not bundle**; each commit must leave
the tree green and shippable.

### Branch model

All F19 work lives on `v0.1.3-jaq`, branched from `v0.1.3`
after the prep commit `432b091` ("F19: prep — jaq dependency,
rationale, and 4-commit implementation plan"). Final step
after C4 is `git checkout v0.1.3 && git merge --ff-only
v0.1.3-jaq` (or `--no-ff` if a merge-commit boundary is
desired). The reason for the branch is to keep partial F19
state off `v0.1.3` — `inspect query` (from C1) without
`--select` on the rest of the verbs (C2-C4) is an inconsistent
contract; the full feature must land atomically from `v0.1.3`'s
point of view.

### C1-fixup commit (between C1 and C2)

The C1 commit `ff0d7a3` shipped `commands/query.rs` with
filter-error stderr lines emitted via raw `eprintln!` rather
than through `crate::error::emit`. That passes the literal CI
check (`tests/error_help_links.rs::no_raw_error_eprintln_outside_error_module`
matches the substring `eprintln!("error:`, and the C1 messages
start with the label, not `error:`), but violates the
*spirit* of the contract: every user-facing error in inspect
should render in the canonical `error: <msg>` + `see: inspect
help <topic>` shape so agents can cross-link consistently.

A separate fix-up commit on `v0.1.3-jaq` (rather than `git
commit --amend` per CLAUDE.md "always create NEW commits"
policy) reroutes the three filter-error stderr lines through
`error::emit`. The messages are normalized so `error::emit`'s
`error:` prefix doesn't double up — labels become "filter
parse:", "filter runtime:", "filter --raw:" (not "filter
runtime error:"), so the rendered output is "error: filter
parse: …".

The catalog entry that gives these errors a `see: inspect help
select` link is C3's responsibility (it ships the `select`
topic in the same commit; adding the catalog row in C2 would
break `tests::every_catalog_row_points_at_a_real_topic_or_none`
because the topic doesn't exist yet).

C1-fixup commit subject: `F19 (C1-fixup): route filter errors
through error::emit for help-topic linkage`. Footer: `Closes
the C1 stderr-routing oversight on v0.1.3-jaq.`

The crate-wide lint `dead_code = "deny"` (see
`Cargo.toml::[lints.rust]`) means every commit's new `pub`
surface needs a non-test caller in the same commit. Suppressing
the lint with `#[allow(dead_code)]` is **not** an acceptable
workaround. This shapes the C1 / C2 boundary below — C1 ships
the abstraction together with its first natural caller (`inspect
query`), and C2 layers `--select` onto every other JSON-emitting
verb as additive sugar.

### Commit C1 — `query::` module + `inspect query <filter>` verb

**Subject:** `F19: query module — jaq filter engine + inspect
query verb`

**Scope:**

- Create `src/query/` with the layout described in the design
  section above (`mod.rs`, `jaq.rs`, `ndjson.rs`, `raw.rs`,
  `tests.rs`).
- Wire `mod query;` into `src/main.rs` (alphabetical
  position between `profile` and `redact`).
- Implement `query::eval` / `eval_slurp` / `compile` /
  `eval_compiled` / `render_raw` / `render_compact` against
  jaq-core 3.x + jaq-std 3.x + jaq-json 2.x. Use
  `jaq_core::load::Arena` for parsing, `jaq_std::funs()` for
  the standard library, `jaq_json::Val` for the
  `serde_json::Value` ↔ jaq value bridge.
- `QueryError` enum (`Parse` / `Runtime` / `RawNonString`)
  with `Display` carrying operator-readable messages.
- **First natural caller: a new `inspect query <filter>`
  verb** that reads JSON or NDJSON from stdin and applies a
  jq-language filter. This is the smallest possible
  consumer that activates every public API in `query::`
  (compile, eval, eval_slurp, render_compact, render_raw,
  ndjson::Filter — all wired through this one verb), so the
  `dead_code` lint stays clean. The verb is also useful in
  its own right for an agent: it can pipe any saved
  envelope or any `--json` output through a filter without
  requiring an external `jq` install. C2 layers `--select`
  onto the rest of the JSON-emitting verbs as additive
  sugar over the same machinery — `inspect query` continues
  to exist after C2 as the escape hatch for ad-hoc filtering
  of arbitrary stdin JSON.
- `inspect query` surface (built in this commit):
  - Argument: `<FILTER>` — single jq-language string.
  - Flags: `--raw` / `-r` (= `jq -r`), `--slurp` / `-s` (=
    `jq -s` — collects every NDJSON value from stdin into
    one array first), `--ndjson` (per-line streaming mode;
    default detection is "single JSON value if stdin parses
    as one, NDJSON otherwise"). When `--ndjson` is explicit
    + not `--slurp`, the verb runs `query::ndjson::Filter`
    on every line.
  - Stdin handling: read to end (capped at `INSPECT_QUERY_STDIN_MAX`
    env var, default 16 MiB — same shape as F9's `--stdin-max`
    contract). Empty stdin → exit 2 (usage error: "no JSON on
    stdin"). Non-JSON stdin → exit 2 with the parse-error byte
    offset.
  - Output: compact JSON, one document per yielded value,
    via `transcript::emit_stdout` so transcripts capture
    the filter output (not the unfiltered input).
  - Exit codes: 0 = filter produced ≥ 1 result. 1 = filter
    produced zero results, or runtime error, or
    `--raw`-on-non-string error. 2 = filter parse error, or
    stdin not parseable as JSON, or empty stdin, or clap
    arg error.
  - `LONG_QUERY` constant in `src/cli.rs` documenting the
    contract (jq-syntax pointer, exit-code table, three
    worked examples covering plain / raw / slurp).
- **No `--select` flag yet on other verbs.** That is C2's
  whole job; C1 sticks to landing the abstraction + its
  first-class caller.

**Acceptance tests:**

- Module unit tests in `src/query/tests.rs` (in-tree;
  filter exercises taken from the recipe set the C3 sweep
  will migrate):
  - `identity_returns_input`, `path_extraction`,
    `array_iteration`, `select_with_predicate`,
    `length_on_array_and_object`, `keys_sorted`,
    `map_then_unique`, `null_safe_path`,
    `parse_error_classified`, `runtime_error_classified`,
    `slurp_collects_all`, `compile_then_eval_three_lines`,
    `render_raw_string_unquoted`,
    `render_raw_non_string_errors`,
    `render_compact_one_per_line`,
    `render_compact_empty_yields_empty_string`,
    `ndjson_per_frame_compact`, `ndjson_per_frame_raw`,
    `ndjson_slurp_length`,
    `ndjson_parse_error_at_construction`,
    `recipe_audit_ls_first_id`,
    `recipe_status_state_and_count`,
    `recipe_compose_ls_project_names`.
- Integration tests in `tests/jaq_query_v013.rs` (new
  file, `f19_query_*` prefix) covering the verb itself:
  - `f19_query_identity_roundtrip` — pipe a known JSON
    object to `inspect query '.'`, assert stdout equals
    input (modulo whitespace).
  - `f19_query_path_extraction` — `echo '{"a":{"b":1}}'
    | inspect query '.a.b'` exits 0, stdout `1`.
  - `f19_query_raw_string` — `echo '{"s":"hi"}' | inspect
    query -r '.s'` exits 0, stdout `hi\n`.
  - `f19_query_raw_non_string` — `echo '{"n":3}' | inspect
    query -r '.n'` exits 1, stderr matches "non-string".
  - `f19_query_ndjson_per_frame` — three NDJSON frames in
    on stdin, `inspect query --ndjson '.line'` emits three
    lines.
  - `f19_query_slurp_length` — three NDJSON frames in,
    `inspect query --slurp 'length'` exits 0, stdout `3`.
  - `f19_query_parse_error_exit_2` — `inspect query '.['
    < /dev/null` (or with a valid stdin) exits 2 with a
    parse-error stderr line.
  - `f19_query_zero_results_exit_1` — filter that yields
    nothing (`.[] | select(false)`) → exit 1, no stdout.
  - `f19_query_empty_stdin_exit_2` — empty stdin → exit 2
    + usage hint.
  - `f19_query_runtime_error_exit_1` — `1 + "x"` → exit 1,
    stderr matches "runtime".

**Post-conditions checklist (run on resume):**

```sh
export PATH="$HOME/.cargo/bin:$PATH"
test -d src/query                                               # ✓
ls src/query/{mod,jaq,ndjson,raw,tests}.rs >/dev/null           # ✓
grep -q '^mod query;' src/main.rs                               # ✓
inspect query --help 2>&1 | head -5                             # contract visible
echo '{"x":1}' | inspect query '.x'                             # stdout: 1
echo '{"s":"hi"}' | inspect query -r '.s'                       # stdout: hi
cargo test --lib query:: 2>&1 | grep -E '^test result'          # all pass
cargo test --test jaq_query_v013 2>&1 | grep -E '^test result'  # all pass
cargo clippy --all-targets -- -D warnings 2>&1 | tail -5        # clean
cargo fmt --check                                               # clean
```

**Update sweep for C1:** source + tests + `LONG_QUERY` help
text. CHANGELOG / backlog row stay held for C4 (single F19
bullet covers all four commits, matching the F11 / F17
multi-commit precedent).

---

### Commit C2 — `--select` / `--select-raw` / `--select-slurp` flags wired onto every JSON-emitting verb

**Subject:** `F19: --select / --select-raw / --select-slurp on
every JSON-emitting verb`

**Pre-condition:** C1 landed; `query::*` is callable from any
verb; `inspect query` smoke-tests pass. C1-fixup landed; filter
errors route through `error::emit`. C2 is purely additive sugar
on top of the existing abstraction — no new public API in
`src/query/` is added in C2.

### Design divergence from the original C2 spec

The original C2 scope said "define a shared clap arg group
`SelectArgs` and add it to every verb args struct that has
`pub json: bool`". A pre-implementation survey of the codebase
(see `Agent` survey transcript in this session) found that
shape doesn't match reality:

- Only **3 places** in `src/` declare `pub json: bool` directly
  (`HelpArgs` and `FleetArgs` in `src/cli.rs`, and
  `FormatArgs::json` in `src/format/mod.rs`).
- Every other JSON-emitting verb inherits `--json` through
  `#[command(flatten)] pub format: FormatArgs` — 30+ verbs.
- JSON emission flows through **three chokepoints** in
  `src/verbs/output.rs`, not 30 per-verb sites:
  - `OutputDoc::print_json` (line 244) — 15 envelope verbs
    (status, health, audit ls/show/grep, compose ls/ps,
    cache, why, connectivity, recipe, search, setup-derived).
  - `JsonOut::write` (line 164) — 10 streaming verbs (logs,
    grep, run --stream, history show, journal-style emitters,
    ps/find/ls/network/images/volumes/ports per-record paths).
  - `Renderer::dispatch` JSON branch (line 105-110) — 6 per-
    record verbs that buffer rows then emit them. Note: this
    branch currently uses `println!` rather than
    `transcript::emit_stdout`, **bypassing the transcript**.
    This is a pre-existing tech-debt gap; C2 fixes it as part
    of the same change set per the LLM-trap-fix-on-first-
    surface + sweep-the-pattern policy in CLAUDE.md.

The cleaner shape — adopted for C2 — is:

1. Add `select`, `select_raw`, `select_slurp` to **`FormatArgs`
   itself** (not a new struct). 30+ verbs get the flags
   automatically via the existing `#[command(flatten)]`. Two
   exceptions (`HelpArgs`, `FleetArgs`) get the same fields
   added directly to their structs to keep the surface
   uniform.
2. Plumb the filter through the three chokepoints, not
   per-verb.
3. Two top-level commands with bespoke serialization
   (`fleet`, `help --json`) plus `setup`'s own `print_json`
   get filter integration at the same time so the contract
   is uniform across every JSON-emitting verb. **Do not skip
   them** — partial coverage is a worse contract than no
   coverage (agents would learn `--select` works on most
   verbs and be surprised when it silently no-ops on others).

### Scope

**Flags + validation (`src/format/mod.rs`):**

- Add three fields to `FormatArgs`:
  - `select: Option<String>` (`--select <FILTER>`) — the jq-
    language filter source.
  - `select_raw: bool` (`--select-raw` / clap `requires =
    "select"`) — emit string yields unquoted (`jq -r`).
  - `select_slurp: bool` (`--select-slurp` / clap `requires
    = "select"`) — collect every NDJSON value into one
    array first (`jq -s`).
  - `--select-raw` and `--select-slurp` are independently
    settable (combinable, `jq -rs`-equivalent).
- Validation in `FormatArgs::resolve()`: if `select.is_some()`
  AND the resolved format is not JSON-class (`--json` /
  `--jsonl`), return `Err(anyhow!("--select requires --json
  or --jsonl"))`. Routes through `ExitKind::Error` → exit 2
  with the canonical `error: …` shape.
- `--quiet` ⊕ `--select` is enforced transitively: `--quiet`
  is already `conflicts_with_all = ["json", "jsonl"]` at
  clap level, and `--select` requires JSON-class via
  `resolve()`, so the combination is rejected.
- New helper `FormatArgs::select_filter() ->
  anyhow::Result<Option<query::ndjson::Filter>>` constructs a
  Filter for streaming verbs in one place. Parse errors are
  mapped to "`--select filter: <message>`" anyhow errors so
  they route through `error::emit` → exit 2 (catalog entry
  shipped in C3).

**OutputDoc plumbing (`src/verbs/output.rs`):**

- Change `OutputDoc::print_json` signature from
  `(&self) -> ()` to `(&self, select: Option<&str>, raw: bool,
  slurp: bool) -> anyhow::Result<crate::error::ExitKind>`.
  - `select == None`: serialize self → `transcript::emit_stdout`,
    return `Ok(Success)` (existing behavior, now just
    explicit-Result).
  - `select == Some(filter)`:
    - Serialize self to `serde_json::Value`.
    - Run `query::eval(filter, &value)` (or `eval_slurp` if
      `slurp == true`).
    - On parse error: return `Err(anyhow!("filter parse: …"))`
      → routes through `error::emit` → exit 2.
    - On runtime error or raw-non-string: emit through
      `error::emit` directly, return `Ok(NoMatches)` → exit 1.
    - On zero results: return `Ok(NoMatches)` → exit 1.
    - On success: render via `query::render_raw` (if `raw`)
      or `render_compact`, emit through
      `transcript::emit_stdout`, return `Ok(Success)`.
- Update every caller to thread the return value through to
  `main`'s `ExitKind` plumbing (most verbs already return
  `anyhow::Result<ExitKind>` — this is a small mechanical
  change at each call site).

**JsonOut streaming plumbing (`src/verbs/output.rs`):**

- Change `JsonOut::write` signature from `(env: &Envelope) ->
  ()` to `(env: &Envelope, filter: Option<&mut
  query::ndjson::Filter>) -> anyhow::Result<()>`.
  - Filter `None`: serialize → emit (existing behavior).
  - Filter `Some(f)`:
    - Convert env to `serde_json::Value`.
    - Call `f.on_line(&value)?`.
    - If returned string is non-empty: emit through
      `transcript::emit_stdout`. (Slurp mode returns empty;
      output is deferred to end-of-stream.)
- Each streaming verb call site:
  - Constructs `let mut select = args.format.select_filter()?`
    once at verb entry.
  - Replaces `JsonOut::write(&env)` with `JsonOut::write(&env,
    select.as_mut())?` in the per-frame loop.
  - At end-of-stream, if `select` is Some: call
    `f.finish()?`; emit if non-empty.
- The runtime-error and raw-non-string error paths in
  `Filter::on_line` and `Filter::finish` already return
  `Err(QueryError)`; the streaming wrapper translates these
  to `error::emit` + early exit (`ExitKind::NoMatches` for
  runtime/raw, `ExitKind::Error` for parse — but parse errors
  are caught at construction time in `select_filter`, so
  the streaming `on_line`/`finish` path only ever sees
  runtime/raw errors).

**Renderer::dispatch fix + plumbing (`src/verbs/output.rs`):**

- Replace `println!("{}", serde_json::to_string(row)?)` with
  the same filter-aware emission path used by `JsonOut::write`.
  - One construction of `Filter` at dispatch entry.
  - Per-row: `JsonOut::write(&env_for_row, filter.as_mut())?`
    semantics (extracted into a private helper to avoid
    duplication).
  - End-of-stream `finish()` flush.
- This **also fixes the pre-existing transcript-bypass gap**
  (line 107). Per CLAUDE.md "fix on first surface" + "sweep
  the pattern", the gap is in the same family as the new
  filter plumbing — both are "JSON output goes through
  transcript and through filter if set" — and gets fixed in
  the same commit.

**Bespoke JSON emitters (NOT skipped):**

- `fleet::emit_json` (`src/commands/fleet.rs:807`): currently
  hand-rolls JSON via `println!`. Refactor to build a
  `serde_json::Value` envelope, then route through the same
  filter-aware emission path used by `OutputDoc::print_json`.
  Adds `--select` support uniformly without changing fleet's
  output schema.
- `help::json::render_full` / `render_topic`: currently
  pre-renders the help registry to a static JSON string.
  Wrap the rendered string: parse to Value, apply filter if
  set, emit. Single-shot, no streaming concerns. Lets agents
  do `inspect help all --json --select '.topics[].name'` for
  discovery.
- `setup::print_json` (`src/commands/setup.rs:210`): the
  cleanest fix is migrating to `OutputDoc::print_json`
  (consistency with all other envelope verbs); fall back to
  local filter integration if migration is too invasive for
  C2's scope. Decision made at implementation time based on
  the actual diff shape.

**Per-verb help (`src/cli.rs` LONG_* constants):**

- Add a one-line `SELECTING:` pointer to each verb's
  `LONG_*` block. Format:
  ```
  SELECTING
    Use `--select '<jq filter>'` to extract a field or shape
    the output. Examples:
      $ inspect <verb> [args] --json --select '.summary' -r
      $ inspect <verb> [args] --json --select '.data | length'
    See `inspect help select` for the filter language.
  ```
  Match the existing `LONG_AUDIT_LS` "ORDERING + JSON
  PROJECTION" precedent. Don't write a paragraph per verb —
  the editorial topic in C3 carries the depth.

**Exit-code contract (stable from C2):**

- `0`: filter produced ≥ 1 result.
- `1`: filter produced zero results, runtime error, or
  raw-non-string error.
- `2`: filter parse error, `--select` without JSON-class
  format, clap arg error.

Identical to `inspect query`'s exit-code shape from C1, so
agents only learn one contract.

**Catalog entry deferral note:** the `ERROR_CATALOG` row that
gives filter errors a `see: inspect help select` cross-link
ships in **C3**, not C2. The reason is that the
`every_catalog_row_points_at_a_real_topic_or_none` test
requires the topic to exist, and the `select` editorial topic
ships in C3. C2's filter errors render as `error: --select …`
without a `see:` line; C3 lights it up. **This is not a
deferral** — it's a sequencing dependency between two commits
that ship in this same release. Re-routing to a generic
existing topic (e.g. `formats`) was considered but rejected:
the right link is `select`, and a transient one-commit
window without it is cleaner than a misleading link that
gets re-targeted in C3.

**Acceptance tests** (new test file
`tests/jaq_select_v013.rs` — separate from
`phase_f_v013.rs` because it cuts across every verb and
keeping the cross-cut visible aids future refactors):

Per-chokepoint coverage:

- `f19_outputdoc_select_summary` (envelope chokepoint) —
  any `OutputDoc`-emitting verb (e.g. `audit ls --json
  --select '.summary'`); assert single compact JSON string.
- `f19_outputdoc_select_raw` — same with `--select-raw`,
  assert unquoted UTF-8.
- `f19_outputdoc_select_data_path` — `audit ls --json
  --select '.data.entries[0].id'`; proves the L7 envelope
  path is reachable.
- `f19_outputdoc_select_array_length` — `--select
  '.data.entries | length'`; assert numeric.
- `f19_jsonout_streaming_per_frame` — `inspect history
  show <id> --json --select '.line'` (or any streaming
  verb without a remote dependency); assert N lines.
- `f19_jsonout_streaming_slurp` — same with
  `--select-slurp 'length'`; assert single numeric line.
- `f19_renderer_dispatch_per_row` — any per-record verb
  using `Renderer::dispatch` (e.g. `inspect ps --json
  --select '.service'`); assert per-row filter is applied.
- `f19_renderer_dispatch_transcript_capture` — same call
  with `INSPECT_TRANSCRIPT=…`; assert the transcript file
  contains the post-filter output (proves the
  `println` → `emit_stdout` fix landed and the filter
  output is what's audited, consistent with the existing
  `--quiet` precedent).

Bespoke-emitter coverage:

- `f19_fleet_select_envelope` — `inspect fleet status
  --json --select '.namespaces | length'` against a test
  fleet; assert numeric.
- `f19_help_json_select_topic_names` — `inspect help all
  --json --select '.topics[].id'`; assert ≥ 1 line.
- `f19_setup_select_namespace_field` (if migration to
  OutputDoc happens in C2) or
  `f19_setup_select_local_filter` (if local plumbing) —
  whichever shape lands.

Validation + error-class coverage:

- `f19_select_requires_json_format` — `inspect status
  --select '.x'` (no `--json`), assert exit 2 + stderr
  contains `--select requires --json or --jsonl`.
- `f19_select_with_csv_errors` — `inspect status --csv
  --select '.x'`, same exit 2.
- `f19_select_quiet_conflict_via_json` — `--json --quiet`
  is already mutex; `--quiet --select '.x'` falls through
  to "select requires JSON" error. Assert exit 2.
- `f19_select_parse_error_exit_2` — `--select '.['`;
  assert exit 2 + stderr matches `error: --select filter:`
  (no `see:` line in C2; C3 adds it).
- `f19_select_runtime_error_exit_1` — `--select '1 +
  "x"'`; assert exit 1 + stderr matches `error:
  --select runtime:`.
- `f19_select_zero_results_exit_1` — filter yielding no
  values; assert exit 1 + empty stdout.
- `f19_select_raw_non_string_errors` — `--select '.count'
  --select-raw` against an envelope with numeric `.count`;
  assert exit 1 + stderr matches `error: --select-raw:`
  `filter yielded non-string`.
- `f19_select_raw_requires_select` — `--select-raw`
  alone (no `--select`); assert clap requires-error,
  exit 2.
- `f19_select_slurp_requires_select` — same shape for
  `--select-slurp`.

Help discoverability:

- `f19_status_help_mentions_select` — `inspect status
  --help | grep -i 'select'`; assert non-empty match
  (proves the `LONG_*` SELECTING pointer landed).
- `f19_audit_ls_help_mentions_select` — same for `audit
  ls`.

Total ≈ 18-20 tests. The test file has helpers for
constructing test envelopes / temp-dir audit logs / fake
servers so tests don't depend on a real SSH host.

**Pre-commit gate:** full suite must be ≤ 28 + 1 = 29 suites
green (the new test file). The ttl-zero flake described in
`CLAUDE.md` is acknowledged; one re-run is allowed. Total
test count grows by ~14.

**Post-conditions checklist (run on resume):**

```sh
export PATH="$HOME/.cargo/bin:$PATH"
grep -nE 'pub select(_raw|_slurp)?:' src/format/mod.rs           # 3 lines
grep -nE 'select_filter' src/format/mod.rs src/verbs/output.rs   # helper + uses
grep -nE 'select.*:.*Option<.*str>' src/cli.rs                   # HelpArgs+FleetArgs additions
grep -n 'transcript::emit_stdout\|println!' src/verbs/output.rs  # NO println for JSON branch
inspect status --help 2>&1 | grep -i 'select'                    # SELECTING line present
inspect audit ls --help 2>&1 | grep -i 'select'                  # SELECTING line present
inspect fleet status --help 2>&1 | grep -i 'select'              # SELECTING line present
inspect help all --help 2>&1 | grep -i 'select'                  # SELECTING line present
cargo test --test jaq_select_v013 2>&1 | grep -E 'test result'   # all pass
cargo test 2>&1 | grep -E '^test result|^running' | tail -40     # clean
```

**Update sweep for C2:** code + per-verb `LONG_*` adjustments
land here; CHANGELOG / backlog / `select.md` topic / MANUAL /
RUNBOOK / SMOKE happen in C3 + C4. The `-h` surface (one
line per verb) lands in C2 because clap arg docstrings live
on the same struct definitions.

---

### Commit C3 — Help / MANUAL / RUNBOOK / SMOKE / README sweep: every `| jq` recipe → `--select`; new `select` editorial topic

**Subject:** `F19: replace | jq recipes with --select across
help, MANUAL, RUNBOOK, SMOKE, README; new "select" editorial
topic`

**Scope (follow the
*"sweep the same pattern across the codebase"* mandate from
`CLAUDE.md` § *LLM-trap fix-on-first-surface* — one commit,
no instance left behind):**

- New file `src/help/content/select.md` per the editorial
  topic spec above. Register in `src/help/topics.rs` next
  to the other content topics. Wire into the search index
  in `src/help/search.rs`; raise the index byte cap to
  96 KB if the existing 80 KB cap trips. Index size is
  deterministic — measure with `cargo test --lib
  help::search::tests` and the existing size-pin test.
- Replace every `| jq` recipe in:
  - `src/cli.rs` `LONG_*` constants (e.g. `LONG_AUDIT_LS`
    examples at lines ~961–973, `audit show` doc-comment
    at line ~3310, anywhere else `jq` appears in long-help
    or doc-comment).
  - `src/help/content/examples.md` (line ~33).
  - `src/help/content/discovery.md` (lines ~18, ~75 —
    reword to "`jq` is optional" wording per design).
  - `src/help/content/quickstart.md` (line ~29 — the
    "Tier 3" prose).
  - `src/help/verbose/search.md` (line ~43).
  - `docs/MANUAL.md` (lines ~1853, ~1857; plus any new
    section spawned by C2 on the new flags).
  - `docs/RUNBOOK.md` (lines ~349, ~392, ~458, ~501).
  - `docs/SMOKE_v0.1.3.md` (lines ~117, ~146, ~170, ~172,
    ~188, ~225, ~229, ~248–249, ~282, ~291, ~301, ~306,
    ~316, ~318, ~373, ~409, ~415 — every `jq` recipe).
  - `README.md` (any `jq` references).
- Translation rules:
  - `… --json | jq '<F>'` → `… --json --select '<F>'`.
  - `… --json | jq -r '<F>'` → `… --json --select '<F>'
    --select-raw`.
  - `… --json | jq -s '<F>'` → `… --json --select-slurp
    '<F>'`.
  - Compound shell pipelines that *also* use `xargs` /
    `head` / `wc` after `jq`: keep the post-filter shell
    pipeline; only the `jq` segment is replaced.
  - Any prose that says "pipe to `jq`" or "use `jq`"
    becomes "use `--select`" (with the `inspect help
    select` cross-link if context permits).
- Add a one-paragraph "External `jq` still works" section
  under `inspect help formats` (or as a sidebar in
  `select.md` — pick one, keep it short). The idea: agents
  that already know jq idioms see the recipes the same way;
  operators with `jq` installed can keep using it for ad-hoc
  exploration outside the recipe set.
- Add `MANUAL.md` § *"JSON projection with `--select`"*
  (≤ 25 lines). Cross-reference from the existing JSON-output
  sections under each verb where applicable.
- Add `RUNBOOK.md` § operator note at the top of the JSON
  consumption guidance: "from v0.1.3 onward, recipes use
  `--select`; pre-v0.1.3 transcripts that pipe to `jq` still
  work because the binary you're driving still emits the
  same envelope".

**Acceptance tests** (extend
`tests/jaq_select_v013.rs`):

- `f19_help_select_topic_lists_in_index` — invoke
  `inspect help` (top-level) and assert the topic list
  contains `select`.
- `f19_help_select_topic_renders` — invoke `inspect help
  select`, assert the rendered prose contains "jq" and
  "--select" and is non-empty.
- `f19_help_search_finds_select_topic` — invoke
  `inspect help --search 'filter'`, assert at least one
  result row points at `select`.
- `f19_no_lone_jq_in_help_content` —
  `cargo`-side test that walks `src/help/content/*.md` +
  `src/help/verbose/*.md` and asserts every `| jq` token is
  inside a code block whose first surrounding sentence
  contains the words "external" or "optional" (i.e. the
  editorial pointer, not a recipe).
- `f19_no_lone_jq_in_long_constants` — compile-time
  `include_str!` check (or runtime test against the verb
  `--help` output) that asserts no `LONG_*` constant emits
  ` | jq ` as part of an example.
- `f19_help_search_index_under_cap` — pin the help search
  index size to ≤ 96 KB. If C3 had to bump from 80 → 96,
  this test pins the new cap; do not allow the cap to be
  trimmed in a future commit without a deliberate raise.

**Post-conditions checklist (run on resume):**

```sh
export PATH="$HOME/.cargo/bin:$PATH"
# Every recipe-bearing surface should be jq-free except for
# the editorial pointer in select.md / formats.md.
grep -RnE '\| jq ' src/help/content/ src/help/verbose/ \
  src/cli.rs docs/MANUAL.md docs/RUNBOOK.md \
  docs/SMOKE_v0.1.3.md README.md
# Expected hits: only inside select.md / formats.md, in a
# block prefixed by "external jq" / "optional".

inspect help select 2>&1 | head -5                              # renders
inspect help --search filter 2>&1 | grep -i select              # found
cargo test --test jaq_select_v013 2>&1 | grep -E 'test result'  # all pass
cargo test 2>&1 | grep -E '^test result|^running' | tail -40    # clean
```

**Update sweep for C3:** help-text + manual + runbook +
smoke. CHANGELOG + backlog tick happen in C4.

---

### Commit C4 — Tests round-out, CHANGELOG + backlog tick, `discovery` probe reword, F19 closes

**Subject:** `F19: tests round-out + CHANGELOG + backlog
tick — close F19`

**Scope:**

- Final acceptance tests, the ones that close the
  field-validation contract:
  - `f19_round_trip_status_envelope` — verify
    `inspect status arte --json` output equals
    `inspect status arte --json --select '.'` output (proves
    identity filter is a no-op on every envelope verb).
  - `f19_audit_show_select_revert_kind` — verify
    `--select '.data.entry.revert.kind'` returns the right
    string for a recorded composite/command_pair/snapshot
    write entry.
  - `f19_compose_ls_select_project_names` — verify
    `--select '.data.compose_projects[].name'` returns the
    project list (closes the precedent footgun
    documented in `CLAUDE.md` `compose ls --json` envelope
    path note).
  - `f19_select_history_show_audit_correlation` — verify the
    cross-link recipe from `docs/SMOKE_v0.1.3.md` (audit_id
    correlation) works under the new `--select` form.
  - `f19_select_run_stream_with_signal_forwarding` — verify
    `inspect run --stream --json --select '.line'` still
    forwards SIGINT cleanly to the remote (i.e., adding the
    filter does not break F16's signal-forwarding contract).
- `src/discovery/probes.rs`: reword the `jq` probe's
  diagnostic prose so a *missing* `jq` produces no scary
  warning ("`jq` is optional from v0.1.3 onward — every
  recipe in this manual works via `inspect <verb> --json
  --select`"). Probe still detects + records `jq` presence;
  the only change is messaging. Update the corresponding
  unit test.
- **CHANGELOG.md**: single bullet at the top of v0.1.3 *Added*
  block. Style matches F11 / F14 / L7: lead with the verb-
  level user-visible change, then the design pillars, then
  the field-trace ("agentic callers on a fresh box need not
  install `jq`"). Flag explicitly: *new flag triple*
  (`--select` / `--select-raw` / `--select-slurp`); *new
  editorial topic* (`inspect help select`); *new pure-Rust
  dependency* (`jaq-core` / `jaq-std` / `jaq-json`); *no
  envelope-shape change*; *no exit-code reuse* (parse-2,
  runtime-1, zero-result-1, raw-non-string-1 all map onto
  pre-existing semantics).
- **INSPECT_v0.1.3_BACKLOG.md**:
  - Add F19 row at the end of the Backlog table (status
    `✅ Done`, Notes column matching F14 / L7 verbosity).
  - Update the "Running total" line at the bottom of the
    backlog from `30 / 30 in-scope shipped` → `31 / 31
    in-scope shipped` and add a sentence about the
    smoke-promotion provenance ("F19 promoted from v0.1.5
    candidate during release-smoke preparation when the
    `| jq`-laden help surface was determined to violate
    the agent-friendliness invariant on a fresh-install
    target").
  - Update the implementation-order line to append `→ F19`.
  - Add a final "F19 field-validation gate" line under the
    smoke-test acceptance bullets at the very bottom.
- **docs/MANUAL.md**: confirm the *"JSON projection with
  `--select`"* section landed in C3; cross-reference if
  missed.
- **docs/RUNBOOK.md**: confirm the operator note from C3
  exists; add a `--select` line to the table of contents
  if RUNBOOK has one.
- **docs/SMOKE_v0.1.3.md**: add a P-level (P8-candidate, but
  noted as in-scope for v0.1.3 since F19 ships in v0.1.3)
  smoke-recipe block: 5 fast `--select` recipes that
  validate the contract end-to-end against a real host. (No
  P8 *enforcement* — the existing P1–P7 already covers every
  envelope verb's JSON path. The smoke recipes prove
  `--select` works on those same JSON paths against a real
  host.)
- Final pre-commit gate run; everything green; commit.

**Acceptance tests:** as above + the full pre-commit gate
matrix (28+1 suites, ~ 925 tests).

**Post-conditions checklist (run on resume):**

```sh
export PATH="$HOME/.cargo/bin:$PATH"
grep -nE '^\| F19' INSPECT_v0.1.3_BACKLOG.md                      # ✓ row
grep -nE '31 / 31 in-scope shipped' INSPECT_v0.1.3_BACKLOG.md     # ✓ updated total
grep -nE 'F19' CHANGELOG.md                                       # ✓ bullet
grep -nE 'JSON projection with `--select`' docs/MANUAL.md         # ✓ section
grep -nE 'select' docs/SMOKE_v0.1.3.md | head                     # ✓ recipes
cargo test 2>&1 | grep -E '^test result|^running' | tail -40      # clean
cargo clippy --all-targets -- -D warnings 2>&1 | tail -5          # clean
cargo fmt --check                                                 # clean
```

**Final closure check:** when this checklist is fully green,
F19 is shipped and v0.1.3 is back at "ready for release prep"
status with the `| jq` / fresh-box gap closed.

---

## Crash-recovery decision tree

If a session was interrupted, run the *Pre-state* block first.
Then map the observed state to one of these cases and act
accordingly:

- **Case A — `src/query/` does not exist.** Pre-C1. Start at
  C1; do not skip the Cargo.toml verification (deps already
  added).
- **Case B — `src/query/` exists, but `inspect query --help`
  fails or the verb is not in `src/cli.rs`.** Mid-C1. The
  abstraction has landed but the verb wiring did not
  finish. Re-read `git diff HEAD` and pick up the verb
  side; run C1 post-conditions before declaring it done.
  Confirm `cargo clippy --all-targets -- -D warnings` is
  clean — if dead-code lint is firing on `query::*`, the
  verb is not consuming everything yet.
- **Case C — `inspect query` passes its smoke recipes but
  `commands/query.rs` filter errors still use raw
  `eprintln!` instead of `error::emit`.** C1 done, C1-fixup
  not started. Open the C1-fixup commit: route the three
  filter-error stderr lines (parse / runtime / raw-non-
  string) through `crate::error::emit` with normalized
  labels ("filter parse:" / "filter runtime:" /
  "filter --raw:" — no "error" word in the label since
  `error::emit` adds the prefix).
- **Case D — C1-fixup landed but no `select` field in
  `FormatArgs` (`src/format/mod.rs`).** C2 not started.
  Open C2 starting with `FormatArgs` extension.
- **Case E — `FormatArgs.select` exists but `OutputDoc::print_json`
  signature still returns `()`.** Mid-C2. Either the
  envelope chokepoint or the streaming chokepoint has
  not been wired yet. Re-read `git diff HEAD`;
  identify which of the three chokepoints (OutputDoc,
  JsonOut, Renderer::dispatch) is incomplete and continue.
  The post-conditions grep on `transcript::emit_stdout` /
  `println!` in `src/verbs/output.rs` tells you whether
  the Renderer transcript-bypass fix landed.
- **Case F — C2 post-conditions green, but `| jq` still in
  help/MANUAL/RUNBOOK/SMOKE OR no `select.md` topic.**
  Mid-C3. Re-run the `grep -RnE '\| jq '` sweep, finish
  remaining files. Verify `inspect help select` renders.
  Verify the new `ERROR_CATALOG` row pointing at "select"
  is in `src/error.rs`. The C3 post-conditions checklist
  tells you when you're done.
- **Case G — C3 post-conditions green, but no `F19` row in
  the backlog.** Mid-C4. Run the C4 closure list; almost
  always this means CHANGELOG / backlog updates are pending.
- **Case H — All post-conditions green; F19 row says
  ✅ Done; branch is `v0.1.3-jaq`.** F19 is shipped on
  the branch. Run the full pre-commit gate one final time
  as a paranoia check; if green, merge `v0.1.3-jaq` back
  into `v0.1.3` (`git checkout v0.1.3 && git merge --ff-only
  v0.1.3-jaq`), delete the local + remote `v0.1.3-jaq`, and
  F19 is closed.

If at any point the state is internally inconsistent (e.g.
`FormatArgs.select` referenced from a chokepoint that
doesn't compile), **read the actual `git diff HEAD` first**
before deciding what to do — the document says what the
plan *was*; the diff says what actually happened.

---

## What F19 ships

F19 ships every flag, every recipe migration, and every help
surface needed to make the current help / MANUAL / RUNBOOK /
SMOKE documents work without an external `jq`:

- `--select` / `--select-raw` / `--select-slurp` on every
  JSON-emitting verb (envelope + streaming).
- `inspect query <FILTER>` standalone verb for arbitrary
  stdin filtering.
- `inspect help select` editorial topic.
- Every `| jq` recipe in `src/help/content/`,
  `src/help/verbose/`, `src/cli.rs` `LONG_*`, `docs/MANUAL.md`,
  `docs/RUNBOOK.md`, `docs/SMOKE_v0.1.3.md`, `README.md`
  rewritten to use `--select`.

This is the F19 contract — 100% of in-tree recipes work via
`--select` after C3 lands.
