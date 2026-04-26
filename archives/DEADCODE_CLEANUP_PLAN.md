# Dead-Code Cleanup — Execution Plan

Version: 1.0
Date: 2026-04-26
Source prompt: [PROMPT_DEADCODE_CLEANUP.md](PROMPT_DEADCODE_CLEANUP.md)
Bible v2 catalog: [INSPECT_BIBLEv6.2.md](INSPECT_BIBLEv6.2.md) §1, §6.7, §8.4, §27
Pre-req: HP-7 closed, 488 tests green, 0 build warnings.
Status: not-started → ships before live-server field tests.

---

## 0. Quick Navigation

| Section | Purpose |
|---|---|
| §1  | Objective + non-goals |
| §2  | Baseline inventory (numbers from a fresh grep) |
| §3  | The v2 allow-list (the only surviving `#[allow(dead_code)]` justifications) |
| §4  | Phase plan — **DC-0 → DC-6**, sequential, each phase test-green |
| §5  | Triage rubric (Category A / B / C decision tree) |
| §6  | Per-file battle map (every suppression site + first-pass classification) |
| §7  | High-risk landmines (the two module-level blanket suppressions) |
| §8  | Test harness & verification gates |
| §9  | Acceptance demo |
| §10 | Rollback strategy |

---

## 1. Objective

Reach a state where:

1. The only `#[allow(dead_code)]` attributes left in `src/` carry an
   inline justification of the form `// v2: <feature>, <reason>`, and
   each one maps to a literal line in the bible's v2 catalog (§3).
2. `cargo build` emits **zero** warnings.
3. `cargo test` is green at the same count we have today (488), or
   higher if dead-code deletion exposes a previously-shadowed test
   that now needs `#[cfg(test)]` gating.
4. `cargo clippy --all-targets -- -D warnings -D dead_code` is green.
   (We did not run with `-D warnings` until now because of the
   suppressions; this is the post-cleanup contract.)

### Non-goals

- No refactor that changes public behaviour. Deletions only.
- No new features. Anything that *would* be a feature — even one line
  of new code — gets queued for v2 and is **not** what justifies
  keeping dead code today.
- No comment-out hoarding. Dead code is deleted; git history is the
  archive.
- No lints beyond `dead_code` in scope. We are not chasing
  `clippy::pedantic` here.

---

## 2. Baseline Inventory

Captured from a fresh `grep -rn "allow(dead_code)" src/` on `main` at
the open of this plan:

```
suppressions:           46
files affected:         24
module-level blankets:   2     (src/exec/mod.rs, src/logql/mod.rs)
LOC in src/:        24,758
build warnings:          0     (because everything is suppressed)
test count:            488     (green)
```

**Per-file distribution** (descending):

| Count | File |
|---|---|
| 5 | [src/profile/cache.rs](src/profile/cache.rs) |
| 4 | [src/paths.rs](src/paths.rs) |
| 4 | [src/help/topics.rs](src/help/topics.rs) |
| 4 | [src/help/render.rs](src/help/render.rs) |
| 4 | [src/format/mod.rs](src/format/mod.rs) |
| 3 | [src/verbs/runtime.rs](src/verbs/runtime.rs) |
| 2 | [src/ssh/askpass.rs](src/ssh/askpass.rs) |
| 2 | [src/error.rs](src/error.rs) |
| 2 | [src/discovery/drift.rs](src/discovery/drift.rs) |
| 2 | [src/commands/placeholders.rs](src/commands/placeholders.rs) |
| 1 each (×14 files) | various |
| **+ 2 module-level** | [src/exec/mod.rs](src/exec/mod.rs), [src/logql/mod.rs](src/logql/mod.rs) |

The two module-level `#![allow(dead_code)]` attributes blanket-hide
every dead item in `src/exec/**` and `src/logql/**`. Real warning
count after we strip them is unknown until step DC-1 — assume **at
least** double the visible 46.

---

## 3. The v2 Allow-List (Authoritative)

These — and only these — are the bible-blessed v2 features. Any
surviving `#[allow(dead_code)]` after this cleanup MUST cite one of
these in its inline justification, and the citation MUST start with
`// v2:`.

| # | Tag | Bible reference | One-line scope |
|---|---|---|---|
| V1 | `tui-mode`              | §1, §27 | Interactive TUI / `info`-style navigator |
| V2 | `k8s-discovery`         | §1, §27 | Kubernetes-native source discovery |
| V3 | `distributed-tracing`   | §1, §27 | OTel / Jaeger span correlation |
| V4 | `os-keychain`           | §27     | OS keychain integration for SSH key passphrases |
| V5 | `per-user-policies`     | §8.4, §27 | Write-restricted tokens, approval flows |
| V6 | `russh-fallback`        | §27     | Pure-Rust SSH fallback when system `ssh` is absent |
| V7 | `parameterized-aliases` | §6.7, §27 | Aliases with arguments / chained aliases |
| V8 | `password-auth`         | §1, §27 | Interactive password auth (currently SSH-key only) |
| V9 | `remote-agents`         | §1, §27 | Tool-side agents on managed hosts |

Every other "future"/"reserved"/"someday" item gets **deleted**.
"Programmer error sites" or "internal asserts" do not justify
`allow(dead_code)` — they justify `unreachable!()`.

---

## 4. Phase Plan

Cadence: each DC-phase is one PR (or one commit on a feature branch)
ending in `cargo test` + `cargo build` both green. Phases are
sequential.

### DC-0 — Strip every suppression, capture the warning map

**Goal:** see the actual size of the iceberg.

**Scope:**
1. Remove every `#[allow(dead_code)]` and `#![allow(dead_code)]` from
   `src/`. Mechanical pass; no other edits.
2. Run `cargo build 2>&1 | tee /tmp/dc0-warnings.txt`.
3. Tally:
   - Total `warning:` lines.
   - Per-module count grouped by `src/<dir>/`.
   - Distinct dead-item kinds (`function is never used`, `field is
     never read`, `variant is never constructed`, etc.).
4. Commit `/tmp/dc0-warnings.txt` to a scratch branch — it's our
   triage map for DC-1..DC-5.

**DoD:**
- Zero `#[allow(dead_code)]` attributes anywhere under `src/`.
- `cargo test` still passes (deletions = zero, behaviour unchanged).
- `cargo build` emits the inventory of warnings.

---

### DC-1 — Triage: build the disposition table

**Goal:** every warning gets one of three labels: **DELETE**, **GATE**, **KEEP**.

**Scope:**
- Process the warning map from DC-0 row by row.
- Apply the rubric in §5.
- Emit `/tmp/dc1-disposition.tsv` with columns:
  `file:line  symbol  kind  category  v2_tag_or_blank  reason`
- DELETE/GATE/KEEP totals must sum to the DC-0 warning count.

**DoD:**
- TSV checked in to a scratch branch (not `main`).
- For every `KEEP`, the `v2_tag` column is one of V1..V9 from §3 —
  no other strings allowed.

---

### DC-2 — Land the KEEP list (Category C)

**Goal:** add the v2-justified annotations *first*, so DC-3..DC-5
deletions cannot accidentally wipe a real v2 sentinel.

**Scope:**
- For each `KEEP` row: add the precise attribute
  ```rust
  #[allow(dead_code)] // v2: <feature>, <one-line reason>
  ```
  using one of the V1..V9 feature tags from §3. The `<feature>` slot
  carries the human-readable bible phrase ("OS keychain integration",
  "Per-user policy enforcement", etc.); the V-tag is implicit by
  matching that phrase against §3.
- No deletions in DC-2. No code changes other than the attributes.

**DoD:**
- `grep -rn "allow(dead_code)" src/` returns *only* lines matching the
  regex `// v2: `.
- A test (added in DC-6) walks every surviving suppression and asserts
  the comment shape.

---

### DC-3 — Delete the easy DELETE rows (Category A, leaf items)

**Goal:** delete every leaf dead item — unused imports, unused enum
variants, unused struct fields, unused free functions, unused private
methods. Leaf = nothing else in the warning map points at it.

**Scope:**
- Walk DELETE rows in dependency order (leaves first, callers last).
- After each module's batch, run `cargo build` to see if cascade
  warnings appeared (a function whose only caller was just deleted).
- Add cascade items to the DELETE queue.
- Run `cargo test` after every batch of ≤ 10 deletions; never let it
  drift red for more than one commit.

**DoD:**
- All DELETE-leaf rows from the disposition TSV are gone.
- `cargo test` green.
- `cargo build` warning count ≤ pre-DC-3 count (no regressions).

---

### DC-4 — Delete the cascade DELETE rows (Category A, transitive)

**Goal:** finish the cascade kicked off by DC-3.

**Scope:**
- Re-run `cargo build` and triage every new warning surfaced by DC-3
  deletions.
- Repeat the leaf-first algorithm until `cargo build` shows no
  `dead_code` warning that is *not* a §3-justified KEEP.
- If a public API symbol surfaces as dead and is not in the §3 KEEP
  list: it gets deleted, even if it was thought to be "exported for
  callers". The only callers that matter are the ones in `src/`,
  `tests/`, `benches/`, and the bin target.

**DoD:**
- `cargo build 2>&1 | grep -c "warning: .*dead_code\|never used\|never read\|never constructed"` returns **0**.
- `cargo test` green.

---

### DC-5 — Gate test-only helpers (Category B)

**Goal:** anything used exclusively by tests moves behind `#[cfg(test)]`.

**Scope:**
- Walk GATE rows from DC-1.
- For each: wrap the item in
  ```rust
  #[cfg(test)]
  ```
  or move it inside a `#[cfg(test)] mod tests { … }` block in the
  same file.
- A handful of items will surface that DC-3/DC-4 thought were dead
  but are actually test-only — fold them in here.
- Builders and fixture constructors used by `tests/` integration
  files do not get `#[cfg(test)]`-gated (they ship in the test build);
  prefer moving them into a `pub(crate) mod test_support` and
  feature-gating that module via `#[cfg(any(test, feature = "test_support"))]`
  only if necessary. **Default: leave integration-test helpers in
  the regular build, since they are exercised by tests.**

**DoD:**
- All GATE rows resolved.
- `cargo build --tests` green.
- `cargo build` (no `--tests`) green and emits the items as
  appropriately gated (no dead_code warning, no leakage into release).

---

### DC-6 — Lock the contract

**Goal:** the cleanup cannot rot. The `dead_code` lint is now part of
the CI contract.

**Scope:**
- Add a unit test or integration test (`tests/no_dead_code.rs`) that
  walks `src/**/*.rs` and asserts:
  1. Every `#[allow(dead_code)]` is followed inline by `// v2:` and
     a tag matching one of V1..V9 from §3.
  2. There are no `#![allow(dead_code)]` blanket attributes anywhere
     in `src/` (they re-introduce module-level rot).
  3. Total surviving suppressions ≤ a checked-in cap (set the cap to
     the post-DC-5 count + 0 — every new v2 sentinel is a deliberate
     PR-level decision).
- Optional but recommended: add to `Cargo.toml`:
  ```toml
  [lints.rust]
  dead_code = "deny"
  ```
  so future contributors get the failure at compile time, not just CI.
- Update CI (`.github/workflows/*.yml` if present) to run
  `cargo build` with no warning suppressions and `cargo test`.

**DoD:**
- `tests/no_dead_code.rs` green; tampering proven to fail (manual
  smoke: add a bogus `#[allow(dead_code)]` without `// v2:` → CI red).
- `cargo build` warning count: 0.
- `cargo test` count: ≥ 488 (today's baseline).
- CI workflow updated.

---

## 5. Triage Rubric (the decision tree)

For every `dead_code` warning surfaced in DC-0:

```
┌─────────────────────────────────────────────────────────────────┐
│ Is the symbol referenced only by `#[cfg(test)]` code?           │
│   YES → Category B (GATE). Move behind `#[cfg(test)]`.          │
│   NO  ↓                                                          │
└─────────────────────────────────────────────────────────────────┘
┌─────────────────────────────────────────────────────────────────┐
│ Does the symbol map to a literal v2 feature in §3 (V1..V9)?     │
│   YES → Category C (KEEP).                                      │
│         Annotate `#[allow(dead_code)] // v2: <feature>, <why>`. │
│   NO  ↓                                                          │
└─────────────────────────────────────────────────────────────────┘
┌─────────────────────────────────────────────────────────────────┐
│ Default: Category A (DELETE).                                   │
│   Includes:                                                      │
│     - "I might need this later" (no — git has it)               │
│     - "It's a public API for downstream callers" (we have no    │
│       downstream callers; this is a binary)                     │
│     - "It documents the intended shape" (no — code documents    │
│       behaviour, comments document intent)                      │
└─────────────────────────────────────────────────────────────────┘
```

**Tiebreaker rule:** when in doubt, DELETE. The cost of restoring
from git is one `git log -S` away. The cost of keeping dead code
forever is paid every PR that touches the surrounding module.

---

## 6. Per-File Battle Map (first-pass classification)

This table is the **starting point** for DC-1. It is not authoritative
— actual triage happens against the warning output, not against this
list — but it sets expectations for where the work concentrates.

| File | # | First-pass guess | Notes |
|---|---|---|---|
| [src/exec/mod.rs](src/exec/mod.rs) | 1 module-level | **mostly DELETE** | Phase 7 said "until then dead-code allowed." Phase 7 is shipped. The blanket attribute is no longer correct. |
| [src/logql/mod.rs](src/logql/mod.rs) | 1 module-level | **mostly DELETE** | Same shape: phase comment says "until Phase 7 wires this in." Done. |
| [src/profile/cache.rs](src/profile/cache.rs) | 5 | mixed — likely **KEEP V2** for a couple, **DELETE** for the rest | Profile caching has obvious K8s-discovery extension points. |
| [src/paths.rs](src/paths.rs) | 4 | likely **KEEP V4 (os-keychain)** for one, **DELETE** for others | Path helpers tend to accrete during scaffolding. |
| [src/help/topics.rs](src/help/topics.rs) | 4 | **DELETE** (HP-2/4/6 wiring is done) | Existing comments say "consumed by HP-X"; HP-X is shipped, so they're now actually used or dead. |
| [src/help/render.rs](src/help/render.rs) | 4 | **DELETE** ("HP-6 will consume" — HP-6 shipped) | Same pattern. |
| [src/format/mod.rs](src/format/mod.rs) | 4 | mixed | Output format helpers; some may be live via `--format`. |
| [src/verbs/runtime.rs](src/verbs/runtime.rs) | 3 | likely **DELETE** | Runtime-detection helpers superseded by the readers under `exec/reader/`. |
| [src/ssh/askpass.rs](src/ssh/askpass.rs) | 2 | one **KEEP V4 (os-keychain)**, one **DELETE** | Askpass is exactly the OS-keychain integration point. |
| [src/error.rs](src/error.rs) | 2 | one **KEEP** (the documented "future error sites" comment is wrong — annotate properly or delete) | `emit_with_topic` is the live API after HP-5; check it's actually called. |
| [src/discovery/drift.rs](src/discovery/drift.rs) | 2 | likely **DELETE** | Drift is shipped; suppressions usually mean superseded helpers. |
| [src/commands/placeholders.rs](src/commands/placeholders.rs) | 2 | **DELETE** | "Placeholders" is literally what gets deleted at end-of-scaffolding. |
| [src/discovery/probes.rs](src/discovery/probes.rs) | 1 | likely **GATE** (test fixture) | One inner-attribute on a function. |
| [src/discovery/engine.rs](src/discovery/engine.rs) | 1 | likely **GATE** | Same shape. |
| [src/exec/field_filter.rs](src/exec/field_filter.rs) | 1 | unknown | Triage from warning. |
| [src/help/search.rs](src/help/search.rs) | 1 | **GATE** | Comment says "exercised by the HP-3 size guard test only" — that's the literal definition of Category B. |
| [src/config/resolver.rs](src/config/resolver.rs) | 1 | unknown | |
| [src/profile/schema.rs](src/profile/schema.rs) | 1 | unknown | |
| [src/selector/parser.rs](src/selector/parser.rs) | 1 | **KEEP V7 (parameterized-aliases)** | Existing comment: "reserved for future inline alias resolution" — that's V7 verbatim. |
| [src/ssh/master.rs](src/ssh/master.rs) | 1 | unknown | |
| [src/verbs/correlation.rs](src/verbs/correlation.rs) | 1 | likely **KEEP V3 (distributed-tracing)** | Correlation is exactly the tracing extension point. |
| [src/verbs/duration.rs](src/verbs/duration.rs) | 1 | unknown | |
| [src/verbs/output.rs](src/verbs/output.rs) | 1 | unknown | |
| [src/commands/recipe.rs](src/commands/recipe.rs) | 1 | unknown | One inner-attribute. |

**Total first-pass guesses:** ~30 DELETE, ~6 KEEP, ~5 GATE,
~5 unknown. Refined during DC-1 against the actual warning output.

---

## 7. High-Risk Landmines

### Landmine 1: the two module-level `#![allow(dead_code)]`

Files: [src/exec/mod.rs:15](src/exec/mod.rs) and [src/logql/mod.rs:23](src/logql/mod.rs).

These are **inner attributes** (`#![…]`) that suppress dead-code
warnings for everything under their respective module trees. Counts:

```
src/exec/   covers 18 files (engine, pipeline, readers, …)
src/logql/  covers 8 files  (lexer, parser, ast, validate, …)
```

When DC-0 strips them, expect the warning count to balloon. This is
where the bulk of DC-3/DC-4's work will land. Treat each module's
warning batch as its own sub-phase: triage `exec/` warnings as a unit
(they share the same parsers/readers), then `logql/` (lexer → parser
→ ast → validate has obvious dependency order).

### Landmine 2: items used only by `--json` / `--search` / `help all`

A handful of `topics.rs` items have comments like
`// consumed by inspect help --json (HP-4)`. After HP-4 shipped,
these *are* live — but the suppression was never removed. Verify with:

```bash
cargo build  # after DC-0 strips the suppressions
grep "topics.rs" /tmp/dc0-warnings.txt
```

If `topics.rs` shows zero warnings, the items are live: just delete
the suppressions (DC-2 work). If they still show as dead, the
"consumed by" claim is wrong — investigate before deleting.

### Landmine 3: integration tests in `tests/`

`#[cfg(test)]` only fires for the in-crate test build. Integration
tests in `tests/*.rs` link against the binary and exercise
`pub(crate)` items via the bin target. An item used only by
`tests/foo.rs` cannot be `#[cfg(test)]`-gated — it has to stay public
in the regular build. The DC-5 default is **leave it alone**; only
`#[cfg(test)]`-gate items used by `#[cfg(test)] mod tests` blocks
inside the same `src/` file.

### Landmine 4: macro-generated dead code

Some warnings come from items generated by `derive(…)`. These are
genuine bugs in our derives (a struct field that's never read) and
the right fix is usually deletion of the field, not suppression. If a
field is needed for `Serialize` round-trip but never read in code,
add `#[serde(skip_deserializing)]` and delete the field — or leave
it and add `#[allow(dead_code)] // v2: <feature>` if the v2 case is
real.

---

## 8. Test Harness & Verification Gates

After every phase:

```bash
cargo build                      # warning count tracked phase-over-phase
cargo test                       # must stay ≥ 488
cargo clippy --all-targets       # additional sanity (informational)
```

**Hard gates (CI-enforced post-DC-6):**

| Gate | Tool | Must pass |
|---|---|---|
| H1 | `cargo build`                                  | 0 warnings |
| H2 | `cargo test`                                   | ≥ 488 passed, 0 failed |
| H3 | `tests/no_dead_code.rs::every_allow_has_v2_tag` | every surviving `#[allow(dead_code)]` carries `// v2: <tag>` |
| H4 | `tests/no_dead_code.rs::no_module_level_blanket` | zero `#![allow(dead_code)]` in `src/` |
| H5 | `tests/no_dead_code.rs::suppression_cap`        | total surviving count ≤ baseline-after-DC-5 |

H3 + H4 are checked-in static walks; H5 is a counted assertion that
prevents creep ("just one more `allow`…").

---

## 9. Acceptance Demo

When DC-6 closes, this script must run clean on a fresh checkout:

```bash
# 1) Suppression count and shape
test "$(grep -rln 'allow(dead_code)' src/ | wc -l)" -le 5     # ≤ 5 v2 sentinels expected
grep -rn 'allow(dead_code)' src/ | grep -v '// v2:' && exit 1 # every survivor has v2 tag
grep -rn '#!\[allow(dead_code)\]' src/ && exit 1              # no module-level blankets

# 2) Build is clean
cargo build 2>&1 | grep -c '^warning:' | grep -q '^0$'

# 3) Tests still green
cargo test 2>&1 | grep -E '^test result:' \
  | awk '{p+=$4; f+=$6} END {exit (p<488 || f>0)}'

# 4) The dead-code lint is now a contract
cargo build 2>&1 | grep -i 'dead_code' && exit 1

# 5) The CI guard test passes
cargo test --test no_dead_code

# 6) LOC accounting (informational, not a gate)
find src/ -name '*.rs' | xargs wc -l | tail -1
```

Expected outcome: every step exits 0; the LOC line reports the new
total (anticipated: ~22,500 — a ~2,200 line reduction from the
24,758 baseline, give or take).

---

## 10. Rollback Strategy

The cleanup is mechanical and high-volume. Two safeguards:

1. **Branch per phase.** `dc-0`, `dc-1` (TSV only — no code), `dc-2`
   through `dc-6`. Each phase merges only when its DoD is met. A
   failing phase rolls back via `git reset --hard origin/dc-<n-1>`.

2. **Atomic commits inside each phase.** Inside DC-3 / DC-4, batch
   deletions in groups of ≤ 10 items per commit, with a one-line
   message naming the module: `dc3: delete dead helpers in
   profile/cache`. If a deletion turns out to break something at a
   later phase, the offending commit is `git revert`'d and the item
   is reclassified KEEP-V<n> with the new evidence.

The bible and the implementation plans (HP-* / phase*) are not
edited by this work — they are the source of truth for what counts
as v2 and what counts as already-shipped. The post-cleanup ground
truth lives in the code; the docs already describe the steady state.

---

## 11. Estimated Deliverables

| Phase | Lands | PR scope |
|---|---|---|
| DC-0 | All 46 + 2 module-level suppressions stripped; warning map captured | small |
| DC-1 | Disposition TSV (no code change) | small (paperwork) |
| DC-2 | ≤ 10 KEEP-V<n> sentinels annotated with `// v2: <feature>` | small |
| DC-3 | First-pass DELETE batch (~80 % of dead items) | medium |
| DC-4 | Cascade DELETE batch | small |
| DC-5 | Test-only items moved behind `#[cfg(test)]` | small |
| DC-6 | `tests/no_dead_code.rs` + Cargo lint config + CI update | small |

**Estimated LOC delta:** −1,500 to −3,000. The two module-level
blankets cover ~6,000 lines of source; even a 25–50 % dead rate in
those modules dwarfs every other source of cleanup.

---

*Source: this plan implements the steps in
[PROMPT_DEADCODE_CLEANUP.md](PROMPT_DEADCODE_CLEANUP.md) end-to-end.
Any deviation is a bug in this plan, not the prompt.*
