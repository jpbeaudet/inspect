
VERBOSE — Safety edge cases

HASH-MISMATCH ON REVERT
  `inspect revert <id>` compares the current remote content's hash
  to the audit entry's `new_hash`. Mismatch means the file changed
  after the recorded mutation — perhaps by another tool or another
  inspect call. The default behaviour is to refuse and emit:

    error: revert hash mismatch: file changed since the recorded mutation
    see: inspect help safety

  Override carefully:
    --force          revert anyway, audit-logged with forced=true
    --merge          show a 3-way diff and exit (no write)

  Forcing a revert overwrites whatever the file currently contains
  with the snapshot. Always pair --force with --apply only after
  reading the merge output.

SNAPSHOT GARBAGE COLLECTION
  Snapshots accumulate under ~/.inspect/audit/snapshots/<hash>.
  inspect never auto-prunes them — every snapshot is the only thing
  standing between a botched edit and an unrecoverable change.

  Manual prune (rarely needed):
    $ inspect audit gc --older-than 90d --apply

  GC is itself a mutating verb: dry-run by default, audit-logged.

AUDIT LOG ROTATION
  Audit files are <YYYY-MM>-<user>.jsonl, mode 600, append-only
  from inspect's perspective. inspect does not rotate or compress
  them. For long-lived workstations, configure logrotate against
  ~/.inspect/audit/*.jsonl with a `copytruncate` policy.

FORENSIC, NOT TAMPER-PROOF
  A user with write access to ~/.inspect/audit can edit history.
  For regulated environments forward audit entries to an external
  sink in real time:

    $ tail -F ~/.inspect/audit/*.jsonl | your-shipping-agent

  inspect itself emits no syslog / network audit channel — the
  forwarding side is intentionally external so the binary stays
  network-policy-clean.

REVERT OF A REVERT
  Reverts are themselves audit entries. To undo a revert, revert the
  revert: `inspect revert <revert-id>`. Each step is logged and
  hash-checked.

SEE ALSO
  inspect help safety        the standard topic body
  inspect help write         dry-run / --apply contract
  inspect help recipes       mutating recipes are audited
