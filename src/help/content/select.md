SELECT — `--select` / `--select-raw` / `--select-slurp` (F19, v0.1.3)

EXAMPLES
  $ inspect status arte --json --select '.data.state'
  $ inspect audit ls --json --select '.data.entries[0].id' --select-raw
  $ inspect audit ls --json --select '.data.entries | length'
  $ inspect compose ls arte --json --select '.data.compose_projects[].name' --select-raw
  $ inspect logs arte/api --json --select '.line' --select-raw
  $ inspect ports arte --json --select 'group_by(.proto) | map({proto: .[0].proto, count: length})' --select-slurp
  $ inspect help all --json --select '.topics[].id' --select-raw

DESCRIPTION
  Every JSON-emitting verb accepts a `--select <FILTER>` flag whose
  argument is a jq-language expression. The filter runs over the
  verb's emitted JSON before bytes hit stdout, so an agent on a
  fresh machine can extract or reshape output without an external
  `jq` install.

  inspect ships a pure-Rust jq implementation (`jaq`) for `--select`
  and the standalone `inspect query <FILTER>` verb. The filter
  language is jq's — every idiom you already know works verbatim.

  Three flags compose. `--select` carries the filter string;
  `--select-raw` and `--select-slurp` are bool modifiers that both
  require `--select`:

    --select '<FILTER>'                          # = jq '<FILTER>'
    --select '<FILTER>' --select-raw             # = jq -r '<FILTER>'
    --select '<FILTER>' --select-slurp           # = jq -s '<FILTER>'
    --select '<FILTER>' --select-raw --select-slurp   # = jq -rs

  `--select` requires a JSON-class output format (`--json` /
  `--jsonl` / `--ndjson`) — combining it with `--csv` / `--md` /
  `--table` is a usage error (exit 2). `--select-raw` and
  `--select-slurp` are mutually compatible.

ENVELOPE PATHS (the ten most common)
  inspect's `--json` envelope is uniform: `{schema_version, summary,
  data, next, meta}`. The verb-specific payload sits under `.data`.
  Cheatsheet:

    .summary                          # one-line human summary
    .data                             # verb-specific payload root
    .data.state                       # F7.5: ok / no_services_matched / empty_inventory
    .data.services                    # F1: per-service array on status / why
    .data.services[] | {name, health} # project a row shape
    .data.entries[0].id               # newest audit-ls entry id (newest-first)
    .data.entries | length            # audit-ls page size
    .data.entry                       # audit-show single-entry payload
    .data.compose_projects[].name     # compose ls project list
    .meta.source.mode                 # F8 cache provenance: live | cached
    .next                             # pagination / next-action hint

  NDJSON-emitting streams (`logs`, `grep`, `find`, `cat`, `search`,
  `run --stream`, `history show`) carry one envelope per line; the
  per-line payload is whatever the verb emits — typically `.line`,
  sometimes `.service`, `.proto`, `.frame`, etc. Use `--select` to
  project per-line, `--select-slurp` to collect frames into a
  single array first.

EXIT CODES
  The select trio shares an exit-code contract with `inspect query`:

    0   filter produced ≥ 1 result
    1   filter produced zero results, runtime error, or
        `--select-raw` on a non-string yield
    2   filter parse error, `--select` without --json/--jsonl,
        or any clap usage error

  Exit 1 is the agent-friendly "no match" signal — same as `grep -q`.
  Exit 2 reserves for "your invocation is malformed; do not retry";
  agents should NOT loop on a 2.

COMMON PITFALLS
  1. SHELL QUOTING. Always single-quote the filter. Double-quoted
     filters get expanded by the shell — `$.id` becomes whatever
     `$` happens to be, and `\(.id)` interpolations break. Single
     quotes pass the filter byte-for-byte to inspect.

  2. `select` THE FLAG vs `select(...)` THE JQ BUILTIN. They are
     unrelated. `--select` is the inspect flag; `select(.healthy)`
     inside the filter is the jq predicate function. Both can
     appear in one invocation:

         $ inspect status arte --json \
             --select '.data.services[] | select(.healthy) | .name'

  3. `--select-raw` REQUIRES STRING YIELDS. `--select-raw` strips
     the JSON quotes from a string yield (`"foo"` → `foo`). A
     numeric, boolean, array, or object yield exits 1 with
     `error: filter --raw: filter yielded non-string`. Either drop
     `--select-raw`, or coerce inside the filter:
     `--select '.count|tostring' --select-raw`.

  4. `--select-slurp` ON ENVELOPE VERBS. The single-envelope verbs
     (`status`, `audit ls`, `compose ls`, `bundle status`, …) emit
     one envelope per invocation; slurping a single value into a
     one-element array (`[.]`) is rarely useful. Slurp is for the
     NDJSON streams (`logs --json`, `ports --json`, …) where
     `length`, `group_by`, `sort_by` need the full set in hand.

  5. PRE-L7 RECIPES (`.[0].id` / `.[0].verb`). `audit ls --json`
     emitted a bare top-level array before the v0.1.3 envelope
     sweep; the recipe `.[0].id` would still parse but indexes
     into a non-array now and yields nothing (exit 1). Use
     `.data.entries[0].id` for the post-sweep envelope. The same
     applies to `audit show`/`grep`/`gc`/`verify` — every audit
     verb is now under the standard envelope.

  6. NULL-SAFE PATHS. A missing field in jq raises `null`, not an
     error. `--select '.data.thing.missing'` on an envelope without
     `thing` yields one `null` line at exit 0 (one result, the
     null), which an agent's `if [[ -z "$out" ]]` check would miss.
     Either guard with `select(. != null)` or use `?` for explicit
     filtering (`.data.thing?.missing? // empty`).

  7. STREAMING ERROR FRAMES (NDJSON verbs). `inspect logs`,
     `inspect grep`, `inspect find`, `inspect cat`, `inspect search`,
     `inspect run --stream --json`, and `inspect history show`
     emit one envelope per line. When the remote command fails to
     start (image missing, container missing, permission denied),
     or when the verb hits an end-of-stream summary frame, the
     emitted envelope does NOT carry the per-line key the operator
     selected. Today's behavior:

         $ inspect run arte --stream --json --select '.line' --select-raw \
             -- "docker logs -f does-not-exist"
         error: filter --raw: filter yielded non-string

     `--select '.line'` yields `null` on the error frame, and
     `--select-raw`'s non-string-yield rejection trips on it. The
     fix is the jq alternative-operator `// empty`, which drops
     `null` and `false` results from the filter output entirely:

         $ inspect run arte --stream --json --select '.line // empty' --select-raw \
             -- "docker logs -f does-not-exist"
         (data lines stream through; error frames are silently dropped; exit code is the verb's exit code, not 1)

     This is the agent-recommended idiom for any per-line projection
     against a streaming verb where a heterogeneous-frame envelope
     is possible. The same shape works for sub-fields:
     `--select '.frame.fields.msg // empty'`. Pair with
     `select(.line | startswith("ERROR"))` to filter both shapes
     and content in one pass.

EXTERNAL `jq` STILL WORKS
  `--select` is sugar; the underlying envelope is unchanged. If you
  prefer `jq` for ad-hoc exploration, every `--json` / `--jsonl`
  output stays pipe-friendly with the external `jq` binary the same
  way it always did. The `inspect setup --discover` probe records
  whether `jq` is installed locally for informational purposes only
  — its absence is no longer a recipe blocker.

INSPECT QUERY (companion verb)
  `inspect query <FILTER>` reads JSON or NDJSON from stdin and
  applies the same engine. Useful for filtering saved envelopes,
  audit log files, or output captured into a tempfile:

    $ inspect status arte --json > /tmp/snap.json   # parse:skip
    $ inspect query '.data.services | length' < /tmp/snap.json   # parse:skip

  See `inspect query --help` for the standalone surface (the same
  flag trio with shorter clap names: `-r` for raw, `-s` for slurp).

SEE ALSO
  inspect help formats       output format flags (--json, --jsonl, ...)
  inspect query --help       standalone filter verb (same engine)
  inspect help safety        envelope shape on audit verbs
