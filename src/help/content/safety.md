SAFETY — Audit log, snapshots, revert

EXAMPLES
  $ inspect audit ls                                  # list all mutations
  $ inspect audit ls --limit 20                       # recent mutations
  $ inspect audit show <id>                           # one entry with diff summary
  $ inspect audit grep "atlas"                        # search audit entries
  $ inspect audit gc --keep 90d --dry-run             # preview retention pass
  $ inspect audit gc --keep 100                       # keep newest 100 per ns
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

RETENTION + ORPHAN-SNAPSHOT GC (L5, v0.1.3)
  `inspect audit gc --keep <X>` deletes audit entries older than
  the retention threshold and sweeps orphan snapshot files.
  `<X>` accepts duration suffixes (`90d` / `4w` / `12h` / `15m`)
  or a bare integer for newest-N-per-namespace. Namespace is
  derived from the entry's `selector` field (`arte/foo` → `arte`);
  selector-less entries group under the sentinel `_`.
  `--keep 0` is rejected.

  `--dry-run` previews counts and freed bytes without modifying
  anything. `--json` emits a top-level envelope with `dry_run`,
  `policy`, `entries_total`, `entries_kept`, `deleted_entries`,
  `deleted_snapshots`, `freed_bytes`, `deleted_ids`, and
  `deleted_snapshot_hashes`.

  THE PINNED-SNAPSHOT INVARIANT — A snapshot file under
  ~/.inspect/audit/snapshots/sha256-<hex> is **never** deleted
  while any retained audit entry references it via
  `previous_hash`, `new_hash`, the `snapshot` filename, or a
  `revert.payload` (state_snapshot kind, including nested entries
  inside an F17 composite-revert JSON array). That is the F11
  revert contract; the GC enforces it as the only invariant the
  config cannot relax.

  AUTOMATIC GC. Set `[audit] retention = "<X>"` in
  ~/.inspect/config.toml to opt in to lazy GC on every audit
  append. The trigger is gated by a once-per-minute cheap-path
  marker (~/.inspect/audit/.gc-checked); within the cheap path
  only the oldest JSONL file's mtime is probed against the
  cutoff, so a busy session does not pay an FS scan per audit
  entry. Errors from the lazy path are swallowed so a transient
  GC failure cannot break the just-appended audit record.

  ~/.inspect/config.toml is a fresh global-policy file shipped by
  L5, distinct from per-namespace `servers.toml`. A missing file
  is not an error — the lazy GC stays off until you opt in.

LIMITATIONS
  The audit log is forensic, not tamper-proof. A user with file
  access can edit or delete entries. For tamper-proof audit trails
  in regulated environments, forward audit entries to an external
  log system.

SEE ALSO
  inspect help write         write verbs and the safety contract
  inspect help recipes       mutating recipes are also audited
