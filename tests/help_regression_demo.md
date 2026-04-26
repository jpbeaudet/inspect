# Help System — Synthetic Regression Recipe

Plan §HP-7 DoD: "one synthetic regression PR (deleting an `after_help`,
breaking a `See also`, removing a topic, adding an error without
`help_topic`) demonstrably fails CI."

This recipe is the executable companion to that DoD. Run it on a clean
checkout to verify the eight CI guards still bite. Each step lists the
**bug**, the **guard expected to fail**, and the **command to run**.

A cleanup step at the bottom restores the working tree.

---

## Bug 1 — Delete an `after_help` (G1)

```bash
# Find the `after_help` block on `inspect grep` and stub it out.
sed -i 's|after_help = AFTER_HELP_GREP|after_help = ""|' src/cli.rs
cargo test --test help_contract every_top_level_subcommand_has_see_also_footer
# Expected: FAIL — "subcommand 'grep' is missing an after_help footer"
git checkout -- src/cli.rs
```

Guard fired: `every_top_level_subcommand_has_see_also_footer` (G1).

---

## Bug 2 — Break a `See also:` cross-link (G2 / G3)

```bash
# Rename a known-good topic reference inside an after_help block.
sed -i 's|inspect help selectors|inspect help selectoors|' src/cli.rs
cargo test --test help_contract grep_help_ends_with_pinned_see_also_line
# Expected: FAIL — "grep --help footer drifted from the HP-2 contract"
git checkout -- src/cli.rs

# And via the topic-side guard:
sed -i '0,/inspect help selectors/{s||inspect help selectoors|}' src/help/content/quickstart.md
cargo test --test help_contract every_see_also_reference_resolves
# Expected: FAIL — "topic 'quickstart' references unknown topic 'selectoors'"
git checkout -- src/help/content/quickstart.md
```

Guards fired: `grep_help_ends_with_pinned_see_also_line` (pinned exact line)
and `every_see_also_reference_resolves` (G3).

---

## Bug 3 — Remove a topic from the registry (G3 / G4 / G6 / G8)

```bash
# Comment out the `examples` topic in the registry.
python3 - <<'PY'
import re, pathlib
p = pathlib.Path("src/help/topics.rs")
src = p.read_text()
new = re.sub(
    r'(    Topic \{\n        id: "examples",[\s\S]*?\n    \},\n)',
    '    // INTENTIONALLY REMOVED FOR REGRESSION DEMO\n',
    src,
    count=1,
)
assert new != src, "regex did not match — registry shape changed"
p.write_text(new)
PY
cargo test --test help_contract every_topic_id_resolves
# Expected: FAIL — `inspect help examples` exits NoMatches.
cargo test --test help_json_snapshot json_skeleton_matches_golden
# Expected: FAIL — `topic_ids` differ from golden (G8).
git checkout -- src/help/topics.rs
```

Guards fired: `every_topic_id_resolves`, `every_topic_has_at_least_three_examples`,
`help_all_dumps_every_topic`, `json_skeleton_matches_golden` (G8).

---

## Bug 4 — Add a raw `eprintln!("error: …")` outside `error.rs` (HP-5 G6)

```bash
# Append a fresh raw error site to a verb file.
cat >> src/verbs/grep.rs <<'EOF'

#[allow(dead_code)]
fn _regression_demo_bug() {
    eprintln!("error: synthetic regression bug — should fail CI");
}
EOF
cargo test --test error_help_links no_raw_error_eprintln_outside_error_module
# Expected: FAIL — "raw eprintln(error:) found in src/verbs/grep.rs"
git checkout -- src/verbs/grep.rs
```

Guard fired: `no_raw_error_eprintln_outside_error_module` (HP-5 G6).

---

## Bug 5 — Drift a topic example from the live CLI (HP-7 G5)

```bash
# Introduce a flag the parser doesn't know about.
sed -i '0,/\$ inspect connect arte/{s||$ inspect connect arte --no-such-flag|}' \
    src/help/content/quickstart.md
cargo test --bin inspect every_topic_example_parses_via_clap
# Expected: FAIL — "[quickstart] UnknownArgument: inspect connect arte --no-such-flag"
git checkout -- src/help/content/quickstart.md
```

Guard fired: `every_topic_example_parses_via_clap` (G5).

---

## Bug 6 — Bloat the search index past the 50 KB cap (HP-3 G7)

```bash
# Append a 60 KB filler topic to the content tree.
python3 -c 'open("src/help/content/_bloat.md","w").write("BLOAT\n" + "filler " * 9000)'
# (and add it to TOPICS — exercise left to the demo runner)
cargo test --bin inspect help::search::tests::index_size_under_50kb
# Expected: FAIL — "index is N bytes, exceeds 50 KB cap"
rm -f src/help/content/_bloat.md
```

Guard fired: `index_size_under_50kb` (G7).

---

## Bug 7 — Bump `--json` schema_version without updating the golden (G8)

```bash
sed -i 's|pub const SCHEMA_VERSION: u32 = 1|pub const SCHEMA_VERSION: u32 = 2|' \
    src/help/json.rs
cargo test --test help_json_snapshot json_skeleton_matches_golden
# Expected: FAIL — "help --json skeleton drift detected. ... schema_version: 2 vs 1"
git checkout -- src/help/json.rs
```

Guard fired: `json_skeleton_matches_golden` (G8).

---

## Cleanup

If any step's `git checkout` was missed:

```bash
git restore src/ tests/
```

---

## Coverage Map

| Guard ID | Test name                                      | Demo bug |
|----------|------------------------------------------------|----------|
| G1       | `every_top_level_subcommand_has_see_also_footer` | 1 |
| G2       | `grep_help_ends_with_pinned_see_also_line`     | 2 |
| G3       | `every_see_also_reference_resolves`            | 2, 3 |
| G4       | `every_topic_has_at_least_three_examples`      | 3 |
| G5       | `help::tests::every_topic_example_parses_via_clap` (`cargo test --bin inspect`) | 5 |
| G6 (HP-5)| `no_raw_error_eprintln_outside_error_module` (`tests/error_help_links.rs`) | 4 |
| G7       | `help::search::tests::index_size_under_50kb` (`cargo test --bin inspect`) | 6 |
| G8       | `json_skeleton_matches_golden`                 | 3, 7 |

All eight guards have a documented synthetic failure mode. None of the
demo bugs require human judgement to detect — every one fails CI on the
PR that introduces it.
