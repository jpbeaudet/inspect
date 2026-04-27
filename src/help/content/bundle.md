# bundle — multi-step orchestration with rollback

`inspect bundle` runs an ordered sequence of steps declared in a YAML
file. Each step is one of `exec` (audited, destructive),
`run` (read-only), or `watch` (block until a condition holds). Steps
can require predecessors, fan out as a parallel matrix, and ship a
compensating `rollback:` command for use on failure.

Two subcommands:

```bash
inspect bundle plan deploy.yaml
inspect bundle apply deploy.yaml --apply --reason 'INC-1234 deploy api'
```

`plan` validates the YAML, interpolates `{{ vars.* }}` and matrix
values, and prints the rendered step list. It never touches a remote.
`apply` runs preflight checks, then steps in declaration order, then
postflight checks. Destructive bundles refuse without `--apply`. CI
should pass `--no-prompt` so failure handling never stalls on stdin.

## YAML schema

```yaml
name: deploy-api          # required
host: arte/api            # default selector for every step
reason: 'INC-1234'        # default reason; --reason CLI flag overrides

vars:                     # arbitrary scalar/map/list, addressable
  service: api            # via {{ vars.service }} in any string field
  version: '1.4.2'

preflight:                # all must pass before step 1 runs
  - check: disk_free
    path: /var/lib/docker
    min_gb: 5
  - check: services_healthy
    services: [api, worker]
    timeout: 60s

steps:
  - id: backup            # unique per bundle; referenced by `requires:`
    exec: pg_dump app > /backup/{{ vars.version }}.sql
    reversible: false     # don't try to delete the backup on rollback

  - id: deploy
    exec: docker compose pull && docker compose up -d
    requires: [backup]
    rollback: docker compose up -d --force-recreate api:{{ vars.version }}_prev
    on_failure: rollback  # abort | continue | rollback | rollback_to: <id>

  - id: smoke
    watch:
      until_http: http://api/health
      match: status == 200
      timeout: 2m
    requires: [deploy]

  - id: warm-cache
    parallel: true
    matrix:
      shard: [a, b, c, d]   # one branch per shard, run concurrently
    max_parallel: 2         # bound concurrency (hard cap = 8)
    exec: curl -fsS http://api/_warm/{{ matrix.shard }}

rollback:                 # bundle-level rollback block; runs in reverse
  - id: rollback-config   # only for `on_failure: rollback`, NOT for
    exec: cp /etc/api.cfg.bak /etc/api.cfg   # rollback_to.

postflight:               # informational; failures DO NOT trigger rollback
  - check: http_ok
    url: http://api/health
```

## Step body kinds

* `exec:` — single shell command. Audited (verb=`exec`,
  `bundle_id`/`bundle_step` set). Subject to `--apply` unless the step
  carries `apply: false`.
* `run:` — read-only command. Not audited. Useful for diagnostic
  collection between destructive steps.
* `watch:` — block-until-condition. Mirrors `inspect watch` flags as
  YAML keys (`until_cmd`, `until_log`, `until_sql`, `until_http`,
  `equals`, `matches`, `gt`, `lt`, `changes`, `stable_for`, `regex`,
  `psql_opts`, `match`, `interval`, `timeout`).

Exactly one of the three must be set per step.

## Failure routing (`on_failure:`)

| value                 | behavior                                                         |
| --------------------- | ---------------------------------------------------------------- |
| `abort` (default)     | Stop. No rollback. Bundle exits 2.                               |
| `continue`            | Log and proceed to the next step.                                |
| `rollback`            | Run reverse rollback for every completed reversible step.        |
| `rollback_to: <id>`   | Roll back from the failed step BACK TO (not including) `<id>`.   |

The bundle-level `rollback:` block runs only on `on_failure: rollback`,
in reverse declaration order. `rollback_to:` skips it on purpose —
the operator is asking for a partial unwind, not a full one.

## Audit correlation

Every audited step (and every rollback action) writes one entry with:

```
verb: exec | bundle.rollback | bundle.watch
bundle_id: b<ts>-<rand>      # one per `bundle apply` invocation
bundle_step: <step-id>       # matches the YAML
```

Filter to a single run with:

```bash
inspect audit ls --bundle b1737000123456-abcd
```

## Exit codes

* `0` — success (preflight + all steps + postflight).
* `2` — preflight failed, a step failed (no rollback or rollback ran),
  postflight failed, `--apply` missing on a destructive bundle, or
  validation error.

## When to reach for `bundle` (vs alternatives)

* For one-off ad-hoc commands across hosts: `inspect run`.
* For one destructive command with audit: `inspect exec`.
* For a sequence with strong ordering, conditional rollback, and
  human-readable plan output: **`inspect bundle`**.
* For periodic cron-style ops: shell scripts that pipe `inspect run`
  output into your scheduler. A bundle is overkill unless you need
  rollback semantics.

## See also

* `inspect help safety` — audit, reasons, snapshots.
* `inspect help write` — `--apply` contract on individual verbs.
* `inspect help selectors` — addressing targets.
* `inspect help fleet` — for cases where you want N hosts × 1 verb
  rather than 1 host × N steps.
