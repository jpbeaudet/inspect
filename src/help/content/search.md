SEARCH — LogQL query syntax for cross-medium search

EXAMPLES
  $ inspect search '{server="arte", source="logs"} |= "error"' --since 1h
  $ inspect search '{server=~"prod-.*", service="storage", source="logs"} |= "timeout"'
  $ inspect search '{server="arte", service="atlas", source="logs"} |= "milvus"' --since 30m
  $ inspect search '{server="arte", source="logs"} | json | status >= 500' --since 1h
  $ inspect search 'sum by (service) (count_over_time({server="arte", source="logs"} |= "error" [5m]))'

DESCRIPTION
  The search DSL is LogQL — the same query language used by Grafana
  Loki. Queries are always single-quoted. The `|` characters inside
  belong to the DSL, not to the shell.

RESERVED LABELS
  server    namespace (e.g. "arte", "prod-eu")
  service   container/service tag (or "_" for host-level)
  source    medium: "logs", "file:/path", "dir:/path", "discovery",
            "state", "volume:name", "image", "network", "host:/path"

SELECTORS
  {server="arte", service="pulse", source="logs"}
  Operators: = (exact)  != (not)  =~ (regex)  !~ (not regex)
  Multiple sources: {sel1} or {sel2} or {sel3}
  Aliases: @plogs or @atlas-conf

LINE FILTERS
  |= "literal"     contains
  != "literal"     does not contain
  |~ "regex"       regex match
  !~ "regex"       regex does not match

PIPELINE STAGES (log queries — streaming, record-by-record)
  | json                     parse as JSON
  | logfmt                   parse as key=value
  | pattern "<pattern>"      positional extraction
  | regexp "<regex>"         named-group regex extraction
  | line_format "{{.field}}" reformat output
  | label_format new=expr    add/rename labels
  | <field> <op> <value>     filter on parsed field (==, !=, >, >=, <, <=, =~, !~)
  | drop label1, label2      remove labels
  | keep label1, label2      retain only listed labels
  | map { <sub-query> }      cross-medium chain ($field$ interpolation)

  $field$ inside a `map` block is consumed by inspect, not the shell.
  Always single-quote queries to preserve it.

METRIC QUERIES (aggregations — full window, not streaming)
  count_over_time({...} |= "..." [5m])
  rate({...} [5m])
  sum by (service) (count_over_time({...} |= "error" [5m]))
  topk(5, sum by (service) (rate({...} [1h])))

  A query is either a log query OR a metric query, never both. The
  parser rejects mixes with a clear error pointing at the offending
  position.

FLAGS (work alongside the query string)
  --since <dur>   --until <dur>   --tail <n>   --follow / -f
  --json          --no-color      --timeout <dur>

FULL LOGQL REFERENCE
  https://grafana.com/docs/loki/latest/query/
  inspect aims for behavioral parity with Loki's parser. Mismatches
  are bugs — please file them with the failing query.

SEE ALSO
  inspect help selectors     selector grammar
  inspect help aliases       using @name in queries
  inspect help formats       output format options
  inspect help examples      worked queries and translation guide
