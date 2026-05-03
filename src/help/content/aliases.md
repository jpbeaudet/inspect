ALIASES — Save and reuse selectors with @name

EXAMPLES
  # Static aliases (parameterless).
  $ inspect alias add plogs '{server="arte", service="pulse", source="logs"}'
  $ inspect alias add storage-prod 'prod-*/storage'
  $ inspect alias list
  $ inspect grep "error" @storage-prod --since 1h
  $ inspect search '@plogs |= "error"'

  # Parameterized aliases (L3, v0.1.3).
  $ inspect alias add svc-logs '{server="arte", service="$svc", source="logs"}'
  $ inspect search '@svc-logs(svc=pulse) |= "error"'

  # Chained aliases.
  $ inspect alias add prod-pulse '@svc-logs(svc=pulse) |= "$pat"'
  $ inspect search '@prod-pulse(pat=ERROR)'

  $ inspect alias show svc-logs --json
  # → {"name":"svc-logs", "parameters":["svc"], "kind":"logql", ...}

DESCRIPTION
  Aliases save a selector under a short @name. Two types are
  supported and inspect detects which is which from the syntax:

  Verb-style:   'prod-*/storage'           works in verb commands
  LogQL-style:  '{server="arte", ...}'     works in inspect search

  Using the wrong kind in the wrong place produces a clear error
  with a one-line fix suggestion.

PARAMETERIZED ALIASES (L3, v0.1.3)
  Alias bodies may contain placeholders. Three forms are recognized:

    $svc                   required placeholder
    ${svc}                 required placeholder (alternate brace form)
    ${svc:-pulse}          optional placeholder with default "pulse"

  Call sites bind values via `@name(key=val,key=val)`. Bare `@name`
  continues to work for parameterless aliases.

  $ inspect alias add svc-logs '{server="arte", service="$svc"}'
  $ inspect search '@svc-logs(svc=pulse) |= "error"'

  $ inspect alias add svc-default '{server="arte", service="${svc:-pulse}"}'
  $ inspect search '@svc-default |= "error"'         # uses default
  $ inspect search '@svc-default(svc=atlas) |= "x"'  # overrides

  • Placeholder names are `[a-zA-Z_][a-zA-Z0-9_]*`.
  • `$$` is a literal `$` (escape).
  • Default values may not contain `}` directly; use `\}` to embed one.
  • Quoted param values let you embed commas: `@a(pat="foo,bar")`.
  • Missing required params (no default) at the call site → exit 2
    with the declared param list printed. Extra params (not declared
    in the body) → exit 2.

  Aliases may chain other aliases up to depth 5. Definitional
  cycles are rejected at `alias add` time (the cycle is printed
  back as `a -> b -> a`). Runtime depth-cap fires if a hand-edited
  `aliases.toml` introduces a chain longer than 5.

  Agent discovery: `inspect alias show <name> --json` includes
  `parameters: []` listing the placeholders the alias declares.
  `inspect alias list --json` includes the same field per entry.

COMMANDS
  inspect alias add <name> <selector>    define or replace
  inspect alias list                     show all (with param hints)
  inspect alias show <name>              show full detail (+ params)
  inspect alias remove <name>            delete

STORAGE
  ~/.inspect/aliases.toml (mode 600). Pre-L3 entries (no
  `parameters` field on disk) deserialize unchanged and are treated
  as parameterless.

SEE ALSO
  inspect help selectors     the selector grammar aliases wrap
  inspect help search        using aliases in LogQL queries
