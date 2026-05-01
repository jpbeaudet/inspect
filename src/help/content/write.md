WRITE — Write verbs, dry-run/apply, safety contract

EXAMPLES
  $ inspect restart arte/pulse                            # dry-run (preview)
  $ inspect restart arte/pulse --apply                    # execute
  $ inspect edit arte/atlas:/etc/foo 's/old/new/'         # show diff (dry-run)
  $ inspect edit arte/atlas:/etc/foo 's/old/new/' --apply
  $ inspect cp ./fix.conf arte/pulse:/etc/pulse.conf
  $ inspect cp ./fix.conf arte/pulse:/etc/pulse.conf --apply

WRITE VERBS
  restart / stop / start / reload    container lifecycle
  cp <local> <sel>:<path>            push file (or pull: cp <sel>:<path> <local>)
  edit <sel>:<path> '<sed-expr>'     in-place content edit (atomic)
  rm / mkdir / touch                 file operations
  chmod / chown                      permission changes
  exec <sel> -- <cmd>                arbitrary command (audited; requires --apply)

RUN vs EXEC
  run    Read-only, NOT audited, no apply gate. Use for ad-hoc
         inspection: `inspect run arte/pulse -- ps auxww`.
  exec   Audited write verb. Same shape as run but every --apply
         lands in the audit log with a --reason. Use when the command
         mutates state OR when you need a forensic record.

SECRET MASKING (P4 + L7)
  By default, every line emitted by `run` and `exec` (and the read
  verbs `logs`, `cat`, `grep`, `find`, `search`, `why`) runs through
  a four-masker pipeline before reaching local stdout:
    1. PEM       (L7) -----BEGIN ... PRIVATE KEY----- through the
                      matching END line collapses to a single
                      `[REDACTED PEM KEY]` marker. Recognized:
                      PKCS#8, PKCS#1, RSA, EC, DSA, OPENSSH, PGP.
                      Public certificates and public keys pass
                      through unchanged.
    2. header    (L7) Case-insensitive `Authorization`, `X-API-Key`,
                      `Cookie`, `Set-Cookie` — value portion becomes
                      `<redacted>`. Catches `curl -v` traces and
                      reverse-proxy logs.
    3. URL       (L7) `scheme://user:pass@host` patterns mask the
                      password to `user:****@host`. Covers postgres,
                      mysql, redis, mongodb, amqp, http, https.
    4. env       (P4) `KEY=VALUE` lines with secret-shaped keys
                      (*_KEY/*_SECRET/*_TOKEN/*_PASSWORD/*_PASS,
                      DATABASE_URL, REDIS_URL, MONGO_URL, etc.) get
                      the value masked to `head4****tail2`.

  Inside an active PEM block the other three maskers do not fire —
  the entire block body is replaced with the single marker. The
  audit JSONL records which masker(s) fired in
  `secrets_masked_kinds` (canonical order pem/header/url/env).

    --show-secrets    bypass ALL FOUR maskers (audit args is stamped
                      [secrets_exposed=true] on `exec`).
    --redact-all      mask every KEY=VALUE pair (env masker only),
                      not just recognized keys.

INNER EXIT CODE (P11)
  `run` and `exec` propagate the remote command's exit code to your
  shell. `inspect run arte/pulse -- 'exit 7'` returns 7. Mixed exits
  across multiple targets fall back to 2.

SAFETY CONTRACT
  1. DRY-RUN BY DEFAULT    No mutation without --apply. Ever.
  2. DIFF FOR EDITS        edit and cp show a unified diff first.
  3. AUDIT LOG             Every --apply is recorded under
                           ~/.inspect/audit/.
  4. SNAPSHOTS             Original content saved before mutation,
                           keyed by SHA-256.
  5. CONFIRMATION          rm/chmod/chown prompt interactively even
                           with --apply. Skip with --yes.
  6. ATOMIC WRITES         edit writes a temp file then renames.
  7. LARGE-FANOUT GUARD    >10 targets prompts even with --apply.
                           Skip with --yes-all.

REVERT
  $ inspect audit ls --limit 20
  $ inspect revert <audit-id>                             # dry-run (reverse diff)
  $ inspect revert <audit-id> --apply                     # restore original

  If the file changed since your edit (hash mismatch), revert warns
  and requires --force.

SEE ALSO
  inspect help safety        audit log details
  inspect help fleet         write verbs across multiple servers
  inspect help examples      search-then-transform workflows
  inspect help recipes       multi-step mutating runbooks
