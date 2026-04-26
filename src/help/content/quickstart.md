QUICKSTART — Set up your first server in 60 seconds

EXAMPLES
  $ inspect add arte                                      # interactive setup
  $ inspect connect arte                                  # one passphrase for the session
  $ inspect status arte                                   # what's running, what's healthy
  $ inspect grep "error" arte --since 1h                  # find errors
  $ inspect why arte/atlas                                # diagnose a service
  $ inspect edit arte/atlas:/etc/foo 's/old/new/'         # preview a fix (dry-run)
  $ inspect edit arte/atlas:/etc/foo 's/old/new/' --apply # apply it
  $ inspect logs arte/atlas --since 30s --follow          # verify

DESCRIPTION
  1. Add:      `inspect add <namespace>` (or set `INSPECT_<NS>_HOST`,
               `INSPECT_<NS>_USER`, `INSPECT_<NS>_KEY_PATH` env vars).
  2. Connect:  `inspect connect <namespace>` — passphrase once, reused for
               the session via OpenSSH ControlMaster.
  3. Discover: `inspect setup <namespace>` builds a profile of every
               container, volume, network, and listening port.
  4. Explore:  `inspect status`, `inspect ps`, `inspect logs`.
  5. Debug:    `inspect grep`, `inspect search`, `inspect why`.
  6. Fix:      `inspect edit`, `inspect cp`, `inspect restart` —
               always dry-run first, add `--apply` to execute.
  7. Verify:   `inspect logs --follow`.

  Three tiers of usage — pick the smallest that fits the task:
    Tier 1:  Verbs (`grep`, `logs`, `edit`, `restart`) — no DSL, just flags
    Tier 2:  `inspect search '<LogQL>'` — for cross-medium pipelined queries
    Tier 3:  `--json | jq | xargs` — for scripted automation

SAFETY
  Every write verb is dry-run by default. Adding `--apply` is the only
  way to mutate state. `inspect exec` additionally requires
  `--allow-exec` so a misclick cannot shell out across the fleet.
  Every mutation is recorded in `~/.inspect/audit/` with a snapshot of
  the original content; `inspect revert <audit-id>` restores it.

SEE ALSO
  inspect help selectors     how to address servers and services
  inspect help search        LogQL query syntax
  inspect help write         write verbs and the safety contract
  inspect help examples      translation guide from grep / stern / kubectl / sed
