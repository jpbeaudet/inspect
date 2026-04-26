ALIASES — Save and reuse selectors with @name

EXAMPLES
  $ inspect alias add plogs '{server="arte", service="pulse", source="logs"}'
  $ inspect alias add storage-prod 'prod-*/storage'
  $ inspect alias list
  $ inspect grep "error" @storage-prod --since 1h
  $ inspect search '@plogs |= "error"'
  $ inspect alias remove plogs

DESCRIPTION
  Aliases save a selector under a short @name. Two types are
  supported and inspect detects which is which from the syntax:

  Verb-style:   'prod-*/storage'           works in verb commands
  LogQL-style:  '{server="arte", ...}'     works in inspect search

  Using the wrong kind in the wrong place produces a clear error
  with a one-line fix suggestion.

COMMANDS
  inspect alias add <name> <selector>    define or replace
  inspect alias list                     show all
  inspect alias show <name>              show expansion
  inspect alias remove <name>            delete
  inspect alias check <name>             validate that it still resolves

STORAGE
  ~/.inspect/aliases.toml (mode 600)

LIMITS (v1)
  No parameterization (`@logs $service` is not supported — use shell
  variables in the selector you pass, or a recipe).
  No chaining (@a cannot reference @b).
  No inline let-binding inside a single query.

SEE ALSO
  inspect help selectors     the selector grammar aliases wrap
  inspect help search        using aliases in LogQL queries
  inspect help recipes       parameterized multi-step alternatives
