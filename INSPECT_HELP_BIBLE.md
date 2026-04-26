# Inspect — Help System Bible

**Purpose:** Specification for a world-class CLI help system. Every feature, flag, concept, and pattern in `inspect` must be discoverable from the command line itself — no browser required, no docs site required. A user (or LLM agent) who has only the binary should be able to learn everything.

**Design standard:** Modeled on the best in class — `git help`, `kubectl explain`, `rustup doc`, `tldr`, `man`. Takes the best from each, avoids their mistakes.

---

## 1. Design Principles

1. **Two paths, one destination.** `inspect help <topic>` for prose docs. `inspect <verb> --help` for flag reference. Both exist. Neither is a dead end. They cross-reference each other.

2. **Progressive disclosure.** `inspect help` shows the topic list (30 seconds). `inspect help <topic>` shows the topic (2 minutes). `inspect help <topic> --verbose` shows everything including edge cases (5 minutes). The user controls depth.

3. **Examples first, grammar second.** Every help topic leads with 3-5 copy-pasteable examples. The formal grammar comes after. People learn by pattern, not by BNF.

4. **Errors close the loop.** Every error message includes a `see: inspect help <relevant-topic>` hint. The user who fails lands directly at the documentation that explains why.

5. **Machine-readable.** `inspect help --json` outputs structured help metadata. An LLM agent can discover every verb, flag, topic, and example programmatically without parsing prose.

6. **Searchable.** `inspect help --search <keyword>` finds every topic, verb, flag, and example that mentions the keyword. The user who doesn't know the right topic name can still find what they need.

7. **Offline-complete.** The entire help system is compiled into the binary via `include_str!`. Zero network dependency. Works on air-gapped systems, in Codespaces, on planes.

8. **Consistent voice.** Every help text is written in the same terse, direct style. No marketing language. No "simply" or "easily." Short sentences. Imperative mood for instructions. Indicative mood for descriptions.

---

## 2. Entry Points

### 2.1 `inspect help` — The topic index

The first thing a new user sees. Must fit on one terminal screen (< 40 lines). Lists every topic with a one-line description.

```
$ inspect help

INSPECT — cross-server debugging & hot-fix CLI

Usage:  inspect <command> [selector] [flags]
        inspect help <topic>
        inspect <command> --help

Topics:
  quickstart      Set up your first server in 60 seconds
  selectors       How to address servers, services, and files
  aliases         Save and reuse selectors with @name
  search          LogQL query syntax for cross-medium search
  formats         Output format options (--json, --csv, --md, --format, ...)
  write           Write verbs, dry-run/apply, safety contract
  safety          Audit log, snapshots, revert
  fleet           Multi-server operations
  recipes         Multi-step diagnostic and remediation runbooks
  discovery       Auto-discovery, profiles, drift detection
  ssh             Persistent connections, ControlMaster, passphrases
  examples        Worked examples and translation guide (grep → inspect, etc.)
  all             Print all help topics (long)

Commands:
  Read:   logs grep cat ls find ps status health volumes images network ports
  Write:  restart stop start reload cp edit rm mkdir touch chmod chown exec
  Diag:   why recipe connectivity
  Search: search
  Fleet:  fleet
  Setup:  add remove list show test connect disconnect connections setup
  Alias:  alias
  Audit:  audit
  Other:  revert help version

Run 'inspect <command> --help' for flag details on any command.
Run 'inspect help <topic>' for in-depth documentation on any topic.
Run 'inspect help --search <keyword>' to find help by keyword.
```

### 2.2 `inspect <command> --help` — Flag reference (clap-generated)

Standard clap `--help` output. Terse, flag-focused, auto-generated from the derive macros. Every command gets this for free.

```
$ inspect grep --help

Search content in logs (default) or files

Usage: inspect grep [OPTIONS] <PATTERN> <SELECTOR>

Arguments:
  <PATTERN>   Search pattern (regex by default; -F for fixed string)
  <SELECTOR>  Target: <server>[/<service>][:<path>] or @alias

Options:
      --since <DURATION>   Start of time window (e.g., 30m, 1h, 2d)
      --until <DURATION>   End of time window
      --tail <N>           Last N matches
  -f, --follow             Stream new matches as they arrive
  -i                       Case-insensitive (overrides smart-case)
  -s                       Case-sensitive (overrides smart-case)
  -w                       Match whole words only
  -F                       Fixed string (no regex interpretation)
  -E                       Extended regex
  -A <N>                   Show N lines after each match
  -B <N>                   Show N lines before each match
  -C <N>                   Show N lines before and after each match
  -v, --invert-match       Show lines that do NOT match
  -m, --max <N>            Stop after N matches
  -c, --count              Print match count only
      --json               Output as line-delimited JSON
      --csv                Output as CSV
      --tsv                Output as TSV
      --yaml               Output as YAML
      --table              Plain ASCII table (no color, no box-drawing)
      --md                 GitHub-flavored Markdown table
      --format <TEMPLATE>  Custom output template (Go-style: {{.field}})
      --raw                Raw content only (no envelope)
      --no-color           Disable color output
  -h, --help               Print this help
  
Smart-case: all-lowercase pattern is case-insensitive; any uppercase is case-sensitive.
Override with -i (insensitive) or -s (sensitive).

Examples:
  inspect grep "error" arte --since 1h
  inspect grep "timeout" arte/pulse,atlas -C 3 --tail 50
  inspect grep "error" 'prod-*/storage' --since 1h --json
  inspect grep "config" arte/atlas:/etc/atlas.conf

See also: inspect help selectors, inspect help formats, inspect help examples
```

The `See also:` footer on every `--help` output is the bridge to the topic system. This is how `--help` (flag reference) connects to `help <topic>` (prose docs).

### 2.3 `inspect help <topic>` — Prose documentation

In-depth coverage of a concept. Examples first, then explanation, then edge cases. Every topic follows the same structure:

```
TITLE
  One-line description

EXAMPLES (3-5, copy-pasteable)
  $ command
  $ command
  $ command

DESCRIPTION
  Prose explanation, 10-30 lines

DETAILS (optional, for complex topics)
  Grammar, tables, edge cases

SEE ALSO
  Related topics and commands
```

---

## 3. Topic Catalog

### 3.1 `inspect help quickstart`

```
QUICKSTART — Set up your first server in 60 seconds

EXAMPLES
  $ inspect add arte                              # interactive setup
  $ inspect connect arte                          # one passphrase for the session
  $ inspect status arte                           # what's running, what's healthy
  $ inspect grep "error" arte --since 1h          # find errors
  $ inspect why arte/atlas                        # diagnose a service
  $ inspect edit arte/atlas:/etc/foo 's/old/new/' # preview a fix (dry-run)
  $ inspect edit arte/atlas:/etc/foo 's/old/new/' --apply  # apply it
  $ inspect logs arte/atlas --since 30s --follow  # verify

DESCRIPTION
  1. Install:  curl -sSf https://inspect-cli.dev/install.sh | sh
  2. Add:      inspect add <namespace>  (or set INSPECT_<NS>_HOST/USER/KEY_PATH env vars)
  3. Connect:  inspect connect <namespace>  (passphrase once, reused for the session)
  4. Explore:  inspect status <namespace>
  5. Debug:    inspect grep / logs / search
  6. Fix:      inspect edit / restart / cp  (always dry-run first)
  7. Verify:   inspect logs --follow

  Three tiers of usage — pick the smallest that fits:
    Tier 1:  Verbs (grep, logs, edit, restart) — no DSL, just flags
    Tier 2:  inspect search '<LogQL>' — for cross-medium and pipelined queries
    Tier 3:  --json | jq | xargs — for everything else

SEE ALSO
  inspect help selectors     how to address servers and services
  inspect help search        LogQL query syntax
  inspect help write         write verbs and safety contract
  inspect help examples      translation guide from grep/stern/kubectl/sed
```

### 3.2 `inspect help selectors`

```
SELECTORS — How to address servers, services, and files

EXAMPLES
  arte                          all services on server 'arte'
  arte/pulse                    one service
  arte/pulse,atlas              two services
  arte/storage                  a named group from the profile
  'prod-*/storage'              storage group on every prod-* server
  arte/atlas:/etc/atlas.conf    a file inside a container
  arte/_:/var/log/syslog        a host-level file (no container)
  @plogs                        a saved alias (see: inspect help aliases)

GRAMMAR
  <selector> ::= <server>[/<service>][:<path>]  |  @<alias>

  server:   name | name,name | 'glob-*' | all | '~exclude'
  service:  name | name,name | 'glob-*' | '/regex/' | group | '*' | '~exclude' | _
  path:     /path/to/file | '/path/*.glob'

  _  means "host-level" — for ports, host files, and systemd units.

RESOLUTION ORDER
  1. Container short name (pulse, atlas)
  2. Profile aliases (db → postgres)
  3. Profile groups (storage → [postgres, milvus, redis, minio])
  4. Globs and regex
  5. Subtractive (~name)

  If a name matches both a service and a group, the service wins (with a warning).

EMPTY RESOLUTION
  If a selector matches nothing, inspect lists available servers, services,
  groups, and aliases. Never a silent no-op.

SEE ALSO
  inspect help aliases       save and reuse selectors
  inspect help search        selectors inside LogQL queries
  inspect help fleet         multi-server selectors
```

### 3.3 `inspect help aliases`

```
ALIASES — Save and reuse selectors with @name

EXAMPLES
  $ inspect alias add plogs '{server="arte", service="pulse", source="logs"}'
  $ inspect alias add storage-prod 'prod-*/storage'
  $ inspect alias list
  $ inspect grep "error" @storage-prod --since 1h
  $ inspect search '@plogs |= "error"'
  $ inspect alias remove plogs

DESCRIPTION
  Aliases save a selector under a short @name. Two types:

  Verb-style:   'prod-*/storage'           → works in verb commands
  LogQL-style:  '{server="arte", ...}'     → works in inspect search

  Using the wrong type produces a clear error with the fix.

COMMANDS
  inspect alias add <name> <selector>    define or replace
  inspect alias list                     show all
  inspect alias show <name>              show expansion
  inspect alias remove <name>            delete
  inspect alias check <name>             validate that it still resolves

STORAGE
  ~/.inspect/aliases.toml (mode 600)

LIMITS (v1)
  No parameterization (@logs $service not supported — use shell variables)
  No chaining (@a cannot reference @b)
  No inline let-binding in queries

SEE ALSO
  inspect help selectors     the selector grammar aliases wrap
  inspect help search        using aliases in LogQL queries
```

### 3.4 `inspect help search`

```
SEARCH — LogQL query syntax for cross-medium search

EXAMPLES
  $ inspect search '{server="arte", source="logs"} |= "error"' --since 1h
  $ inspect search '{server=~"prod-.*", service="storage", source="logs"} |= "timeout"'
  $ inspect search '
      {server="arte", service="atlas", source="logs"} or
      {server="arte", service="atlas", source="file:/etc/atlas.conf"}
      |= "milvus"
    ' --since 30m
  $ inspect search '{server="arte", source="logs"} | json | status >= 500' --since 1h
  $ inspect search 'sum by (service) (count_over_time({server="arte", source="logs"} |= "error" [5m]))'

DESCRIPTION
  The search DSL is LogQL — the same query language used by Grafana Loki.
  Queries are always single-quoted. The | inside is the DSL's, not the shell's.

RESERVED LABELS
  server    namespace (e.g., "arte", "prod-eu")
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
  | map { <sub-query> }      cross-medium chain (Splunk-style $field$ interpolation)

  $field$ in map is consumed by inspect, not the shell. Always single-quote queries.

METRIC QUERIES (aggregations — full window, not streaming)
  count_over_time({...} |= "..." [5m])
  rate({...} [5m])
  sum by (service) (count_over_time({...} |= "error" [5m]))
  topk(5, sum by (service) (rate({...} [1h])))

  A query is either a log query OR a metric query, never both.

FLAGS (work alongside the query string)
  --since <dur>   --until <dur>   --tail <n>   --follow / -f   --json   --no-color

FULL LOGQL REFERENCE
  https://grafana.com/docs/loki/latest/query/
  inspect aims for behavioral parity with Loki's parser. Mismatches are bugs.

SEE ALSO
  inspect help selectors     selector grammar
  inspect help aliases       using @name in queries
  inspect help formats       output format options
  inspect help examples      worked examples
```

### 3.5 `inspect help formats`

```
FORMATS — Output format options

EXAMPLES
  $ inspect status arte --json
  $ inspect ps 'prod-*' --csv > fleet.csv
  $ inspect status arte --md | pbcopy
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

  Formats are mutually exclusive. Combining two is an error.

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

  Unknown fields render as <none> (kubectl convention).

ENVELOPE BEHAVIOR
  --json:     full SUMMARY/DATA/NEXT envelope
  --csv/tsv:  data only (no summary, no next)
  --format:   template only (no envelope)
  --raw:      content only (no decoration)
  default:    summary above, data as table, next below

COLOR
  Respects NO_COLOR env var. --no-color flag equivalent.
  Non-TTY output auto-disables color.

SEE ALSO
  inspect help examples      format usage in real workflows
```

### 3.6 `inspect help write`

```
WRITE — Write verbs, dry-run/apply, safety contract

EXAMPLES
  $ inspect restart arte/pulse                       # dry-run (shows what would happen)
  $ inspect restart arte/pulse --apply               # execute
  $ inspect edit arte/atlas:/etc/foo 's/old/new/'    # shows diff (dry-run)
  $ inspect edit arte/atlas:/etc/foo 's/old/new/' --apply
  $ inspect cp ./fix.conf arte/pulse:/etc/pulse.conf --diff
  $ inspect cp ./fix.conf arte/pulse:/etc/pulse.conf --apply

WRITE VERBS
  restart / stop / start / reload    container lifecycle
  cp <local> <sel>:<path>            push file (or pull: cp <sel>:<path> <local>)
  edit <sel>:<path> '<sed-expr>'     in-place content edit (atomic)
  rm / mkdir / touch                 file operations
  chmod / chown                      permission changes
  exec <sel> -- <cmd>                arbitrary command

SAFETY CONTRACT
  1. DRY-RUN BY DEFAULT    No mutation without --apply. Ever.
  2. DIFF FOR EDITS        edit and cp show unified diff before applying.
  3. AUDIT LOG             Every --apply recorded in ~/.inspect/audit/
  4. SNAPSHOTS             Original content saved before mutation.
  5. CONFIRMATION          rm/chmod/chown prompt interactively with --apply.
                           Skip with --yes.
  6. ATOMIC WRITES         edit writes temp file then renames.
  7. LARGE-FANOUT GUARD    >10 targets prompts even with --apply.
                           Skip with --yes-all.

REVERT
  $ inspect audit ls --since today
  $ inspect revert <audit-id>                        # dry-run (shows reverse diff)
  $ inspect revert <audit-id> --apply                # restore original

  If the file changed since your edit (hash mismatch), revert warns
  and requires --force.

SEE ALSO
  inspect help safety        audit log details
  inspect help fleet         write verbs across multiple servers
  inspect help examples      search-then-transform workflows
```

### 3.7 `inspect help safety`

```
SAFETY — Audit log, snapshots, revert

EXAMPLES
  $ inspect audit ls                      # list all mutations
  $ inspect audit ls --since today        # today's mutations
  $ inspect audit show <id>               # show one entry with diff summary
  $ inspect audit grep "atlas"            # search audit entries
  $ inspect revert <audit-id>             # preview revert (dry-run)
  $ inspect revert <audit-id> --apply     # restore original content

AUDIT LOG
  Location:  ~/.inspect/audit/<YYYY-MM>-<user>.jsonl
  Mode:      600 (user-only)
  Format:    One JSON object per line, append-only

  Fields: ts, user, host, verb, selector, args, diff_summary,
          previous_hash, new_hash, snapshot path, exit, duration_ms

SNAPSHOTS
  Location:  ~/.inspect/audit/snapshots/<hash>
  Content:   Original file content before mutation
  Keyed by:  SHA-256 hash (deduplicated across edits)

  Snapshots are what make revert possible. Without the original content,
  a hash alone can't undo a change.

REVERT
  inspect revert <audit-id> restores the file at the recorded selector
  to the snapshot content. Follows the same safety contract:
  - Dry-run by default (shows reverse diff)
  - --apply to execute
  - Audit-logged as a revert
  - If current content doesn't match new_hash, warns + requires --force

LIMITATIONS
  The audit log is forensic, not tamper-proof. A user with file access
  can edit or delete entries. For tamper-proof audit trails in regulated
  environments, forward audit entries to an external log system.

SEE ALSO
  inspect help write         write verbs and the safety contract
```

### 3.8 `inspect help fleet`

```
FLEET — Multi-server operations

EXAMPLES
  $ inspect fleet status                            # all configured servers
  $ inspect fleet status --ns 'prod-*'              # wildcard
  $ inspect fleet status --group production         # named group
  $ inspect fleet restart pulse --ns 'prod-*' --apply

DESCRIPTION
  'inspect search' handles multi-server via LogQL selectors:
    {server=~"prod-.*", source="logs"} |= "error"

  'inspect fleet <verb>' does the same for other verbs:
    inspect fleet status --ns 'prod-*'

NAMESPACE SELECTION
  --ns <pattern>         glob or comma-list
  --group <name>         named group from ~/.inspect/groups.toml
  --exclude-ns <pattern> subtractive

GROUPS
  Defined in ~/.inspect/groups.toml:
    [groups]
    production = ["prod-eu", "prod-us", "prod-asia"]

BEHAVIOR
  Results stream per server as they arrive (no blocking on slowest).
  Failed servers appear with error rows; fleet continues with the rest.
  Exit 0 only if every server succeeded.
  Concurrency capped at INSPECT_FLEET_CONCURRENCY (default 8).

FLEET WRITE VERBS
  Same safety contract. Large-fanout interlock on total target count.
  >10 total targets prompts even with --apply.

SEE ALSO
  inspect help write         safety contract for fleet writes
  inspect help selectors     server-spec patterns
```

### 3.9 `inspect help recipes`

```
RECIPES — Multi-step diagnostic and remediation runbooks

EXAMPLES
  $ inspect recipe deploy-check arte
  $ inspect recipe disk-audit 'prod-*'
  $ inspect recipe cycle-atlas arte/atlas           # mutating recipe (dry-run)
  $ inspect recipe cycle-atlas arte/atlas --apply   # apply all steps

DESCRIPTION
  Recipes are YAML files that sequence multiple inspect commands.
  They turn tribal knowledge ("after a deploy, check these 5 things")
  into repeatable one-liners.

DEFAULT RECIPES (shipped with the binary)
  deploy-check        status + health + error search + connectivity
  disk-audit          volume sizes + log file sizes + image sizes
  network-audit       connectivity matrix + port scan
  log-roundup         errors across all services, last 5 minutes
  health-everything   health check every discovered service

USER RECIPES
  Location: ~/.inspect/recipes/<name>.yaml
  Format:
    name: cycle-atlas
    description: "Edit config, restart, verify"
    mutating: true
    steps:
      - edit '{selector}:/etc/atlas.conf' 's|timeout=30|timeout=60|'
      - restart '{selector}'
      - logs '{selector}' --since 30s --tail 50

  {selector} is replaced by the selector you pass on the command line.
  Mutating recipes require 'mutating: true' and run as dry-run
  unless --apply is passed to the recipe itself.

SEE ALSO
  inspect help write         safety contract for mutating recipes
  inspect why <selector>     built-in diagnostic recipe
```

### 3.10 `inspect help discovery`

```
DISCOVERY — Auto-discovery, profiles, drift detection

EXAMPLES
  $ inspect setup arte                    # full discovery scan
  $ inspect status arte                   # uses cached profile
  $ inspect show arte --profile           # print the profile

DESCRIPTION
  'inspect setup <ns>' connects to the server, scans everything running,
  and produces a profile cached at ~/.inspect/profiles/<ns>.yaml.

  Discovery scans: docker ps/inspect, volumes, networks, images,
  listening ports (ss/netstat), systemd units, health endpoints,
  log driver configuration, and remote tooling (rg, jq, sed).

DRIFT DETECTION
  Every command runs an async drift check in the background.
  If the running container set differs from the cached profile,
  a warning appears (never blocks the foreground command).

REFRESH
  Full re-discovery only on explicit 'inspect setup <ns>'
  or when cache TTL expires (default 7 days).
  Local edits to the profile are preserved across re-discovery.

REMOTE TOOLING
  Discovery probes for rg, jq, journalctl, sed on the remote.
  This determines filter pushdown strategy:
    rg available    → fast remote regex filtering
    grep only       → slower fallback (with hint to install rg)
    journalctl      → used for containers with journald log driver
    sed             → used for remote edits

SEE ALSO
  inspect help ssh           connection and credential management
  inspect help selectors     how discovery feeds service resolution
```

### 3.11 `inspect help ssh`

```
SSH — Persistent connections, ControlMaster, passphrases

EXAMPLES
  $ inspect connect arte                   # one passphrase for the session
  $ inspect connections                    # list active sessions
  $ inspect disconnect arte                # close one
  $ inspect disconnect-all                 # close all

DESCRIPTION
  inspect uses OpenSSH ControlMaster multiplexing. First connection
  prompts for passphrase (if key is encrypted); subsequent commands
  reuse the session via a control socket.

CREDENTIAL RESOLUTION (in order)
  1. Existing inspect-managed control socket (alive) → reuse
  2. User's ~/.ssh/config ControlMaster (alive) → reuse
  3. ssh-agent with key loaded → use
  4. INSPECT_<NS>_KEY_PASSPHRASE_ENV set → read from env
  5. Interactive prompt (rpassword)

CONFIGURATION
  Environment variables (primary):
    INSPECT_<NS>_HOST, _USER, _KEY_PATH, _PORT
    INSPECT_<NS>_KEY_PASSPHRASE_ENV, _KEY_INLINE (base64, CI only)

  Config file: ~/.inspect/servers.toml (mode 600)

  Per-server: persist = true (default), persist_ttl = "4h" (Codespace) / "30m"

CONTROL SOCKETS
  Location: ~/.inspect/sockets/<ns>.sock (mode 600)
  Lifecycle: created on connect, removed on disconnect or TTL expiry
  Stale sockets: auto-detected and cleaned up on next command

SECURITY
  Passphrases never on disk. Keys never inline on disk (env only).
  No password auth. No auto-trust of unknown host keys.
  Socket mode 600. Never shared across users.

SEE ALSO
  inspect help quickstart    first-time setup
  inspect help fleet         multi-server connection management
```

### 3.12 `inspect help examples`

```
EXAMPLES — Worked examples and translation guide

TRANSLATION GUIDE (you know X → here's inspect)

  grep -i "error" file.log
    → inspect grep "error" arte/svc:/path/file.log -i

  stern --since 30m pulse
    → inspect logs arte/pulse --since 30m

  kubectl logs <pod> --since=30m | grep -i error
    → inspect logs arte/<service> --since 30m -i "error"

  ssh box "docker logs pulse --since 30m | grep error"
    → inspect grep "error" arte/pulse --since 30m

  ssh box "sudo sed -i 's/old/new/' /etc/foo.conf"
    → inspect edit arte/_:/etc/foo.conf 's/old/new/' --apply

  scp ./file.conf box:/etc/file.conf
    → inspect cp ./file.conf arte/_:/etc/file.conf --apply

  ssh box "docker restart pulse"
    → inspect restart arte/pulse --apply

  # Loki LogQL:
  {job="varlogs"} |= "error"
    → inspect search '{server="arte", source="logs"} |= "error"'

WORKFLOW EXAMPLES

  # Find errors and restart affected services
  inspect search '{source="logs"} |= "OOM"' --since 5m --json \
    | jq -r '.service' | sort -u \
    | xargs -I{} inspect restart arte/{} --apply

  # Push a config fix across all prod atlas instances
  inspect edit '*/atlas:/etc/atlas.conf' 's|timeout=30|timeout=60|'
  inspect edit '*/atlas:/etc/atlas.conf' 's|timeout=30|timeout=60|' --apply

  # Mixed sources: same pattern in logs AND a config file
  inspect search '
    {server="arte", service="pulse", source="logs"} or
    {server="arte", service="atlas", source="file:/etc/atlas.conf"}
    |= "milvus"
  ' --since 30m

  # Error rate per service (metric query)
  inspect search 'sum by (service) (count_over_time(
    {server="arte", source="logs"} |= "error" [5m]
  ))'

  # Export fleet status as Markdown for a GitHub issue
  inspect fleet status --ns 'prod-*' --md | pbcopy

  # Pipe to fzf for interactive service selection
  inspect ps arte --format '{{.service}}' | fzf | xargs -I{} inspect logs arte/{} --follow

SEE ALSO
  inspect help quickstart    getting started
  inspect help search        LogQL syntax reference
  inspect help write         write verb examples
  inspect help formats       output format options
```

---

## 4. Cross-Cutting Features

### 4.1 Error-to-help linking

Every error message includes a `see:` reference:

```
error: selector 'arte/foo' matched no services.
  Available on arte: pulse, atlas, synapse, neo4j, postgres, milvus, redis, minio
  Groups: storage, knowledge
  see: inspect help selectors

error: expected ',' between label matchers, got 'service'
  {server="arte" service="pulse"}
                  ^^^^^^^
  see: inspect help search

error: alias '@atlas-conf' is a LogQL selector, not a verb selector.
  For verb commands, define a verb-style alias:
    inspect alias add atlas-v 'arte/atlas'
  see: inspect help aliases

error: --json and --csv are mutually exclusive. Pick one output format.
  see: inspect help formats

error: Cannot apply without --apply flag.
  inspect restart arte/pulse --apply
  see: inspect help write
```

### 4.2 `inspect help --search <keyword>`

Full-text search across all help topics, verb descriptions, flag names, and examples.

```
$ inspect help --search timeout

Results:

  inspect help search
    ...--since <dur>   Start of time window (e.g., 30m, 1h, 2d)...
    ...| status >= 500   filter on parsed field...

  inspect grep --help
    ...--since <DURATION>   Start of time window...

  inspect help examples
    ...inspect edit '*/atlas:/etc/atlas.conf' 's|timeout=30|timeout=60|'...

  inspect help ssh
    ...persist_ttl = "4h"...
```

Implementation: at build time, generate a search index from all help texts (simple trigram or keyword index compiled into the binary). At runtime, match the query against the index and print matching topics with context snippets.

### 4.3 `inspect help --json`

Machine-readable help metadata for LLM agents and tooling:

```json
{
  "version": "0.1.0",
  "topics": ["quickstart", "selectors", "aliases", "search", "formats", "write", "safety", "fleet", "recipes", "discovery", "ssh", "examples"],
  "commands": {
    "read": ["logs", "grep", "cat", "ls", "find", "ps", "status", "health", "volumes", "images", "network", "ports"],
    "write": ["restart", "stop", "start", "reload", "cp", "edit", "rm", "mkdir", "touch", "chmod", "chown", "exec"],
    "diagnostic": ["why", "recipe", "connectivity"],
    "search": ["search"],
    "fleet": ["fleet"],
    "setup": ["add", "remove", "list", "show", "test", "connect", "disconnect", "connections", "setup"],
    "alias": ["alias"],
    "audit": ["audit", "revert"]
  },
  "reserved_labels": ["server", "service", "source"],
  "source_types": ["logs", "file:", "dir:", "discovery", "state", "volume:", "image", "network", "host:"],
  "output_formats": ["json", "jsonl", "csv", "tsv", "yaml", "table", "md", "format", "raw"]
}
```

An LLM agent given this JSON + the LogQL spec can construct correct commands without reading prose.

### 4.4 `inspect help all`

Prints every topic sequentially. Long output, meant for piping to a file or pager:

```
$ inspect help all | less
$ inspect help all > inspect-reference.txt
```

### 4.5 `inspect help <topic> --verbose`

Adds edge cases, implementation notes, and caveats that the standard topic omits for brevity. Example: `inspect help ssh --verbose` adds the MaxSessions caveat, the stale-socket recovery mechanism, and the bulk-SCP bandwidth note.

---

## 5. Implementation

### 5.1 Storage

All help texts are Rust string literals or `include_str!` from markdown files in `src/help/`. No external files at runtime.

```
src/help/
  mod.rs                 # topic registry, search index, dispatch
  quickstart.md
  selectors.md
  aliases.md
  search.md
  formats.md
  write.md
  safety.md
  fleet.md
  recipes.md
  discovery.md
  ssh.md
  examples.md
```

### 5.2 The `help` command in clap

```rust
#[derive(Subcommand)]
enum Commands {
    /// Show help on a topic, search help, or list all topics
    Help {
        /// Topic name (e.g., search, selectors, write)
        topic: Option<String>,
        
        /// Search all help for a keyword
        #[arg(long)]
        search: Option<String>,
        
        /// Output as JSON (for LLM agents and tooling)
        #[arg(long)]
        json: bool,
        
        /// Include edge cases and verbose notes
        #[arg(long)]
        verbose: bool,
    },
    // ... other commands
}
```

### 5.3 Search index

At build time (via `build.rs` or a const fn), tokenize every help file into a keyword→topic map. At runtime, `--search` does a substring match against the map and prints matching topics with context. No external crate needed; a simple `HashMap<String, Vec<(TopicId, LineNumber)>>` compiled into the binary is sufficient.

### 5.4 `See also` in --help

Every clap command's `after_help` includes cross-references:

```rust
#[derive(Args)]
#[command(after_help = "See also: inspect help selectors, inspect help formats, inspect help examples")]
struct GrepArgs { ... }
```

### 5.5 Error-to-help linking

The error type carries an optional `help_topic: Option<&'static str>` field. The error renderer appends `see: inspect help <topic>` when present.

```rust
#[derive(Error, Debug)]
enum InspectError {
    #[error("selector '{0}' matched no services")]
    EmptySelector(String, #[help_topic] &'static str),
    // rendered as:
    // error: selector 'arte/foo' matched no services.
    //   see: inspect help selectors
}
```

---

## 6. Quality Standards

1. **Every verb has `--help`.** Auto-generated by clap. Includes examples and `See also`.
2. **Every topic has 3-5 examples.** Examples are copy-pasteable. They use realistic selectors and patterns, not `foo`/`bar`.
3. **Every error has a `see:` reference.** No error leaves the user without a next step.
4. **`inspect help` fits on one screen.** < 40 lines. Topic list + command list. No scrolling required.
5. **`inspect help <topic>` fits in 2 minutes of reading.** If it's longer, split it.
6. **No prose without an example.** If a concept is explained, it's demonstrated.
7. **Consistent formatting.** UPPERCASE for section headers in help output. Monospace for commands and flags. No color in help text (it's piped to `less` often).
8. **Tested.** A CI test verifies: every command has `--help`, every topic in the index resolves, every `See also` reference is valid, and every example in help text is a syntactically valid inspect command.
