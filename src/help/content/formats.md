FORMATS — Output format options

EXAMPLES
  $ inspect status arte --json
  $ inspect ps 'prod-*' --csv
  $ inspect status arte --md
  $ inspect ps arte --format '{{.service}}\t{{.health}}\t{{.uptime}}'
  $ inspect cat arte/atlas:/etc/atlas.conf --raw

FORMATS
  (default)          Rich table with color and box-drawing
  --json / --jsonl   Line-delimited JSON (NDJSON)
  --csv              RFC 4180 CSV with header row
  --tsv              Tab-separated values with header row
  --yaml             YAML document(s)
  --table            Plain ASCII table (no color, no box-drawing)
  --md               GitHub-flavored Markdown table
  --format '<tpl>'   Go-style template per record ({{.field}})
  --raw              Raw content only (no envelope, no decoration)

  Formats are mutually exclusive. Combining two is a hard error.

TEMPLATE FUNCTIONS (for --format)
  {{.field}}                  field value
  {{.field | upper}}          uppercase
  {{.field | lower}}          lowercase
  {{.field | join ","}}       join list with separator
  {{.field | json}}           render as JSON
  {{.field | len}}            length
  {{.field | default "n/a"}}  fallback value
  {{.field | truncate 40}}    shorten string
  {{.field | ago}}            human-readable time-since
  {{.field | pad 20}}         right-pad for alignment

  Unknown fields render as <none> (kubectl convention) rather than
  failing — templates are forgiving so a missing field on one record
  does not abort the whole stream.

ENVELOPE BEHAVIOR
  --json:     full SUMMARY/DATA/NEXT envelope
  --csv/tsv:  data only (no summary, no next)
  --format:   template only (no envelope)
  --raw:      content only (no decoration)
  default:    summary above, data as table, next below

JSON PROJECTION (F19, v0.1.3)
  Every JSON-emitting verb accepts `--select '<jq filter>'` for
  in-binary projection — no external `jq` install required. Add
  `--select-raw` to strip JSON quotes from string yields (the
  `jq -r` shape) or `--select-slurp` to collect NDJSON frames into
  one array first (the `jq -s` shape). External `jq` still works
  on every `--json` / `--jsonl` output; pre-F19 recipes that pipe
  to `jq` are unchanged. See `inspect help select`.

COLOR
  Respects the standard NO_COLOR env var. The --no-color flag is
  equivalent. Non-TTY output (pipes, redirection) auto-disables
  color regardless of flags.

SEE ALSO
  inspect help examples      format usage in real workflows
  inspect help search        --json envelope shape for search
  inspect help select        --select / --select-raw / --select-slurp
