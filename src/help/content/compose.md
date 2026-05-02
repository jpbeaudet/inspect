COMPOSE — first-class verbs over Docker Compose projects (F6, v0.1.3)

Replaces the v0.1.2-era pattern where the operator dropped back to
`inspect run <ns> -- 'cd <project_dir> && sudo docker compose …'`
to inspect or mutate compose state. Every sub-verb resolves the
project's working directory from the cached profile (populated at
`inspect setup` time via `docker compose ls --format json`), so the
operator never types the path.

READ SUB-VERBS

  ls       List compose projects on the namespace.
  ps       Per-service status table for one project.
  config   Effective merged compose config (redacted).
  logs     Aggregated logs for a project, or one service inside it.

WRITE SUB-VERBS (audited; require --apply)

  up       Bring up a project. verb=compose.up.
  down     Tear down a project. verb=compose.down. --volumes is destructive.
  pull     Pull images for a project. verb=compose.pull. Streams progress.
  build    Build images for a project. verb=compose.build. Streams progress.
  restart  Restart a single service. verb=compose.restart.

EXEC (inspect-run-style; not audited)

  exec     Run a command inside a compose service container. Mirrors
           `inspect run`'s contract: no apply gate, no audit, output
           runs through the L7 redaction pipeline. Use the audited
           write verbs for state mutations; use exec for inspection
           and fast iteration inside a running service container.

SELECTORS

  <ns>                       — for `compose ls`
  <ns>/<project>             — for `compose ps`, `compose config`,
                               aggregated `compose logs`,
                               `compose restart --all`,
                               `compose up`, `compose down`, and
                               project-wide `compose pull` /
                               `compose build`
  <ns>/<project>/<service>   — for narrowed `compose logs`,
                               per-service `compose pull` /
                               `compose build`, the safe
                               `compose restart`, and `compose exec`

  The existing `<ns>/<service>` form continues to work for the
  generic read/write verbs (`inspect logs`, `inspect restart`)
  because F5's resolver tries the compose service label first.

JSON SCHEMAS (--json)

  ls:      data.compose_projects = [
             {namespace, name, status, working_dir, compose_file,
              service_count, running_count}, ...]

  ps:      data.services = [
             {service, state, ports, image, uptime}, ...]

  config:  data.config = the rendered config text after the
           redaction pipeline; data.secrets_masked = bool.

  logs:    streamed line-by-line to stdout (no envelope).

  up/down/pull/build/restart:
           data = {namespace, project, audit_id, exit, duration_ms,
                   compose_file_hash, ...verb-specific fields}.
           Per-verb audit entry uses verb=compose.<sub> with
           args="[project=<p>] [compose_file_hash=<sha-12>] [...]".

  exec:    streamed line-by-line to stdout (no envelope, mirrors
           `inspect run`).

AUDIT TAGS

  Every audited compose write stamps these bracketed tags into
  the audit entry's `args` field so `inspect audit grep` works:

    [project=<name>]                always
    [service=<name>]                pull/build (when service-scoped) +
                                    restart (per-iteration)
    [compose_file_hash=<sha-12>]    when the compose file was readable
    [no_detach=true]                up + when --no-detach was passed
    [force_recreate=true]           up + when --force-recreate was passed
    [volumes=true]                  down + when --volumes was passed
    [rmi=local]                     down + when --rmi was passed
    [ignore_pull_failures=true]     pull + when flag was passed
    [no_cache=true]                 build + when --no-cache was passed
    [pull=true]                     build + when --pull was passed

  A post-mortem can verify the project's compose file did not
  change between the audit and a re-run by re-hashing the file
  on the host and comparing 12-hex prefixes.

REVERT KIND

  All five compose write verbs record `revert.kind = unsupported`.
  Compose state mutations have no clean inverse: `up` is countered
  by `down` only on paper (down can wipe volumes); `pull` and
  `build` change image cache state in non-reversible ways; `restart`
  is fundamentally idempotent. `inspect revert <id>` on these
  entries surfaces the chained hint with the exact "what to run if
  you want to roll back" command rather than silently no-opping.

EXIT CODES

  0   ok
  1   no matching compose project / service
  2   usage error (missing service portion without --all,
                   malformed selector, missing -- cmd on exec, ...)

EXAMPLES

  $ inspect compose ls arte
  $ inspect compose ps arte/luminary-onyx
  $ inspect compose config arte/luminary-onyx --json
  $ inspect compose logs arte/luminary-onyx --tail 200
  $ inspect compose logs arte/luminary-onyx/onyx-vault --follow
  $ inspect compose restart arte/luminary-onyx/onyx-vault --apply
  $ inspect compose up arte/luminary-onyx --apply
  $ inspect compose down arte/luminary-onyx --apply --yes
  $ inspect compose down arte/luminary-onyx --volumes --apply --yes-all
  $ inspect compose pull arte/luminary-onyx --apply
  $ inspect compose pull arte/luminary-onyx/onyx-vault --apply
  $ inspect compose build arte/luminary-onyx --no-cache --apply
  $ inspect compose exec arte/luminary-onyx/onyx-vault -- ps -ef
  $ inspect compose exec arte/luminary-onyx/onyx-vault -u root -- df -h

DISCOVERY + STATUS INTEGRATION

  Compose project discovery runs once at `inspect setup` time and
  caches a `compose_projects: [...]` list on the namespace's
  profile. `inspect status <ns>` surfaces the count as a new
  `compose_projects: N` line in human output (omitted when zero)
  and an always-present `compose_projects` array in `--json`.
  Projects added or removed out-of-band become visible after the
  next `inspect setup`, or sooner via
  `inspect compose ls <ns> --refresh`.

SEE ALSO

  inspect help safety       audit log + revert semantics
  inspect help write        write-verb safety contract
  inspect help formats      --json envelope / per-verb schemas
