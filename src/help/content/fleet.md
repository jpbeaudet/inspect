FLEET — Multi-server operations

EXAMPLES
  $ inspect fleet --ns '*' status                         # all configured servers
  $ inspect fleet --ns 'prod-*' status                    # wildcard
  $ inspect fleet --ns '@production' status               # named group
  $ inspect fleet --ns 'prod-*' restart pulse --apply

DESCRIPTION
  `inspect search` already handles multi-server via LogQL selectors:
    {server=~"prod-.*", source="logs"} |= "error"

  `inspect fleet <verb>` does the same for the other verbs:
    inspect fleet --ns 'prod-*' status

NAMESPACE SELECTION
  --ns <pattern>          glob or comma-list
  --group <name>          named group from ~/.inspect/groups.toml
  --exclude-ns <pattern>  subtractive

GROUPS
  Defined in ~/.inspect/groups.toml:
    [groups]
    production = ["prod-eu", "prod-us", "prod-asia"]

BEHAVIOR
  Results stream per server as they arrive (no blocking on the
  slowest). Failed servers appear with error rows; the fleet run
  continues with the rest. Exit code is 0 only when every server
  succeeded. Concurrency is capped at INSPECT_FLEET_CONCURRENCY
  (default 8).

FLEET WRITE VERBS
  Same safety contract as single-server writes. The large-fanout
  interlock applies to the total target count: >10 total targets
  prompts even with --apply. Skip with --yes-all.

SEE ALSO
  inspect help write         safety contract for fleet writes
  inspect help selectors     server-spec patterns
  inspect help recipes       fleet-aware diagnostic runbooks
