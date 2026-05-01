SAFETY — Audit log, snapshots, revert

EXAMPLES
  $ inspect audit ls                                  # list all mutations
  $ inspect audit ls --limit 20                       # recent mutations
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

OUTPUT REDACTION (L7, v0.1.3)
  Every line emitted by `run`, `exec`, `logs`, `cat`, `grep`,
  `find`, `search`, `why`, and the merged follow stream runs
  through a four-masker pipeline before reaching local stdout (or
  a JSON envelope's `line` field). The pipeline runs in fixed
  order: PEM private-key blocks collapse to a single
  `[REDACTED PEM KEY]` marker; `Authorization` / `Cookie` /
  `X-API-Key` / `Set-Cookie` header values become `<redacted>`;
  `scheme://user:pass@host` URL credentials mask the password to
  `user:****@host`; secret-shaped `KEY=VALUE` env pairs become
  `head4****tail2`. Inside an active PEM block the other three
  maskers do not fire on suppressed lines.

  --show-secrets bypasses ALL FOUR maskers (single flag, single
  bypass). Audit JSONL records which maskers fired for a given
  step in `secrets_masked_kinds` (canonical order
  pem/header/url/env) so post-hoc reviewers can tell two redacted
  runs apart by which pattern almost leaked. The text-side audit
  args also stamps `[secrets_masked=true]` when any masker fired.

  See `inspect help write` for the per-masker pattern table.

LIMITATIONS
  The audit log is forensic, not tamper-proof. A user with file
  access can edit or delete entries. For tamper-proof audit trails
  in regulated environments, forward audit entries to an external
  log system.

SEE ALSO
  inspect help write         write verbs and the safety contract
  inspect help recipes       mutating recipes are also audited
