RECIPES — Multi-step diagnostic and remediation runbooks

EXAMPLES
  $ inspect recipe deploy-check --sel arte
  $ inspect recipe disk-audit --sel 'prod-*'
  $ inspect recipe cycle-atlas --sel arte/atlas           # dry-run (mutating)
  $ inspect recipe cycle-atlas --sel arte/atlas --apply   # apply all steps

DESCRIPTION
  Recipes are YAML files that sequence multiple inspect commands.
  They turn tribal knowledge ("after a deploy, check these 5
  things") into repeatable one-liners that can be reviewed,
  versioned, and shared.

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

  {selector} is replaced by the selector you pass on the command
  line. Mutating recipes require `mutating: true` and run as a
  dry-run unless --apply is passed to the recipe itself.

RELATED
  `inspect why <selector>` is a built-in diagnostic recipe — it
  walks status, health, recent errors, and connectivity for the
  selected service.

SEE ALSO
  inspect help write         safety contract for mutating recipes
  inspect help fleet         recipes parameterised by namespace
  inspect help examples      worked diagnostic flows
