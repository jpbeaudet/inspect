# Cleaning Duty — post-release codebase normalization

This document is the standing checklist for the **release-prep
cleanup pass** that runs once per minor release, after the final
backlog item ships and before the tag.

The goal is to convert the in-flight, marker-rich working state of
a release into industry-grade source that an open-source contributor
can read cold without reaching for the backlog.

The convention during a release is the opposite: every non-trivial
comment, doc-comment, test name, and changelog entry carries an
`F<n>` / `L<n>` prefix and a `(vX.Y.Z)` tag because that traceability
is *essential* during development — every smoke-test bug, every code
review, every cross-file reference relies on it. **Don't strip the
markers mid-release.** They are load-bearing context until the
release is locked.

Once the release is locked, those markers become noise to a future
reader who has no reason to care which slot of which backlog the
code came from. Cleaning Duty lifts them out.

## When to run

Run **after** the final backlog item is shipped and smoke-tested
against a live system, and **before** the version tag is cut. This
sits between the last `✅ Done` row and the `git tag vX.Y.Z`.

The release-prep order is:

1. Final backlog item ships (CHANGELOG / MANUAL / RUNBOOK / help
   text / tests all updated per the per-backlog-item sweep).
2. Live smoke runbook passes end-to-end.
3. **Cleaning Duty (this document).**
4. Archive sweep — move closed planning docs into
   `archives/v<MAJOR.MINOR.PATCH>/`.
5. README freshness pass — version status banner, install snippet,
   version-history table, test counts.
6. CLAUDE.md release-window update — pivot the in-flight policy
   sections to the next release.
7. Tag, build, publish.

## Scope — what gets normalized

### A. Inline comments and doc-comments

Strip `F<n>` / `L<n>` / `(vX.Y.Z)` markers from prose. **Keep the
substance.** Three classes:

- **Class A — substance with marker prefix.** Drop the prefix,
  capitalize the next letter, leave the rest untouched. Example:

  ```rust
  /// F9 (v0.1.3): byte count of local stdin forwarded to the
  /// remote command. Recorded so a post-hoc audit can answer …

  // becomes:

  /// Byte count of local stdin forwarded to the remote command.
  /// Recorded so a post-hoc audit can answer …
  ```

- **Class B — load-bearing migration / on-disk-format hints.**
  These document a real format boundary (audit JSONL field added
  in version X, older entries don't carry it, deserializer must
  accept absence). Keep the version-as-data because it pins the
  schema-migration semantics — but rephrase neutrally:

  ```rust
  /// L7 (v0.1.3): which redaction maskers fired during this
  /// verb's streamed output. Pre-L7 entries elide the field …

  // becomes:

  /// Which redaction maskers fired during this verb's streamed
  /// output. Older audit entries (audit schema versions before
  /// this field existed) deserialize as `None`.
  ```

- **Class C — pure breadcrumbs.** The marker is followed by an
  already-self-contained explanation. Drop the prefix and
  capitalize. No substance change.

### B. CHANGELOG and BACKLOG referencing

The CHANGELOG stays as-is — it is a historical document and
preserves its release-window markers. The closed backlog moves to
`archives/v<MAJOR.MINOR.PATCH>/` unmodified.

### C. Test names + per-release archive test files

**Do not rename test functions.** `f14_stream_records_streamed_true`
stays. Test names are git-archaeology IDs, not user-facing prose,
and bisecting against a release becomes painful if they drift.

Test *comments* in active suites get Class A/B/C the same as source.
But **per-release archive test files** — `tests/phase_a_v011.rs`,
`tests/phase_b_v011.rs`, `tests/phase_c_v011.rs`,
`tests/phase_f_v013.rs`, etc. — keep their release-window markers
in section headers and bullet-list module docstrings. These files
are themselves archaeology: their `//! - **P3** ...` style manifests
document what the release shipped, and stripping the P/F/L prefixes
dissociates them from the test names underneath. Treat the file's
top-level `//!` block and per-section `// FN — heading` dividers
as immutable; clean only inline body comments that read as
substance, not as section IDs.

### D. Module-level `//!` doc-comments

Same A/B/C treatment, but pay extra attention: these render at the
top of the rustdoc page for the module. Even a trace of `F8 / L7`
in the rendered docs reads as in-progress chatter to a contributor
browsing the API.

### E. CLAUDE.md and the active-release backlog

The next release's CLAUDE.md "Help-text discoverability" section
flips back to using `F<n>` / `L<n>` markers — that's policy for the
*new* in-flight release. The cleanup pass is for the release that
just shipped.

## Scope — what does NOT get normalized

- **`CHANGELOG.md`** — historical record.
- **`archives/`** — preserved verbatim.
- **Test function names** — git-archaeology IDs.
- **Audit JSONL on-disk schema** — entries on real users' disks
  carry the markers in their `verb` / `args` / preview fields and
  the deserializer must continue to accept them. The schema is
  separate from the source.
- **Live release backlog (next release)** — the in-flight one keeps
  its markers throughout development; this doc only ever runs
  against the *just-shipped* release.

## Mechanical pass (recommended)

The volume is large (v0.1.3 had ~800 marker comments across ~100
source files), so a partial-mechanical pass works well:

```sh
# Identify candidates — read every match before editing.
grep -rEnh "(F[0-9]+|L[0-9]+)(\.[0-9]+)?\s*\(v[0-9]+\.[0-9]+\.[0-9]+\)" \
    src/ tests/ --include="*.rs"
```

Class A is regex-replaceable in bulk; Class B requires per-comment
review (skim every match for "pre-Fn" / "legacy" / "older entries"
language and rephrase, don't delete). Class C is trim-and-capitalize.

Commit along **module boundaries** (one commit per top-level `src/`
subdirectory: `src/cli.rs`, `src/verbs/`, `src/safety/`, `src/ssh/`,
`src/discovery/`, etc.). Do **not** bundle the whole sweep into one
mega-commit — review burden is too high and a regression in any
one module forces a revert of the lot.

## Pre-commit gate

The standard gate (`cargo fmt --check && cargo clippy --all-targets
-- -D warnings && cargo test`) must pass on every commit. The cleanup
is comment-only, so test results should be byte-for-byte unchanged.
A diff that touches tests is a red flag — investigate.

## Post-cleanup verification

```sh
# Should return zero matches outside CHANGELOG / archives /
# audit-format-migration code (which legitimately keeps version
# strings as on-disk schema markers).
grep -rEn "(F[0-9]+|L[0-9]+)(\.[0-9]+)?\s*\(v[0-9]+\.[0-9]+\.[0-9]+\)" \
    src/ tests/ docs/ README.md --include="*.rs" --include="*.md"
```

A handful of legitimate hits remain — `inspect help <topic>`
editorial content that intentionally cites a release for users, or
schema-version constants. Audit each one and document why it stays.
