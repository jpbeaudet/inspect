
VERBOSE — Search edge cases

TIMEOUTS AND BUDGETS
  Cross-medium queries fan out per server. The default per-source
  timeout is 30s; when a source exceeds it inspect emits a partial
  result with a `(partial: arte/logs timed out)` footer instead of
  failing the whole query.

  Tunables:
    --timeout <duration>     per-source budget (e.g. 5s, 2m)
    --max-results <N>        global cap; 0 = unlimited
    --concurrency <N>        in-flight sources across the fleet

  For tail-style queries, raise --timeout above the slowest source's
  expected log-reach window; otherwise late lines look like drops.

LABEL DISAMBIGUATION
  When a label name collides between mediums (`source` is reserved,
  but a metric label literally named `source` is legal), prefix it:

    {medium="logs", source="docker"}    # log-stream selector
    {metric="up", source="prom"}        # metric label happens to be 'source'

  The `medium` label is always the disambiguator of last resort.

LOGQL VS METRIC SUBSET
  Metric queries use a strict LogQL subset:
    - rate(), sum(), avg(), max(), min(), count_over_time()
    - by (label) grouping
    - no regex matchers in metric form (use logs medium for that)

  Mixing a metric aggregation with a log selector returns:
    error: cannot mix metric aggregation with log selector
    see: inspect help search

REGEX SAFETY
  Regex matchers (`=~`, `!~`) are bounded by the upstream Loki
  parser; pathological patterns are rejected. inspect adds no extra
  ReDoS defence beyond that — keep patterns anchored where possible.

PIPELINES
  $ inspect search '…' --json | jq …          # canonical machine path
  $ inspect search '…' --csv  | mlr …          # spreadsheet path
  $ inspect search '…' --ndjson | grep …        # streaming filter

  --json buffers; --ndjson streams. Use --ndjson for tail/follow.

SEE ALSO
  inspect help search        the standard topic body
  inspect help formats       output format flags
  inspect help selectors     how the fan-out targets are picked
