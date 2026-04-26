SAFETY — Audit log, snapshots, revert

EXAMPLES
  $ inspect audit ls                                  # list all mutations
  $ inspect audit ls --since today                    # today's mutations
  $ inspect audit show <id>                           # one entry with diff summary
  $ inspect audit grep "atlas"                        # search audit entries
  $ inspect revert <audit-id>                         # preview revert (dry-run)
  $ inspect revert <audit-id> --apply                 # restore original content

AUDIT LOG
  Location:  ~/.inspect/audit/<YYYY-MM>-<user>.jsonl
  Mode:      600 (user-only)
  Format:    One JSON object per line, append-only

  Fields: ts, user, host, verb, selector, args, diff_summary,
          previous_hash, new_hash, snapshot path, exit, duration_ms.

SNAPSHOTS
  Location:  ~/.inspect/audit/snapshots/<hash>
  Content:   Original file content before mutation
  Keyed by:  SHA-256 (deduplicated across edits of the same content)

  Snapshots are what make revert possible. Without the original
  content a hash alone cannot undo a change.

REVERT
  inspect revert <audit-id> restores the file at the recorded
  selector to the snapshot content. It follows the same safety
  contract as any write:
    - Dry-run by default (shows reverse diff)
    - --apply to execute
    - Audit-logged as a revert
    - If current content does not match new_hash, warns and
      requires --force.

LIMITATIONS
  The audit log is forensic, not tamper-proof. A user with file
  access can edit or delete entries. For tamper-proof audit trails
  in regulated environments, forward audit entries to an external
  log system.

SEE ALSO
  inspect help write         write verbs and the safety contract
  inspect help recipes       mutating recipes are also audited
