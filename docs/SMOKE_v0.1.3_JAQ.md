# SMOKE v0.1.3 — JAQ live sweep

Companion to `docs/SMOKE_v0.1.3.md`. Run **after** P0–P7 of the main
smoke have all passed cleanly **and** the P8 follow-ups (P8-A through
P8-D) are tagged ✅ in their tracker. This sheet exercises the F19
`--select` chokepoint comprehensively against a real arte session
before we re-run the regular smoke against the v0.1.3-jaq build.

> **Why a separate sheet.** Per `CLAUDE.md` smoke-scope discipline,
> the regular runbook is the gate; an agent that wanders off to
> "just check X" loses the systematic-coverage signal. This sheet
> *is* that systematic coverage for the F19 surface specifically:
> 30 recipes that together touch every common jq idiom, every
> JSON-emitting verb class, both `--select-raw` and `--select-slurp`,
> and the streaming-vs-envelope split. Once it's green we know
> F19's chokepoint is sound across the board and the regression
> re-smoke can focus on non-F19 surfaces.

## Pre-flight

Same setup as the main smoke. In particular:

```sh
export SMOKE_CTR=inspect-smoke-redis
export PATH="$HOME/.cargo/bin:$PATH"   # WSL
inspect connect arte                    # P8-C: produces a transcript block now
inspect status arte --json | head -1   # sanity: arte responds
```

If P3/P4 of the main smoke have not yet run in this session, several
audit-targeting recipes (B-AUDIT.\*, D.\*) will report `NoMatches`
rather than fail — they assume the audit log carries at least one
`run`-class write and one `restart`-class write. Either run P3/P4
first or substitute another verb that has run today.

---

## Section A — Core jq idioms (12 recipes)

These exercise the **language**, not specific verbs. Each recipe uses
`audit ls --limit 5 --json` as its source so the byte stream is
deterministic across consecutive runs (read-only against an immutable
on-disk projection — same property the P8.1 round-trip recipe uses).

```sh
# (A1) Identity. The unfiltered envelope and `--select '.'` must
# produce byte-identical streams (modulo trailing newline). Pins
# the F19 round-trip identity at the most-common chokepoint.
inspect audit ls --limit 5 --json > /tmp/inspect-jaq-A1-plain.json
inspect audit ls --limit 5 --json --select '.' > /tmp/inspect-jaq-A1-id.json
diff -q /tmp/inspect-jaq-A1-plain.json /tmp/inspect-jaq-A1-id.json
# Pass: files differ at most by a trailing newline.

# (A2) Dotted path. The summary string is a top-level field on
# every envelope verb. Trip-wire for a regression that drops the
# .summary key.
inspect audit ls --limit 5 --json --select '.summary' --select-raw
# Pass: prints exactly one non-empty line, exit 0.

# (A3) Array iteration with .foo[].
inspect audit ls --limit 5 --json --select '.data.entries[].verb' --select-raw
# Pass: prints up to 5 verb names, one per line, exit 0.

# (A4) Index. .data.entries[0] reaches the newest entry (audit ls
# is sorted newest-first, per CLAUDE.md "audit ordering" invariant).
inspect audit ls --limit 5 --json --select '.data.entries[0].id' --select-raw
# Pass: one ULID-shaped id, exit 0.

# (A5) length. Aggregate over an array.
inspect audit ls --limit 5 --json --select '.data.entries | length'
# Pass: an integer 1..5 on stdout, exit 0.

# (A6) keys (and keys_unsorted). Surfaces the envelope shape.
# IMPORTANT: do NOT add `--select-slurp` here. `audit ls --json` emits
# a single envelope; slurping wraps it in `[envelope]`, after which
# `.data` tries to index an array by string and trips
# `error: filter runtime: cannot index […] with "data"`. This is
# pitfall #4 in `inspect help select` ("--select-slurp on envelope
# verbs"); the recipe deliberately exercises the slurp-free shape.
inspect audit ls --limit 5 --json --select '.data | keys'
# Pass: a JSON array containing "entries" (and any other top-level
# data fields). Live arte projection on 2026-05-09 returned ["entries"].

# (A7) select(...) predicate. Filter rows by a field value.
inspect audit ls --limit 25 --json \
  --select '.data.entries[] | select(.exit == 0) | .id' --select-raw \
  | head -3
# Pass: up to 3 ids on stdout, exit 0. (head closing the pipe must
# NOT trigger a panic — pins the SIGPIPE invariant.)

# (A8) map(...). Transform every element.
inspect audit ls --limit 5 --json \
  --select '.data.entries | map({id, verb})'
# Pass: a JSON array of {id, verb} objects, exit 0.

# (A9) group_by + map. Multi-stage projection — same shape as the
# select.md ports example but on a deterministic source.
inspect audit ls --limit 25 --json \
  --select '.data.entries | group_by(.verb) | map({verb: .[0].verb, count: length})'
# Pass: a JSON array of {verb, count} objects, one per distinct
# verb in the last 25 audit rows, exit 0.

# (A10) sort_by — string ordering on a deterministic field.
# IMPORTANT: sort by `.ts` (RFC3339 string), NOT `.duration_ms`. The
# `audit ls` envelope ships a *compact projection* per entry that
# omits `duration_ms` (see A11 — the projection is exactly 10 fields:
# diff_summary, exit, id, is_revert, reason, revert, selector, server,
# ts, verb). Sort_by on a missing field returns null for every row,
# the sort is stable, and `.[-1]` would silently pick a tiebreak —
# correct jaq behavior, but uninformative. The full duration_ms is
# at `audit show <id>`. RFC3339 timestamps lex-sort correctly without
# a fromdate coercion, so `.ts` is the right surrogate here.
inspect audit ls --limit 25 --json \
  --select '.data.entries | sort_by(.ts) | .[0] | {id, verb, ts}'
# Pass: a single {id, verb, ts} object — the OLDEST of the 25, exit
# 0. Live arte projection on 2026-05-09 returned a 2026-05-05
# connect.reauth entry as oldest in a 16-entry audit log.

# (A11) to_entries / from_entries. Object ↔ array roundtrip.
inspect audit ls --limit 1 --json \
  --select '.data.entries[0] | to_entries | map(.key) | sort'
# Pass: a JSON array of the entry's field names (id, ts, verb,
# selector, …) sorted, exit 0.

# (A12) // empty (alternative). The pitfall #7 idiom on a synthetic
# missing field. Drops the null entirely so an agent's
# `read -r` loop doesn't see a "null" line.
{
  inspect audit ls --limit 1 --json \
    --select '.data.entries[0].nonexistent_field // empty' --select-raw
  echo "EOF"
} | tee /tmp/inspect-jaq-A12.out
test "$(grep -c '^null$' /tmp/inspect-jaq-A12.out)" = "0"
# Pass: stdout is just "EOF" — the missing-field projection emitted
# zero output lines (// empty replaced the null with no result),
# exit 0. Pre-fix recipe `--select '.data.entries[0].nonexistent_field'`
# would have emitted "null" + "EOF".
```

Section A pass criterion (rolled up): every recipe exits 0; each `Pass:` shape matches.

---

## Section B — Per-verb envelope-shape sweep (13 recipes)

Each recipe targets one JSON-emitting verb and pins one envelope
contract via `--select`. Coverage is biased toward verbs that hold
operator-state (audit, history, compose, status) where a regression
in the chokepoint would silently reshape an agent-facing contract.

### B-CORE — status / why / ps

```sh
# (B1) status — the most-driven envelope. .data.state is the F7.5
# discriminator — pin its value space.
S=$(inspect status arte --json --select '.data.state' --select-raw)
case "$S" in ok|no_services_matched|empty_inventory) echo "B1 ok: $S";;
  *) echo "B1 FAIL: unexpected state $S"; exit 1;; esac

# (B2) why — diagnostic walk. The why payload's checks block is
# a top-level object on .data; iterate it.
inspect why arte/${SMOKE_CTR} --json \
  --select '.data | keys' --select-slurp
# Pass: a JSON array of the why fields, exit 0.

# (B3) ps — service inventory.
inspect ps arte --json \
  --select '.data.services[] | select(.healthy) | .name' --select-raw \
  | head -3
# Pass: up to 3 healthy service names on stdout, exit 0.
```

### B-INFRA — ports / volumes / images / network

```sh
# (B4) ports — NDJSON-y but envelope-emitting on --json. Test
# without --jsonl. Counts protocol distribution.
inspect ports arte --json \
  --select '[.[] | .proto] | group_by(.) | map({proto: .[0], count: length})' \
  --select-slurp
# Pass: a JSON array of {proto, count} objects, exit 0.

# (B5) volumes — list with size projection.
inspect volumes arte --json \
  --select '.data.volumes[] | {name, driver}' \
  --select-slurp \
  --select-raw 2>&1 | head -3 || true
# Pass: --select-raw on a non-string (object) yield exits 1 with
# "filter --raw: filter yielded non-string". Drop --select-raw:
inspect volumes arte --json \
  --select '.data.volumes[] | {name, driver}' --select-slurp \
  | head -50
# Pass: a JSON array of {name, driver} objects, exit 0.

# (B6) images — image inventory. .data.images[].repo_tags is itself
# an array; flatten it.
inspect images arte --json \
  --select '[.data.images[].repo_tags[]] | unique | length'
# Pass: a single integer = distinct repo:tag count across all images,
# exit 0.

# (B7) network — list networks.
inspect network arte --json \
  --select '.data.networks[].name' --select-raw \
  | wc -l
# Pass: an integer ≥ 1 (Docker always provisions bridge/host/none),
# exit 0.
```

### B-AUDIT — audit ls / show / grep / gc / verify

These rely on P3/P4 having run in the current session.

```sh
# (B8) audit ls — newest-first ordering pin. The first id from
# --limit 25 must equal the bare --limit 1 result.
A=$(inspect audit ls --limit 25 --json --select '.data.entries[0].id' --select-raw)
B=$(inspect audit ls --limit 1  --json --select '.data.entries[0].id' --select-raw)
test "$A" = "$B" && echo "B8 ok: $A" || echo "B8 FAIL: $A != $B"

# (B9) audit show — single-entry payload at .data.entry. Pick a
# fresh-ish write entry so .data.entry.revert is present (F11
# capture-before-apply guarantees this for write verbs).
WRITE_ID=$(inspect audit ls --limit 50 --json \
  --select '[.data.entries[] | select(.verb == "run") | select(.is_revert == false) | .id][0]' \
  --select-raw)
test -n "$WRITE_ID" && \
  inspect audit show "$WRITE_ID" --json \
    --select '.data.entry | {id, verb, revert: .revert.kind}' \
    --select-slurp \
  || echo "B9 SKIP: no run-class audit entry yet"

# (B10) audit grep — regex over verb names. .data.entries shape.
inspect audit grep '^run\b' --json \
  --select '.data.entries | length'
# Pass: an integer ≥ 0, exit 0.

# (B11) audit gc --dry-run — gc projection.
inspect audit gc --dry-run --json \
  --select '.data | keys' --select-slurp
# Pass: a JSON array containing at least "candidates" / "kept" / similar
# keys (exact set verb-version dependent), exit 0.

# (B12) audit verify — chain integrity check.
inspect audit verify --json \
  --select '.data.ok'
# Pass: `true` (or a numeric/string verb-version-dependent ok signal),
# exit 0.
```

### B-COMPOSE — compose ls / ps

```sh
# (B13) compose ls + compose ps cross-link. ls gives projects;
# ps drills into one project's services.
PROJECT=$(inspect compose ls arte --json \
  --select '.data.compose_projects[0].name' --select-raw)
test -n "$PROJECT" && \
  inspect compose ps arte --project "$PROJECT" --json \
    --select '.data.services | length' \
  || echo "B13 SKIP: no compose projects on arte"
# Pass: an integer ≥ 1 (project has at least one service), exit 0.
```

Section B pass criterion: every recipe either prints its expected
shape on stdout exit 0, or prints an explicit `SKIP` line for the
session-state-dependent ones (B9, B13).

---

## Section C — NDJSON streaming sweep (5 recipes)

These exercise the **per-line** chokepoint, including the P8-B
`// empty` null-safety idiom on a real streaming verb against arte.

```sh
# (C1) logs --json --select '.line // empty' --select-raw — the
# documented streaming idiom. Use a known-running container so the
# stream produces real lines.
timeout 3 inspect logs arte/${SMOKE_CTR} --json \
  --select '.line // empty' --select-raw \
  | head -5 || true
# Pass: up to 5 log lines on stdout, exit 0 (or 124 from timeout).
# No "filter --raw: filter yielded non-string" in stderr.

# (C2) logs with a ranking projection inside --select. Slurps the
# full set, sorts by length, takes the longest 3.
timeout 3 inspect logs arte/${SMOKE_CTR} --since 5m --json \
  --select '[.line // empty] | sort_by(length) | .[-3:]' --select-slurp \
  || true
# Pass: a JSON array of up to 3 strings, exit 0.

# (C3) grep --json — content match across files. Use a path that
# exists in every container shape.
inspect grep --json arte/${SMOKE_CTR} '/etc/hostname' --pattern '.' \
  --select '.line // empty' --select-raw 2>&1 | head -3 || true
# Pass: at most a few lines (hostname files are small), exit 0.
# Adjust path if /etc/hostname is missing.

# (C4) find --json — shape pinning for discovery.
inspect find arte/${SMOKE_CTR} '/' --name '*.conf' --json \
  --select '.path // empty' --select-raw 2>&1 | head -5 || true
# Pass: up to 5 .conf paths on stdout (varies by container), exit 0.

# (C5) run --stream --json --select '.line // empty' --select-raw —
# the F19 × F16 × P8-B intersection. Adds back the P8.5 SIGINT
# round-trip but with the null-safe filter so error frames don't
# poison the stream.
timeout --signal=INT 5 inspect run arte --stream --json \
  --select '.line // empty' --select-raw \
  -- "docker logs -f ${SMOKE_CTR}" || true
inspect run arte -- "ps -ef | grep 'docker logs -f ${SMOKE_CTR}' \
  | grep -v grep | wc -l"   # expect 0
# Pass: zero orphaned processes; no filter-rejection lines in stderr.
```

Section C pass criterion: each recipe streams real lines (or completes
empty) without a filter-rejection error in stderr; the SIGINT
round-trip leaves zero orphans.

---

## Section D — Cross-verb correlation (3 recipes)

These exercise **chains** across verbs — the shape an agent uses to
build complex workflows. Each chain ends with a non-trivial assertion.

```sh
# (D1) audit ls → audit show round-trip via --select-raw → --json.
# Pulls the newest write id, fetches its detail, asserts the verb
# and id match. Pins both the F19 raw-strip and the audit envelope.
NEWEST=$(inspect audit ls --limit 1 --json \
  --select '.data.entries[0].id' --select-raw)
SHOWN_ID=$(inspect audit show "$NEWEST" --json \
  --select '.data.entry.id' --select-raw)
test "$NEWEST" = "$SHOWN_ID" && echo "D1 ok: $NEWEST" \
  || echo "D1 FAIL: $NEWEST != $SHOWN_ID"

# (D2) audit ls → history show --audit-id round-trip. Same shape
# as P8 recipe (4) but asserted explicitly. Pins the P8-C fix:
# every audit entry from a namespace verb must be findable in the
# F18 transcript by its id.
NEWEST_NS=$(inspect audit ls --limit 25 --json \
  --select '[.data.entries[] | select(.selector | startswith("arte")) | .id][0]' \
  --select-raw)
test -n "$NEWEST_NS" && \
  inspect history show arte --audit-id "$NEWEST_NS" | head -3 \
  || echo "D2 SKIP: no arte-scoped audit in last 25"
# Pass: at least one transcript line printed; exit 0.

# (D3) compose ls → compose ps → status correlation. Project from
# compose ls, drill into services via compose ps, cross-check that
# `status` reports the same service set.
PROJECT=$(inspect compose ls arte --json \
  --select '.data.compose_projects[0].name' --select-raw)
test -n "$PROJECT" || { echo "D3 SKIP: no compose projects"; exit 0; }
COMPOSE_SVCS=$(inspect compose ps arte --project "$PROJECT" --json \
  --select '.data.services[].name' --select-raw | sort | tr '\n' ' ')
STATUS_SVCS=$(inspect status arte --json \
  --select '.data.services[].name' --select-raw | sort | tr '\n' ' ')
echo "D3 compose svcs: $COMPOSE_SVCS"
echo "D3 status  svcs: $STATUS_SVCS"
# Pass: every service in COMPOSE_SVCS appears in STATUS_SVCS
# (status may include extra non-compose-managed containers; that's
# fine — the inclusion direction is what matters).
```

Section D pass criterion: D1 round-trip equality holds; D2 surfaces
transcript lines (exits successfully); D3's compose service set is a
subset of the status set.

---

## Wrap-up: post-sweep gate

Once Sections A–D all pass, log the result and proceed to the
regression re-smoke:

```sh
echo "JAQ sweep PASS at $(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  | tee -a /tmp/inspect-smoke-jaq-results.log
```

Then run `docs/SMOKE_v0.1.3.md` end-to-end (P0–P7) against the
v0.1.3-jaq build. The goal of the regression smoke is to confirm
that:

1. The P8-D stderr-surface fix changes the visible behavior of
   streaming-failure paths (now you see remote stderr; previously
   you didn't) without regressing the success-path TTY merging.
2. The P8-C connect/disconnect transcript-block fix produces blocks
   on every namespaced invocation (verify by running
   `inspect history list arte` after a few connect/disconnect
   cycles).
3. The reauth ordering fix is observable: trigger an F13 reauth
   (let a master expire, then run a verb), and confirm the
   resulting transcript block's `audit_id=` footer points at the
   verb's primary audit, not the `connect.reauth` side-effect.
4. P8-B's `// empty` recipe (P8.6 in the main smoke) passes
   without filter-rejection lines.

If all four of those land cleanly, v0.1.3-jaq is ready for tag
review. Open follow-ups go to v0.1.5+ per the no-silent-deferrals
policy.

---

## Triage cheatsheet (when a recipe fails)

| Symptom | Likely cause | Where to look |
|---|---|---|
| `error: filter parse error: …` | invalid jq syntax in the recipe | quote the filter; check shell expansion |
| `error: filter --raw: filter yielded non-string` | `--select-raw` on object/array/null yield | drop `--select-raw` or coerce with `tostring`; use `// empty` for nullable paths |
| `error: --select requires a JSON-class output …` | missing `--json` on the verb | add `--json` (the F19 mutex check) |
| `Cannot index object with number` | pre-L7 envelope recipe (`.[0]` instead of `.data.entries[0]`) | read the verb's envelope path in `inspect help select` |
| `0 blocks match --audit-id …` | F18 transcript footer mismatch | re-check P8-C (connect/disconnect blocks; reauth ordering) |
| streaming verb hangs | no `timeout` wrapper or unbounded source | wrap in `timeout 3`; confirm container exists |
| `arte: exit N` with empty DATA | P8-D: remote stderr; should now surface inline | look at the next stderr line; pre-P8-D builds dropped it |

---

*This sheet is generated against the v0.1.3-jaq branch state as of
2026-05-07. If the F19 surface gains new flags or a verb's envelope
changes shape, update the relevant Section before re-running.*
