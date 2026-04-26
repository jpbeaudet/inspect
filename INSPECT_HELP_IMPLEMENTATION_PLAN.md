# INSPECT Help System — Implementation Plan

Version: 1.0
Date: 2026-04-26
Source of truth: [INSPECT_HELP_BIBLE.md](INSPECT_HELP_BIBLE.md)
Status: not-started → ships before Phase 12.

---

## 0. Quick Navigation

| Section | Purpose |
|---|---|
| §1  | Delivery objective and non-goals |
| §2  | Architecture at a glance (one diagram, one paragraph) |
| §3  | File layout (`src/help/`) |
| §4  | Phase plan — **HP-0 → HP-6**, with per-phase DoD |
| §5  | Topic catalog (12 topics, owner, status) |
| §6  | CLI surface contract (`inspect help`, flags, exit codes) |
| §7  | Error → help linkage protocol |
| §8  | Test harness & CI guards (the contract that prevents rot) |
| §9  | Style guide (the rules every topic file must obey) |
| §10 | Risks & open decisions |
| §11 | Acceptance demo script |

---

## 1. Delivery Objective

Ship the help system specified in `INSPECT_HELP_BIBLE.md` such that:

1. A new user can discover every verb, flag, topic, and concept **from the binary alone** — no browser, no docs site, no network.
2. Every error message lands the user on the relevant topic.
3. An LLM agent can enumerate the entire surface programmatically via `--json`.
4. The system **cannot rot**: CI guards reject any new command, flag, or error site that doesn't carry its help wiring.

### Non-goals (v1)

- No interactive TUI / `info`-style navigator — pure text output, pager-friendly.
- No localization — English only. Topic files are structured to make i18n a later additive change.
- No dynamic help fetched from the network.
- No man pages — `inspect help` *is* the man page. (A `man inspect` build artifact is HP-6 stretch.)

---

## 2. Architecture at a Glance

```
                     ┌──────────────────────────────────┐
       user ─────►   │  clap-generated `--help`         │ ◄── short, terse, flag-focused
                     │  (every command, auto)            │
                     └────────────┬─────────────────────┘
                                  │ "see also: inspect help <topic>"
                                  ▼
                     ┌──────────────────────────────────┐
       user ─────►   │  `inspect help [topic]`          │ ◄── prose, examples-first
                     │  src/help/<topic>.md (include_str!)
                     └────────────┬─────────────────────┘
                                  │
                ┌─────────────────┼─────────────────┬──────────────────┐
                ▼                 ▼                 ▼                  ▼
      `--search <kw>`     `--json`           `--verbose`         `help all`
      (compiled index)   (LLM contract)    (edge cases)       (pipeable dump)
```

**Single sentence:** every verb gets clap `--help` (auto), every concept gets a `.md` topic compiled into the binary via `include_str!`, the two are cross-linked both ways, and an in-binary keyword index plus a JSON dump make the whole thing searchable by humans and machines.

---

## 3. File Layout

```
src/
├── cli.rs                          # add `Help { ... }` subcommand
├── help/
│   ├── mod.rs                      # registry, dispatch, render
│   ├── render.rs                   # pager-aware writer, NO_COLOR, width detection
│   ├── search.rs                   # build-time index + runtime query
│   ├── json.rs                     # `--json` envelope + schema version
│   ├── topics.rs                   # const TOPICS: &[Topic]
│   ├── content/                    # one .md per topic (include_str!)
│   │   ├── quickstart.md
│   │   ├── selectors.md
│   │   ├── aliases.md
│   │   ├── search.md
│   │   ├── formats.md
│   │   ├── write.md
│   │   ├── safety.md
│   │   ├── fleet.md
│   │   ├── recipes.md
│   │   ├── discovery.md
│   │   ├── ssh.md
│   │   └── examples.md
│   └── verbose/                    # optional sidecars for --verbose
│       ├── ssh.md
│       └── search.md
├── error.rs                        # add `help_topic: Option<&'static str>`
└── ...
build.rs                            # generate src/help/index.bin (search index)
tests/
└── help_contract.rs                # CI guards (see §8)
```

**Why `.md` files instead of inline `&'static str`:** topic content is authored, reviewed, and grep'd as prose. Markdown renders fine in plain terminals (we strip nothing — `**bold**` and `` `code` `` are intentional Loki/git-style cues). The `include_str!` keeps the binary single-file.

---

## 4. Phase Plan

Cadence: each HP-phase is a single PR. No phase merges with red tests or a missing CI guard. Phases are sequential — later phases depend on earlier ones. Total expected delivery: ≤ 7 PRs.

### HP-0 — Foundation & dispatch

**Goal:** `inspect help` and `inspect help <topic>` work end-to-end with one real topic + a placeholder index page.

**Scope:**
- `src/help/mod.rs` with `Topic { id, title, summary, body: &'static str }` and `const TOPICS: &[Topic]`.
- `Help { topic, search, json, verbose }` clap subcommand wired in `src/cli.rs`.
- Dispatcher in `src/commands/help.rs`: no topic → render index; topic found → render body; topic missing → render "did you mean" with edit-distance suggestions.
- `src/help/render.rs`: pager auto-launch (env `PAGER`, default `less -FRX`), `NO_COLOR` honored, terminal-width-aware wrapping at 80 cols (override `INSPECT_HELP_WIDTH`).

**Deliverables:**
- One real topic shipped: `quickstart.md`.
- Topic index page literally matches §2.1 of the bible.
- Exit codes: 0 on success, 1 on unknown topic, 2 on internal error.

**DoD:**
- `inspect help` prints the index, fits in ≤ 40 lines on an 80-col tty.
- `inspect help quickstart` renders the topic.
- `inspect help nonexistent` exits 1 with `did you mean: quickstart?`.
- Smoke test in `tests/help_contract.rs` covers all three.

---

### HP-1 — Topic content (all 12)

**Goal:** every topic in bible §3 ships as a real `.md` file.

**Scope:**
- Author the remaining 11 topics: `selectors`, `aliases`, `search`, `formats`, `write`, `safety`, `fleet`, `recipes`, `discovery`, `ssh`, `examples`.
- Each topic obeys the §9 style guide (EXAMPLES → DESCRIPTION → DETAILS → SEE ALSO).
- Examples use realistic selectors (`arte/atlas`, `prod-*/storage`) — never `foo`/`bar`.

**Deliverables:**
- 12 topic files, each ≤ 120 lines, ≥ 3 copy-pasteable examples.
- The `examples` topic includes the full grep/stern/kubectl/sed translation guide from bible §3.12.

**DoD:**
- `inspect help all` renders every topic without error, ordered as in the index.
- `tests/help_contract.rs` asserts every topic has ≥ 3 example lines (`$ inspect …`).
- Each topic's `SEE ALSO` block resolves (HP-7 enforces; HP-1 hand-checked).

---

### HP-2 — Per-verb `--help` polish + cross-links

**Goal:** every clap command's `--help` carries (a) realistic examples and (b) a `See also: inspect help <topic>` footer.

**Scope:**
- Audit every `#[derive(Args)]` struct in `src/cli.rs`. Add `#[command(after_help = "…")]` with `See also` lines mapping to the relevant `inspect help <topic>`.
- Add a 2–3 line example block to each verb's `long_about` so `inspect <verb> --help` is self-sufficient.
- Verb→topic mapping table (lives in `src/help/topics.rs`):

  | Verb group | Primary topic | Secondary |
  |---|---|---|
  | `grep`, `cat`, `ls`, `find`, `ps`, `status`, `health`, `volumes`, `images`, `network`, `ports`, `logs` | `selectors` | `formats`, `examples` |
  | `search` | `search` | `selectors`, `aliases`, `formats` |
  | `restart`, `stop`, `start`, `reload`, `cp`, `edit`, `rm`, `mkdir`, `touch`, `chmod`, `chown`, `exec` | `write` | `safety`, `fleet` |
  | `audit`, `revert` | `safety` | `write` |
  | `fleet` | `fleet` | `write`, `selectors` |
  | `recipe`, `why`, `connectivity` | `recipes` | `examples` |
  | `add`, `remove`, `list`, `show`, `test`, `setup`, `connect`, `disconnect`, `connections`, `disconnect-all` | `discovery` | `ssh`, `quickstart` |
  | `alias` | `aliases` | `selectors`, `search` |

**Deliverables:**
- Updated `cli.rs` with `after_help` on every command.
- Topic-to-verb registry exposed via `src/help/topics.rs::verbs_for(topic)` (used by `--json`).

**DoD:**
- CI guard: every command in clap's `Command::get_subcommands()` has a non-empty `after_help`.
- `inspect grep --help` ends with the exact line `See also: inspect help selectors, inspect help formats, inspect help examples`.

---

### HP-3 — Search index + `--search`

**Goal:** `inspect help --search <keyword>` finds every topic, verb, flag, and example mentioning the keyword. Zero-runtime-cost: index baked into the binary.

**Scope:**
- `build.rs`: read every file under `src/help/content/`, tokenize on whitespace and word boundaries, lowercase, strip 30-word stop list. Emit a sorted `&'static [(keyword, &[(TopicId, line_number)])]` to `OUT_DIR/help_index.rs`, `include!`-d by `src/help/search.rs`.
- Runtime query: substring match against the keyword column (binary search), then return matching `(topic, line)` pairs grouped by topic, with a 60-char context snippet around each line.
- Multi-keyword AND semantics: `inspect help --search "fleet apply"` only returns topics that mention both.

**Deliverables:**
- `src/help/search.rs` with `pub fn query(needle: &str) -> Vec<SearchHit>`.
- Output format matches bible §4.2 byte-for-byte.

**DoD:**
- `inspect help --search timeout` produces ≥ 3 hits across `search`, `grep --help`, `ssh`.
- `inspect help --search xyzzynonexistent` exits 1 with empty result line.
- Index size ≤ 50 KB compiled (asserted in test).
- Build does not require any new runtime dep — pure `std`.

---

### HP-4 — `--json` machine contract

**Goal:** `inspect help --json` emits the full discoverable surface as a single stable JSON document. LLM agents and external tools depend on this.

**Scope:**
- Schema versioned: `{"schema_version": 1, "binary_version": "x.y.z", ...}`.
- Document includes:
  - `topics`: `[{ id, title, summary, examples: [...], see_also: [...] }, ...]`
  - `commands`: `{ verb: { aliases, summary, flags: [{ name, short, long, takes_value, repeated, description }], examples, see_also } }`
  - `reserved_labels`, `source_types`, `output_formats` (literal lists from bible §3.4–§3.5)
  - `errors`: `[{ code, summary, help_topic }, ...]` (the catalog HP-5 builds)
- `inspect help <topic> --json` emits a single-topic envelope.
- Pretty-printed when stdout is a tty, compact NDJSON-friendly otherwise.

**Deliverables:**
- `src/help/json.rs` with the serializer.
- Snapshot test (`tests/help_json_snapshot.rs`) asserts the schema is byte-stable across builds — schema bumps require a deliberate snapshot update.

**DoD:**
- Document validates against an inline JSON Schema in the test.
- `jq '.commands.grep.flags[].name'` works.
- Schema version bump procedure documented in this file (§10).

---

### HP-5 — Error → help linkage

**Goal:** every user-facing error names the topic that explains it.

**Scope:**
- Extend `src/error.rs`: every variant carries `help_topic: Option<&'static str>`. New constructor pattern: `InspectError::EmptySelector { selector, help_topic: "selectors" }`.
- Audit pass over **every** `eprintln!("error: …")` and `anyhow!(…)` site in the codebase. For each, decide topic or `None`.
- Renderer (top-level in `main.rs`) appends `\n  see: inspect help <topic>` when `help_topic` is `Some`.
- Errors with no obvious topic (programmer errors, internal asserts) get `None` deliberately — CI guard does not require coverage there.

**Deliverables:**
- `tests/help_contract.rs::all_user_errors_have_topic_or_none()` enumerates every `InspectError` variant.
- A catalog table in `src/error.rs` mapping error code → help topic, exposed via `--json` (HP-4).

**DoD:**
- The five canonical error examples in bible §4.1 reproduce byte-for-byte under `INSPECT_NON_INTERACTIVE=1`.
- Audit-log shows every unique error code emitted under the integration test suite has a topic decision recorded.

---

### HP-6 — Verbose, `help all`, render polish

**Goal:** depth-on-demand and pipe-friendly dumps; final UX scrub.

**Scope:**
- `--verbose` flag on `inspect help <topic>`: appends optional `src/help/verbose/<topic>.md` sidecar when present (bible §4.5).
- `inspect help all`: concatenates every topic with `═══` separators, suppresses pager (assumed to be piped).
- Render polish:
  - Pager: respect `PAGER`, default `less -FRX`, fall through to direct stdout when not a tty.
  - Width: detect via `tput cols` / `terminal_size` crate (already a transitive dep) / env `COLUMNS`, default 80, clamp 60..120.
  - `NO_COLOR` and `INSPECT_NO_COLOR` both honored.
  - Strip ANSI when stdout is not a tty.
- Optional stretch: `man inspect` build target via `clap_mangen` — emits `man/inspect.1` from the same registry.

**Deliverables:**
- 2–4 `verbose/<topic>.md` sidecars (`ssh`, `search`, `write`, `safety` are the obvious candidates).
- `help all` dump fits a single `less` window's heading navigation.

**DoD:**
- `inspect help ssh --verbose` adds the MaxSessions caveat.
- `inspect help all > out.txt && wc -l out.txt` succeeds; output ≥ 1500 lines.
- `NO_COLOR=1 inspect help search | grep -c $'\x1b\['` returns `0`.

---

### HP-7 — CI guards & shipped tests *(merged into HP-0..HP-6 incrementally; finalised here)*

**Goal:** the help system cannot rot. Every guard listed in §8 is wired into `cargo test` and runs on every PR.

**Scope:** see §8 — that section is the full spec.

**DoD:** all 8 guards green; one synthetic regression PR (deleting an `after_help`, breaking a `See also`, removing a topic, adding an error without `help_topic`) demonstrably fails CI.

---

## 5. Topic Catalog

| # | Topic | Bible § | Owner | Status | Lines (target) |
|---|---|---|---|---|---|
| 1 | `quickstart`  | 3.1  | TBD | not-started | ≤ 60 |
| 2 | `selectors`   | 3.2  | TBD | not-started | ≤ 90 |
| 3 | `aliases`     | 3.3  | TBD | not-started | ≤ 70 |
| 4 | `search`      | 3.4  | TBD | not-started | ≤ 120 |
| 5 | `formats`     | 3.5  | TBD | not-started | ≤ 90 |
| 6 | `write`       | 3.6  | TBD | not-started | ≤ 90 |
| 7 | `safety`      | 3.7  | TBD | not-started | ≤ 80 |
| 8 | `fleet`       | 3.8  | TBD | not-started | ≤ 80 |
| 9 | `recipes`     | 3.9  | TBD | not-started | ≤ 80 |
| 10 | `discovery`  | 3.10 | TBD | not-started | ≤ 80 |
| 11 | `ssh`        | 3.11 | TBD | not-started | ≤ 90 |
| 12 | `examples`   | 3.12 | TBD | not-started | ≤ 120 |

**Total:** ≤ 1050 lines of authored prose, plus per-verb `after_help` lines, plus optional `verbose/` sidecars.

---

## 6. CLI Surface Contract

```
inspect help                         # topic + command index (≤ 40 lines)
inspect help <topic>                 # render one topic
inspect help <topic> --verbose       # topic + verbose/<topic>.md sidecar
inspect help all                     # every topic, in catalog order
inspect help --search <keyword...>   # AND across keywords
inspect help --json                  # full machine-readable surface
inspect help <topic> --json          # one topic as JSON envelope

inspect <verb> --help                # clap-auto, with after_help "See also:"
inspect <verb> -h                    # short form (clap default)
```

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Help rendered (or search produced ≥ 1 hit) |
| 1 | Topic not found / search returned 0 hits |
| 2 | Internal error (I/O on pager, malformed JSON, etc.) |

### Env vars

| Var | Default | Purpose |
|---|---|---|
| `PAGER` | `less -FRX` | Pager binary for `inspect help <topic>` |
| `INSPECT_HELP_WIDTH` | terminal width or 80 | Override wrap column |
| `NO_COLOR` / `INSPECT_NO_COLOR` | unset | Disable any ANSI in help |
| `INSPECT_HELP_NO_PAGER` | unset | Force direct stdout (also implied by non-tty) |

---

## 7. Error → Help Linkage Protocol

Every `InspectError` variant declares a topic (or explicit `None`) at construction time:

```rust
return Err(InspectError::EmptySelector {
    selector: sel.to_string(),
    help_topic: Some("selectors"),
});
```

The top-level renderer in `main.rs` appends:

```
  see: inspect help selectors
```

**Coverage rule:** any error reachable from the user-facing CLI must either carry a topic or be tagged `#[allow(missing_help_topic)]` with a one-line justification comment. The CI guard in §8 enforces this.

**Authoritative mapping table** (kept in `src/error.rs`):

| Error variant | Topic |
|---|---|
| `EmptySelector`, `BadSelectorGrammar`, `AmbiguousService` | `selectors` |
| `BadAliasType`, `UnknownAlias` | `aliases` |
| `BadLogQL`, `UnknownLabel`, `UnknownSource`, `MetricVsLogMixed` | `search` |
| `MutuallyExclusiveFormat`, `BadTemplate` | `formats` |
| `MissingApply`, `LargeFanoutAborted`, `MissingAllowExec` | `write` |
| `AuditEntryNotFound`, `RevertHashMismatch` | `safety` |
| `NamespaceNotConfigured`, `BadNamespacePattern` | `fleet` |
| `RecipeNotFound`, `RecipeMutatingNeedsApply` | `recipes` |
| `DiscoveryFailed`, `ProfileMissing`, `RemoteMissingTool` | `discovery` |
| `SshConnectFailed`, `MaxSessionsExceeded`, `KeyDecryptFailed` | `ssh` |

---

## 8. Test Harness & CI Guards

All in `tests/help_contract.rs`. Each guard is a single `#[test]` so failure messages name the violated rule.

| # | Guard | What it checks |
|---|---|---|
| G1 | `every_command_has_after_help` | walks `clap::Command::get_subcommands()`; asserts `after_help().is_some()` and contains `"inspect help "`. |
| G2 | `every_command_has_long_about_examples` | asserts each command's `long_about` contains at least one line starting with `$ inspect `. |
| G3 | `every_topic_resolves` | every topic listed in `TOPICS` and every topic referenced by a `See also:` line resolves to a real file. |
| G4 | `every_topic_has_examples` | parse each topic file; assert `EXAMPLES` block has ≥ 3 `$ inspect` lines. |
| G5 | `every_topic_example_parses` | for every `$ inspect …` line in every topic, run it through clap's `try_parse_from` (no execution) and assert success. Catches stale flag names. |
| G6 | `every_user_error_has_topic_decision` | reflection over `InspectError` variants: each must be either `Some("…")` or carry `#[allow(missing_help_topic)]`. |
| G7 | `search_index_size_bounded` | the compiled index is ≤ 50 KB. |
| G8 | `json_schema_is_stable` | `inspect help --json` matches a checked-in golden snapshot; intentional bumps require updating the snapshot. |

**Synthetic regression PR** (run once at HP-7 close, kept in `tests/help_regression_demo.md` as a recipe): introduce four bugs and verify each guard fails with the expected message.

---

## 9. Style Guide for Topic Files

Every `.md` file under `src/help/content/` MUST conform:

1. **First non-blank line** is the title in the form `TITLE — One-line description` (bible §2.3).
2. **EXAMPLES** block first, ≥ 3 lines, each starting `$ inspect `, copy-pasteable, using realistic names (`arte`, `prod-*`, `pulse`, `atlas`) — never `foo`/`bar`.
3. **DESCRIPTION** block: prose, ≤ 30 lines, terse, imperative for instructions, indicative for facts.
4. **DETAILS / GRAMMAR / FLAGS** (optional): tables, BNF, edge cases.
5. **SEE ALSO** block last: `inspect help <topic>   <one-line reason>`. Every reference must resolve (G3 enforces).
6. Section headers in **UPPERCASE** (no `##` markdown). The renderer is line-oriented; we deliberately avoid Markdown's heading hierarchy because the output is plain text.
7. Inline formatting allowed: `` `code` `` for commands/flags, `**bold**` for emphasis. The renderer leaves these as-is — modern terminals render `bold` via `*`/`_` cues, and pure-text users still parse them visually.
8. Hard-wrap at 80 columns. (CI guard not added in v1; manual review enforces.)
9. No external URLs except in `search.md` (the Loki LogQL upstream reference).
10. Voice: terse, direct. No "simply", no "easily", no "just". No marketing.

---

## 10. Risks & Open Decisions

| Risk | Mitigation |
|---|---|
| **Topic content rot when verbs change** | G5 (every embedded example must `try_parse_from` clap) catches stale flag names automatically. |
| **`--json` schema breakage breaks LLM agents** | G8 (golden snapshot) makes every change deliberate; schema is versioned. Bump procedure: increment `schema_version`, update snapshot, document the diff in CHANGELOG. |
| **Index bloat blows up binary** | G7 caps at 50 KB. Stop list + lowercase keeps it tight. |
| **Pager dependency** | Auto-detect `less` then `more` then direct stdout. Never required. |
| **`man inspect` parity** | Stretch only. If shipped (HP-6), generated from the same `TOPICS` registry via `clap_mangen` so it can't drift. |

**Open decisions (call out before HP-1 starts):**

1. Topic file extension: `.md` (chosen — see §3) vs `.txt`. Decision: `.md` — better for editor highlighting, no runtime markdown parsing.
2. Sub-topics (`inspect help search.metrics`)? **Defer**. Use `--verbose` instead.
3. Localization scaffolding? **Defer to v2.** Topic files keyed by `id`; `content/<lang>/<topic>.md` is the future shape, not blocking v1.
4. Should `inspect help` index page include the `Audit:` and `Other:` command rows from bible §2.1? **Yes** — index page is the contract, render it verbatim.

---

## 11. Acceptance Demo Script

When HP-6 closes, this script must run clean, top to bottom, on a fresh checkout:

```bash
# 1) Index page fits one screen
inspect help | wc -l            # expect ≤ 40

# 2) Topics work
for t in quickstart selectors aliases search formats write safety fleet \
         recipes discovery ssh examples ; do
  inspect help "$t" >/dev/null || { echo "FAIL: $t"; exit 1; }
done

# 3) Per-verb help has cross-links
for v in grep logs search restart edit cp fleet audit alias setup ; do
  inspect "$v" --help | grep -q "See also: inspect help " \
    || { echo "FAIL: $v --help missing See also"; exit 1; }
done

# 4) Search works
inspect help --search timeout | grep -q "inspect help search"

# 5) JSON contract
inspect help --json | jq -e '.schema_version == 1
                            and (.topics|length) == 12
                            and (.commands|keys|length) >= 30' >/dev/null

# 6) Errors land on topics
inspect grep "x" arte/nonexistent 2>&1 | grep -q "see: inspect help selectors"

# 7) Verbose adds depth
inspect help ssh --verbose | wc -l > /tmp/v
inspect help ssh           | wc -l > /tmp/n
test "$(cat /tmp/v)" -gt "$(cat /tmp/n)"

# 8) NO_COLOR honored
NO_COLOR=1 inspect help search | grep -c $'\x1b\[' | grep -q "^0$"

# 9) help all is pipeable
inspect help all | wc -l        # expect ≥ 1500

# 10) CI guards
cargo test --test help_contract
```

If every step exits 0, the help system ships.

---

## 12. Phase Summary Table

| Phase | Lands | Depends on | PR scope |
|---|---|---|---|
| HP-0 | dispatch + 1 topic + render | — | small |
| HP-1 | 11 remaining topic files | HP-0 | medium (mostly prose) |
| HP-2 | per-verb `after_help` + examples in `long_about` | HP-1 | medium (audit every clap struct) |
| HP-3 | search index + `--search` | HP-1 | small (build.rs + 1 module) |
| HP-4 | `--json` + golden snapshot | HP-2 | small |
| HP-5 | error → topic linkage across codebase | HP-1 | medium (audit pass) |
| HP-6 | `--verbose`, `help all`, render polish, optional `man` | HP-1, HP-2 | small |
| HP-7 | CI guards finalised + regression demo | all prior | small |

**Estimated total deliverable:** ~1500 lines of prose (topic files), ~600 lines of Rust (module + search + json + render), ~200 lines of tests, plus mechanical edits to `cli.rs` and `error.rs`. No new runtime dependencies.

---

*Source: this plan implements `INSPECT_HELP_BIBLE.md` end-to-end. Any deviation from the bible is a bug in this plan, not the bible.*
