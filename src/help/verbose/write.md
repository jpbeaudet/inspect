
VERBOSE — Write-verb edge cases

LARGE-FANOUT GUARD
  Mutating verbs that resolve to ≥ 25 targets are aborted before
  the first packet leaves the local host:

    error: large-fanout aborted: matched 64 targets, threshold is 25
    see: inspect help write

  Override:
    --large-fanout            run anyway, still requires --apply
    INSPECT_LARGE_FANOUT=1    ambient override (CI / scripted flows)

  The threshold is intentionally low. Most write operations are
  surgical; a 64-target match is almost always a selector mistake.

EXEC-WITHOUT-ALLOW-EXEC
  `inspect exec …` requires --allow-exec on every invocation. There
  is no env-var bypass and no per-namespace allowlist — the gate is
  by-design noisy. Pair with --apply for actual execution; without
  --apply, exec dry-runs print the resolved command on every
  target.

  Recipes that need exec must include `--allow-exec` per step in
  their YAML; the recipe loader does not silently inject it.

CP REMOTE-TO-REMOTE
  Direct remote → remote copy is not supported. inspect refuses:

    error: cp: remote-to-remote copy is not supported
    see: inspect help write

  Workaround: stream through stdout
    $ inspect cat arte/atlas:/etc/x | inspect cp - prod/atlas:/etc/x

EDIT AND CONCURRENT WRITES
  `inspect edit` snapshots the remote file, opens $EDITOR locally,
  then re-fetches on save and aborts if the remote changed in the
  interim. Override with --force to overwrite (adds a `forced=true`
  audit field).

DRY-RUN CONTRACT
  Every mutating verb is dry-run by default. The first non-zero
  exit code from a dry-run means the change *would* fail, not that
  the system was modified. To see what would run without any
  network roundtrip, add --plan-only.

SEE ALSO
  inspect help write         the standard topic body
  inspect help safety        audit log + revert semantics
  inspect help fleet         multi-server write contract
