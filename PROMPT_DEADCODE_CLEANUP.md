# Task: Dead Code Cleanup — Full Sweep

You are working on the `inspect` CLI tool (Rust). We have been suppressing dead code warnings since the beginning of the build because we were scaffolding phases incrementally. That scaffolding is now complete. Every feature in v1 scope is implemented. It is time to remove all dead code before field testing.

## Context

- The codebase is ~18,000 lines of Rust across `src/`
- Dead code warnings have been suppressed with `#[allow(dead_code)]` throughout the build
- Some dead code is genuine leftovers from scaffolding phases that got superseded
- Some dead code may be intentionally reserved for v2 features
- We need to distinguish between the two and handle each correctly

## Procedure — Follow These Steps In Order

### Step 1: Inventory

Find and count every dead code suppression in the codebase:

```bash
grep -rn "allow(dead_code)" src/
```

Report the total count and list every file that has suppressions.

### Step 2: Remove All Suppressions

Remove every `#[allow(dead_code)]` attribute from the entire `src/` directory. Do not delete any actual code yet — only remove the suppression attributes.

### Step 3: Compile and Capture Warnings

Run `cargo build` and capture the full warning output. This is the map of all dead code. Report:
- Total number of dead code warnings
- Group them by module/directory (how many in `verbs/`, `search/`, `ssh/`, etc.)

### Step 4: Triage Every Warning

For each dead code warning, determine which category it falls into:

**Category A — DELETE: Truly dead code.**
- Scaffolding helpers that were superseded by later phases
- Functions/structs/enums written for an approach that was later replaced
- Unused imports, unused struct fields, unused enum variants
- Test utilities not actually used in any test

→ Action: Delete the code entirely.

**Category B — GATE: Only used in tests.**
- Helper functions or builders used exclusively by test modules

→ Action: Move behind `#[cfg(test)]` or into `#[cfg(test)] mod tests {}`.

**Category C — KEEP: Intentionally reserved for a documented v2 feature.**
These are features explicitly listed as v2 in the bible:
- OS keychain integration
- Per-user policy enforcement
- TUI mode
- Kubernetes-native discovery
- Distributed tracing
- Pure-russh fallback
- Parameterized/chained aliases

→ Action: Keep the code, but replace the suppression with a **specific** annotation:
```rust
#[allow(dead_code)] // v2: <exact feature name and one-line reason>
```

**If code does not clearly map to a documented v2 feature, it is Category A. Delete it.** When in doubt, delete. Code in version control can be recovered; dead code in the binary cannot be justified.

### Step 5: Verify Zero Warnings

After completing all deletions and annotations, run:

```bash
cargo build 2>&1 | grep "warning" | wc -l
```

The target is **0 warnings**. Not zero via suppression — zero clean, except for the small number of explicitly annotated v2 items.

Also run:
```bash
cargo test
```

Ensure no tests broke from the deletions.

### Step 6: Report

Provide a summary:
- How many `#[allow(dead_code)]` were removed
- How many lines/functions/structs were deleted (Category A)
- How many items were moved to `#[cfg(test)]` (Category B)  
- How many items were kept with v2 annotations (Category C)
- Final warning count after cleanup
- Any tests that broke and how they were fixed
- Total lines of code before and after (`find src/ -name '*.rs' | xargs wc -l | tail -1`)

## Rules

- Do NOT add new `#[allow(dead_code)]` without a v2 justification comment
- Do NOT suppress warnings with `#[allow(unused)]` blanket attributes
- Do NOT comment out code instead of deleting it — that is not cleanup, that is hoarding
- Do NOT skip any warning — every single one gets triaged
- Do NOT break any existing tests — run `cargo test` after every batch of deletions
- Preserve all public API signatures that are actively used by commands
- If deleting dead code causes a cascade (other code that only existed to support the deleted code), follow the cascade and delete that too
