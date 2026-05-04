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

REDACTION LIMITS (known boundaries)
  The L7 pipeline is line-oriented and pattern-based. There are
  three classes of input it does NOT mask, by design — each is
  documented here so operators can run with eyes open.

  1. POST-PROCESSED SECRETS (G6).
     `echo "$API_KEY" | base64`, `… | xxd`, `… | gzip | base64`,
     and similar transforms re-encode the secret into bytes the
     pattern maskers do not recognize. JSON-encoding a structured
     secret (e.g. a service-account key) produces the same blind
     spot: the encoded form is not a `KEY=VALUE`, not a header,
     not a URL credential, and not a PEM block, so every masker
     skips it.
     Operator discipline: when a verb's command intentionally
     re-encodes secret material, run with `--show-secrets` so the
     audit log records the exposure explicitly (per the
     `[secrets_exposed=true]` contract), and do NOT redirect that
     output into a log file or shared transcript.

  2. MULTI-LINE NON-PEM SECRETS (G8).
     The env masker matches `KEY=VALUE` on a single line. A shell
     here-doc style:
         export TOKEN="line1
         line2line3"
     masks line 1 only; lines 2..N pass through verbatim.
     PEM private keys are exempt: the dedicated PEM masker
     collapses the entire `-----BEGIN ... -----END` block to a
     single `[REDACTED PEM KEY]` marker regardless of line count.
     For non-PEM multi-line values (multi-line API tokens,
     line-continuation-assembled JSON keys), prefer single-line
     shell variables, or run with `--show-secrets` and treat the
     verb output as sensitive.

  3. ARBITRARY OPAQUE BLOBS.
     A high-entropy random string with no `KEY=` prefix, no header
     framing, and no URL framing has no signal for the maskers to
     latch onto. This is the same boundary GitHub Actions and
     other line-masking systems hit: pattern-based redaction
     cannot distinguish a deliberate identifier from a credential
     when both share the same alphabet.

  These limits apply to stdout/stderr, JSON `line` fields, audit
  `args` / `rendered_cmd` / `revert.preview`, and L7 transcripts
  (F18) uniformly — there is exactly one redaction code path
  shared across all four surfaces.

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

THREAT MODEL — OPERATOR AUTHORITY PASS-THROUGH
  inspect is an operator tool, not a sandbox. Every command it
  dispatches runs with the operator's full authority on the
  target — same SSH key, same docker socket, same kubectl
  credentials. inspect adds three guard rails over a raw shell
  (audit log, dry-run-by-default `--apply` gate, L7 redaction);
  it does NOT add a privilege boundary. Several consequences
  follow that are documented here so operators run with eyes
  open.

  1. PATH ARGUMENTS ARE NOT VALIDATED.
     `inspect put foo.cfg arte/svc:/../../../etc/shadow --apply`
     is honored — the path is whatever the remote shell
     interprets it as. inspect does not reject `..` in paths
     because doing so would block legitimate writes to
     well-known absolute paths. The threat surface is identical
     to running `scp` or `docker cp` directly: an operator who
     can dispatch can already write anywhere their target
     credentials allow.

  2. BUNDLE YAML IS TRUSTED CODE.
     Bundles are operator-authored YAML; their `command:` strings
     are passed verbatim to the remote shell. `{{ matrix.<k> }}`
     and `{{ vars.<k> }}` interpolations substitute the value
     literally — no shell-escape, no quoting. A matrix entry like
     `volume: "$(rm -rf /)"` will execute as a subshell on the
     target. Treat a bundle the same as any executable script:
     read it before running, never run a bundle from an
     untrusted source.

  3. ROLLBACK CAN BE DESTRUCTIVE.
     A bundle's `rollback:` block runs with the same authority
     as the forward block. A malicious bundle can intentionally
     corrupt data on rollback; a buggy rollback can corrupt
     data on a legitimate failure. Bundle authors are
     responsible for making rollback idempotent and bounded.
     `inspect bundle … --no-rollback` opts out of rollback at
     dispatch time when the operator wants to inspect the
     half-applied state manually.

  4. LOCAL EXECUTOR (F19) HAS NO SSH GATE.
     `type = "local"` namespaces dispatch directly under the
     operator's UID — there is no SSH key, no passphrase prompt,
     no ControlMaster lifecycle. The `--apply` gate, audit
     logging, and snapshot capture still fire identically; the
     authority boundary is the operator's local shell. Use
     local executors for the operator's own machine; do not
     install inspect under a service account whose process
     should not be running operator-authored shell.

  5. CONFIG IS A CREDENTIAL STORE.
     ~/.inspect/ contains the audit log (every command run
     against every server), session transcripts (full output
     of those commands, post-redaction), the snapshot store
     (pre-mutation file contents), and the profile cache
     (resolved namespace metadata). Protect this directory
     with the same care as ~/.ssh/. The directory is mode 0700
     and every file inside is mode 0600; the profile lock-file
     contention check is deliberately strict to surface
     unexpected concurrent access.

SESSION TRANSCRIPTS (F18, v0.1.3)
  Per-namespace, per-day, human-readable transcripts at
  ~/.inspect/history/<ns>-<YYYY-MM-DD>.log (mode 0600). Each verb
  invocation against a namespace produces one fenced block:

    ── 2026-04-28T14:32:11Z arte #b8e3a1 ──────────────────────
    $ inspect run arte -- 'docker ps'
    arte | atlas-vault
    arte | atlas-pg
    ── exit=0 duration=423ms audit_id=01HXR9Q5YQK2 ──

  The `── … ──` fence is awk-extractable; the trailing
  audit_id= cross-links back to the structured audit entry.

  Verbs that don't resolve a namespace (`inspect help`, `inspect
  audit ls`, `inspect history ...` itself) produce no transcript
  — they would always be near-empty.

  Management verbs:
    inspect history show [<ns>] [--date|--grep|--audit-id]
    inspect history list [<ns>]
    inspect history clear <ns> --before YYYY-MM-DD --yes
    inspect history rotate

  Retention via [history] in ~/.inspect/config.toml:
    retain_days = 90              # delete older files on rotate
    max_total_mb = 500            # cap across all namespaces
    compress_after_days = 7       # gzip older files

  Per-namespace overrides in ~/.inspect/servers.toml:
    [namespaces.<ns>.history]
    disabled = true               # skip transcript for this ns
    redact = "off" | "normal" | "strict"

  Redaction: every line tee'd to the transcript runs through the
  L7 four-masker pipeline before being appended (PEM / Authorization
  / URL credentials / KEY=VALUE). `redact = "off"` writes raw
  lines (file mode 0600 already restricts exposure). `--show-secrets`
  on the originating verb bypasses both stdout and transcript
  redaction.

  Performance: one fdatasync(2) per verb invocation regardless of
  output volume. Buffer capped at 16 MiB; overflow becomes a
  `[transcript truncated]` marker.

SEE ALSO
  inspect help write         write verbs and the safety contract
  inspect help recipes       mutating recipes are also audited
  inspect history --help     transcript management surface
