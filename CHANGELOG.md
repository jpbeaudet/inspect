# Changelog

All notable changes to `inspect` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.3] — 2026-05-04

Closes the v0.1.3 patch backlog (`INSPECT_v0.1.3_BACKLOG.md`): 30 of
30 in-scope items shipped. Field feedback from four independent
v0.1.2 users plus a multi-hour destructive migration session by the
primary operator. **L1 (TUI mode)** was authorized-deferred to v0.5\*
mid-release once the maintainer confirmed the user base is
LLM-driven and a `ratatui` dashboard offers no value to agentic
callers — see `CLAUDE.md` "Authorized deferrals" registry for the
full rationale. This is the one explicit deferral out of v0.1.3;
every other backlog item shipped.

### Fixed (pre-release security audit)

A second pre-tag pass — this time a defensive sweep against a
checklist of real-world CVEs and breach patterns
(`INSPECT_v013_SECURITY_AUDIT.md`) — surfaced three additional
hardening fixes (S1, S2, S4). All three close in-place before
tagging.

- **S1 — Implicit SSH agent / X11 forwarding.** Operator
  `~/.ssh/config` directives (`ForwardAgent yes`, `ForwardX11
  yes`) silently inherited into every inspect-spawned ssh
  invocation, exposing the local agent socket and X11 cookie to
  every namespace inspect dispatched against. inspect has zero
  use cases that require either forward (no agent-on-target,
  no remote X clients), so `base_args()` in `src/ssh/options.rs`
  now emits `-o ForwardAgent=no -o ForwardX11=no` unconditionally
  on every dispatch, suppressing the personal-config inheritance.
  Defends against the OpenSSH agent-forwarding lateral-movement
  patterns (CVE-2023-38408 family).
- **S2 — `/proc/<pid>/environ` secrets bypassed the env masker.**
  The L7 redactor's env-secret rule masks `KEY=VALUE` pairs
  line-by-line, but `/proc/*/environ` separates entries with NUL
  bytes rather than newlines, so the entire blob arrived as one
  unrecognizable line and slipped through verbatim into audit /
  transcript output. `redact_for_audit` and the streaming line
  iterator in `src/redact/mod.rs` now detect embedded NULs and
  recursively mask each NUL-separated segment, so a curious
  `cat /proc/$pid/environ` from a shell session still has its
  secret env vars masked. Defends against agent-context-window
  leak via process-environment exfil (§2.1 of audit).
- **S4 — Username / hostname shell-metachar injection.**
  `servers.toml` accepted any UTF-8 string for `user` and `host`,
  so a malicious or fat-fingered config like
  `user = "alice;rm -rf ~"` could be expanded by `Match exec %u`
  / `%h` directives in operator `~/.ssh/config` (CVE-2026-35386
  family). `src/config/namespace.rs` now validates `user` against
  the POSIX login-name grammar
  (`[A-Za-z_][A-Za-z0-9_.-]{0,31}`) and `host` against
  RFC-952/1123 hostname or IP-literal grammar, with the new
  typed errors `InvalidUser` / `InvalidHost` surfaced via
  `inspect help safety` and `inspect help ssh`. Validation runs
  at config-load before any ssh argv is constructed.
- **S5 — URL credential masker leaked the suffix of passwords
  containing `@`.** The L7 URL-credential redactor anchored on the
  *first* `@` after the password, so connection strings like
  `postgres://admin:p@ssw0rd!@db.internal/app` were rewritten to
  `postgres://admin:****@ssw0rd!@db.internal/app` — masking only
  `p` and leaking `ssw0rd!` into audit / transcript output. The
  regex in `src/redact/url.rs` is now greedy on the password and
  captures the host explicitly (Rust `regex` has no look-around,
  so we rewrite `$1:****@$2` instead of using a host lookahead).
  Backtracking finds the rightmost `@` that is followed by a
  host-shaped token; embedded `@`s in the password are consumed
  as data. Three new unit tests cover the §5.4 case, three-`@`
  pathological input, and pass-through for non-URL `@` patterns.
- **S6/S7 — Threat model documented in `inspect help safety`.**
  The audit pass surfaced four areas where inspect explicitly does
  not add a privilege boundary, but where the boundary's absence
  was not previously documented for operators: (1) path arguments
  to `inspect put` / `inspect cp` are not validated against `..`
  traversal — the threat surface is identical to running `scp`
  directly; (2) bundle YAML is trusted code and matrix
  interpolation is literal substitution, not shell-escape — a
  matrix entry like `volume: "$(rm -rf /)"` will execute as a
  subshell on the target; (3) bundle rollback runs with the same
  authority as the forward block — bundle authors are responsible
  for idempotent / bounded rollback; (4) local executor (F19)
  has no SSH gate — namespaces with `type = "local"` dispatch
  directly under the operator's UID, so the `--apply` gate +
  audit log are the only safety boundary. New "THREAT MODEL —
  OPERATOR AUTHORITY PASS-THROUGH" section under `inspect help
  safety` makes all four explicit. No code change — the
  audit also positively verified that audit / transcript / home
  / snapshots / sockets / scripts / cursors / profile-cache all
  enforce 0700 / 0600 (§6.3) and that password auth (L4) never
  passes the secret on the cmdline (`SSH_ASKPASS` mechanism with
  3-attempt cap; §10.1 / §10.2 / §10.3).

### Fixed (pre-release pain-point audit)

A structured pain-point audit run between code-freeze and tag
surfaced nine gaps (G1–G9). The four release-blocking or
correctness items were closed in-place before tagging; the
remaining three are documented limitations called out under
`inspect help safety` and `inspect help watch`.

- **G2 — Embedded secrets leaked into audit `args` / `rendered_cmd`
  / `revert.preview`.** The L7 stream redactor only masked
  stdout/stderr. An operator running
  `inspect exec arte -- 'psql -h db -U u -W s3cret …' --apply
  --no-revert` recorded `s3cret` verbatim in the JSONL audit entry's
  `args`, `rendered_cmd`, and `revert.preview` ("Original cmd: …")
  fields. New `crate::redact::redact_for_audit(text)` helper runs
  the line-oriented header / URL-credential / inline-env-secret
  maskers across single-line command strings; wired through
  `stamp_args` and every `rendered_cmd = Some(...)` audit site
  across `verbs/run.rs`, `verbs/write/{exec,edit}.rs`,
  `verbs/steps.rs`, `verbs/watch.rs`, `verbs/compose/{down,build,up,
  pull}.rs`, `bundle/exec.rs`, and `commands/revert.rs`.
  `--show-secrets` opts out (records verbatim, stamps
  `[secrets_exposed=true]` per the existing contract). Five
  acceptance tests in `tests/phase_f_v013.rs` (`g2_exec_*`).
- **G4 — `ControlMaster=auto` in operator's `~/.ssh/config` could
  promote inspect-spawned ssh into a backgrounded master.** When
  inspect dispatches without attaching to its own ControlPath
  socket (no master alive, or the operator disabled
  ControlMaster lifecycle), a personal `ControlMaster auto` in
  ssh_config caused the short-lived dispatch to self-promote into
  a backgrounded master that detached stdio and outlived the
  parent. Fixed by adding explicit `-o ControlMaster=no` on the
  direct-ssh fallthrough at all three call sites in
  `src/ssh/exec.rs` (`run_remote`, `run_remote_streaming`, and the
  streaming-capturing dispatch).
- **G5 — `ControlPath` socket length silently exceeded `sun_path`.**
  On macOS (`sun_path` = 104 bytes) and even on Linux (108 bytes),
  a long namespace + long `INSPECT_SOCKETS_DIR` could push the
  ControlPath beyond what `bind(2)` accepts. The kernel truncated
  the path silently; `ssh -M` then either failed with an opaque
  "Control socket … connect" error or, worse, two namespaces
  collided onto the same truncated path. New
  `validate_socket_path` in `src/ssh/master.rs` checks the final
  socket path length up front (104-byte conservative limit covering
  Darwin) and emits a typed `ControlSocketPathTooLong` error
  pointing the operator at `INSPECT_SOCKETS_DIR=/tmp/i` or a shorter
  namespace. Two unit tests cover the boundary.
- **G9 — Atomic `cp` write didn't set `noclobber`.** The
  `build_stream_atomic_script` helper used by `inspect cp --to-remote`
  /  `--from-remote` ran `cat > $tmp` under `set -e` only. If `$tmp`
  resolved to an existing symlink the redirect would clobber the
  link target; if a hostile actor pre-created `$tmp` between the
  `mktemp` and the redirect, the script would overwrite it. Adding
  `set -C` (noclobber) to the prelude makes the redirect use
  `O_EXCL` semantics — refuses pre-existing files and refuses to
  follow symlinks on the create path. One unit test asserts the
  `set -eC` prefix.

### Fixed — release-smoke LLM-trap

- **Key-auth retries now fail-fast locally (~10ms) instead of
  burning an SSH handshake (~1-3s) per wrong attempt.** Same
  release-smoke turn that fixed the 1-vs-3 retry asymmetry: the
  three retry attempts felt sluggish on `inspect connect arte`
  vs `ssh-add ~/.ssh/id_ed25519`, even though both are doing
  "is this passphrase right for this key?" The cause: ssh-add
  decrypts the key file locally (no network); inspect was
  spawning a full `ssh -fN user@host` master per attempt, which
  has to TCP-connect, KEX, attempt auth, and only THEN reject
  the wrong passphrase — ~1-3s on internet RTT, plus 2-3
  internal askpass retries inside ssh's `load_identity_file`
  with the same wrong value before giving up.

  **Fix.** Each wrong-passphrase retry on the key-auth path
  now goes through `validate_key_passphrase_locally` first —
  a local `ssh-keygen -y -f <key_path>` invocation fed via
  the same askpass mechanism. ssh-keygen returns a non-zero
  exit when the passphrase doesn't decrypt the key, in
  milliseconds, no network. Only the correct-passphrase
  attempt proceeds to spawn the full ssh master (which still
  needs the network handshake for the actual session start —
  unavoidable). Matches the speed of ssh-add's encrypted-key
  retries.

  Password auth gets `pre_validate_locally: false` because the
  password is the remote sshd's secret, not a local artifact —
  there's nothing to pre-validate against. Password retries
  still cost one handshake each, which is the inherent cost of
  remote-secret verification.

  - `InteractiveAuthConfig` gains a `pre_validate_locally: bool`
    field. `key_passphrase()` constructor sets it `true`,
    `password()` sets it `false`. The retry loop in
    `run_interactive_master_with_retries` runs the local
    pre-flight when configured + `target.key_path` is `Some`,
    falling through to the network attempt only on local
    success (or when no key_path is configured — defensive
    fallback that should be unreachable for key auth since the
    resolver enforces `key_path`).
  - `validate_key_passphrase_locally(key_path, askpass)`:
    spawns `ssh-keygen -y -f <key_path>` with `stdin=null`,
    `stdout=null`, `stderr=piped`, askpass env vars applied via
    the same `AskpassScript::env_vars()` we use for `ssh -fN`.
    On non-zero exit, surfaces the captured stderr in the
    error chain so the per-attempt warning has a meaningful
    root cause (`ssh-keygen rejected passphrase: ...`) rather
    than an ssh-side `Permission denied (publickey)` that
    arrived 2s later.
  - 3 new integration tests in `local_passphrase_validation_tests`
    cover right / wrong / missing-key paths against a real
    `ssh-keygen -t ed25519`-generated keypair in a tempdir.
    `right_passphrase_validates_in_milliseconds` pins a
    generous `<2s` ceiling that still catches a regression
    where someone accidentally re-introduces a network call.
    Tests are gated on ssh-keygen-availability; the parity
    structural test
    `pre_validate_locally_only_set_for_key_auth` covers the
    static contract on environments without OpenSSH.
  - `ENV_INTERACTIVE_PASSPHRASE` mutation in tests is now
    serialized through a per-module `env_lock()` mutex
    (mirror of `cancel::tests::test_lock`) so parallel test
    runs do not race on the process-global env.
  - `src/help/content/ssh.md` CREDENTIAL RESOLUTION gains a
    paragraph explaining the local-pre-validation mechanism;
    `CLAUDE.md ### SSH ControlMaster reuse` gains the
    invariant.

  Field-validated against arte (encrypted ed25519): three
  retries on `inspect connect` show `warning: ssh passphrase
  attempt N/3 failed` lines that fire instantly after pressing
  enter rather than after a perceptible network pause.
  `time inspect connect arte` with 1 wrong + 1 right shows
  `user 0m0.563s` (CPU time across the full flow) — vs the
  ~10s wall clock that pure-handshake retries would burn on
  3 wrong attempts. All 28 test suites + 3 new tests green.

- **F13 auto-reauth aborted on a single wrong passphrase keystroke;
  password-auth had three retries — key-auth had zero.** Surfaced
  immediately after fixing the first-connect host-key trap, when
  the operator typed a deliberately wrong passphrase on the F13
  auto-reauth prompt and got `auto-reauth for 'arte' failed:
  reauth 'arte'` after one ssh-side `Permission denied (publickey)`
  — exit non-zero, verb aborted, operator forced to rerun. The
  password-auth path (`start_master_password`, lines 707-754
  pre-refactor) had a 3-attempt loop with attempt counter and
  per-miss warning. The key-auth path (`start_master`, line 416-453
  pre-refactor) had a single-shot prompt + propagation. F13
  auto-reauth (`reauth_namespace`) calls `start_master`, so it
  inherited the no-retry behavior — the F13 auto-reauth UX gap
  was strictly worse than the password-auth UX gap, which is the
  inverse of what an operator would expect for the recovery flow.

  **Fix.** Both interactive flavors now share
  `run_interactive_master_with_retries` in `src/ssh/master.rs`,
  parameterized by `InteractiveAuthConfig::key_passphrase()` /
  `::password()`. The shared helper enforces:

    - Same cap (`PASSPHRASE_MAX_ATTEMPTS = PASSWORD_MAX_ATTEMPTS = 3`).
    - Same prompt shape: `Enter <label> (namespace '<ns>',
      host <h>, attempt N/3):`.
    - Same per-miss warning: `warning: ssh <kind> attempt N/3
      failed` to stderr; loop continues.
    - Same final error on exhaustion: `ssh <kind> auth for '<ns>'
      failed after 3 attempt(s); aborting. hint: <flavor-specific>
      see: inspect help ssh\nlast error: <ssh stderr>`.
    - Empty input on any prompt aborts immediately without
      consuming an attempt slot (matches the pre-existing
      password-path behavior).
    - Same env-var lifecycle (`set_var` before askpass, `remove_var`
      after run, zeroize the local copy) and same keychain-save
      timing (`if result.is_ok() && save_to_keychain`, BEFORE
      zeroize) on both flavors.

  Per-flavor knobs that differ legitimately (env-var name —
  `INSPECT_INTERACTIVE_PASSPHRASE` vs `INSPECT_INTERACTIVE_PASSWORD`;
  ssh `-o` opts — `[]` vs `PASSWORD_AUTH_SSH_OPTS`; success hook —
  `None` vs `Some(maybe_warn_password_auth)`; chained recovery hint
  — `ssh-keygen -y -f <key_path>` vs "verify the password against
  the host directly") live in the `InteractiveAuthConfig`
  constructor for that flavor and nowhere else, so a future drift
  attempt has to physically edit one of the constructors.

  - 5 unit tests in `ssh::master::interactive_retry_parity_tests`
    pin the contract: `max_attempts` parity (and the literal
    `== 3`), distinct env-var names + auth modes, non-empty
    user-facing strings on every config, `inspect help ssh` in
    every final hint, and the password-only
    `PASSWORD_AUTH_SSH_OPTS` staying off the key path. New
    interactive-auth flavors (e.g. FIDO2 PIN, hardware-token PIN)
    MUST go through the same helper or these break.
  - `src/help/content/ssh.md` CREDENTIAL RESOLUTION reflows so
    both flavors document the retry contract and reference the
    `ssh-keygen -y -f` recovery; an "Empty input on any prompt
    aborts immediately" note is added so an agent reading the
    topic before driving the verb knows the keystroke contract.
  - `CLAUDE.md` `### SSH ControlMaster reuse` gains a new
    field-validated invariant capturing the parity contract +
    the test module that pins it.

  Field-validated end-to-end against arte (encrypted ed25519,
  fresh shell): three attempts on `inspect connect`, three
  attempts on F13 auto-reauth via `inspect run`, recovery
  succeeds on attempt 2/3 or 3/3, exhaustion shows the
  `ssh-keygen -y -f` chain. All 28 test suites + 5 new tests
  green.

- **🔴 First-connect to an unknown host hung in a tight askpass
  loop (CRITICAL).** Surfaced live during release-smoke when
  arte's entry was wiped from `~/.ssh/known_hosts` after a
  codespace restart. `target/release/inspect connect arte`
  prompted for the passphrase, the operator typed it, and then
  ssh hung silently. Verbose ssh (`-vvv`) showed
  `read_passphrase: requested to askpass` repeating 41+ times
  before the operator ^C'd. Diagnostic askpass (env-gated dump
  of what the askpass child sees) revealed the root cause: ssh
  was invoking askpass to answer the host-key confirmation
  prompt
  (`Are you sure you want to continue connecting (yes/no/
  [fingerprint])?`) — but inspect's askpass is a *passphrase*
  helper that returns the value of an env var, so it returned
  the passphrase string. ssh rejected that as "neither yes/no/
  fingerprint", reprompted with `Please type 'yes', 'no' or
  the fingerprint:`, askpass returned the same value, ssh
  reprompted — infinite loop. This had been masked through all
  of v0.1.3 because every smoke run had arte already in
  known_hosts; F13 yesterday "worked" only because the host
  was already trusted.
  Yesterday's F13 auto-reauth path on stale ControlMaster
  inherits the same trap (a fresh shell after a codespace
  restart often loses BOTH the master AND the known_hosts
  entry), so this regressed the entire reauth flow for any
  first-time / re-provisioned host.

  **Fix.** Every ssh-spawn site now sets
  `-o StrictHostKeyChecking=accept-new` (OpenSSH ≥ 7.6):
  unknown hosts are auto-added to `known_hosts` on first
  connect (operator sees a single
  `Warning: Permanently added '<host>' (ED25519) to the list
  of known hosts.` line on stderr), but a *changed* key still
  aborts with `Host key verification failed.` — caught by
  `ssh_precheck::classify` and routed to the existing
  `host_key_changed_hint` MITM warning.

  - `src/ssh/master.rs::build_master_command` (extracted from
    `run_master_with_opts` for testability) now appends
    `-o StrictHostKeyChecking=accept-new` to every master
    start. Both the BatchMode probe (step 3 of the auth
    ladder) and the interactive-prompt path inherit the flag.
  - `src/discovery/ssh_precheck.rs::build_precheck_command`
    (extracted from `run` for testability) does the same. The
    BatchMode-yes precheck against an unknown host previously
    misclassified as `HostKeyChanged` (the MITM warning
    surface) on first connect; under accept-new it succeeds
    silently, which matches the contract that "first-connect
    is normal, key-change is the security-sensitive event."
  - Dispatch sites (`run_remote`, `run_remote_streaming`,
    `run_remote_streaming_capturing`) ride the already-
    verified ControlMaster channel, so they inherit the
    trust decision and need no change.
  - `src/help/content/ssh.md` gains a `FIRST-CONNECT HOST KEY`
    section explaining the askpass-loop trap and the
    accept-new fix; the SECURITY section's "No auto-trust of
    unknown host keys" line was removed (it's no longer
    accurate; the security model is now "auto-trust on first,
    refuse on change", documented inline).
  - `docs/MANUAL.md` §3.3 gains a "First-time connect to an
    unknown host" subsection mirroring the help-topic prose.
  - 3 regression tests
    (`ssh::master::accept_new_tests::build_master_command_includes_accept_new`,
    `ssh::master::accept_new_tests::build_master_command_batch_probe_includes_accept_new`,
    `discovery::ssh_precheck::tests::precheck_command_includes_accept_new`)
    pin the flag presence at the command-builder layer so the
    trap can't silently regress.

  Field-validated end-to-end against arte (OVH, encrypted
  ed25519, fresh shell with no known_hosts entry) running
  `target/release/inspect connect arte` →
  `inspect disconnect arte` → reconnect → `pkill -9 -f
  ControlPath=...arte.sock` (to simulate codespace restart
  stale-master) → `inspect run arte 'echo reauth-works'`
  triggering F13 auto-reauth → all five steps PASS.
  CLAUDE.md `### SSH ControlMaster reuse` updated with the
  new field-validated invariant so future ssh-spawn sites
  must include the flag.

- **`inspect add --non-interactive`: documented `INSPECT_<NS>_HOST=...`
  env-var form was fictional.** MANUAL.md §3.2 advertised a headless-
  setup recipe (`INSPECT_ARTE_HOST=... INSPECT_ARTE_USER=... inspect
  add arte --non-interactive`), but `src/commands/add.rs` only
  consults CLI flags — the env vars were never read. An agent
  following the manual got `missing required value for 'host' in
  non-interactive mode` with no hint that the env-var form was a
  doc claim, not an implementation. Caught live as the very first
  command of the smoke session that exercised the new
  LLM-trap-fix-on-first-surface policy. Sweep landed in one commit
  per the policy (extinguish the class, not the instance):
    - `docs/MANUAL.md` §3.2 rewritten to show the working
      `--host` / `--user` / `--key-path` / `--port` /
      `--key-passphrase-env` flag form with a real-shaped example.
    - `LONG_ADD` in `src/cli.rs` rewritten to enumerate the
      required-flag set under `--non-interactive` and explicitly
      disclaim the env-var form ("There is NO env-var form
      (`INSPECT_<NS>_HOST=...` is not consulted)") so an agent
      reading `inspect add --help` cannot fall into the same trap.
    - `AddArgs.long_about` (the inline duplicate of the same prose
      that had drifted from `LONG_ADD`) replaced with
      `long_about = LONG_ADD` so there's a single source of truth.
    - The error message itself now chains to recovery:
      `missing required value for 'host' in non-interactive mode\n
       hint: pass `--host <value>` on the command line (env vars
       like INSPECT_<NS>_HOST are not consulted)` so an operator
       hitting the failure gets the fix in the same line. The
       `_` in field names like `key_path` is converted to `-` to
       match the actual flag name (`--key-path`).
    - 3 regression tests in `tests/phase_f_v013.rs::smoke_add_*`
      pinning all three layers: success path with required flags,
      failure-with-hint shape, and `--help` output disclaiming
      the env-var form.
    - Help-search index cap raised 128 → 144 KB to fit the new
      LONG_ADD prose (per CLAUDE.md "raise the cap, do not trim
      docs"; cap-raise log in `src/help/search.rs`).

- **🔴 SIGPIPE panic on `inspect <verb> | head -N` (CRITICAL).**
  Surfaced live during the v0.1.3 release smoke when
  `inspect compose logs arte/luminary-atlas --tail 50 --match
  'error|Error' | head -20` panicked mid-stream with
  `thread 'main' panicked at 'failed printing to stdout: Broken
  pipe (os error 32)'`. Rust's stdlib installs `SIG_IGN` for
  SIGPIPE at process start, so writes to a closed pipe surface
  as `EPIPE` — and `println!` / `writeln!` turn that into a
  panic + exit 101 + backtrace on stderr. For an agent-facing
  CLI that is constantly piped through `head`, `grep -m1`, `jq`
  etc., every short-circuited pipeline ended in a backtrace
  instead of the conventional silent exit 141 = 128 + SIGPIPE.
  Fix: `exec::cancel::install_handlers` now also calls
  `signal(SIGPIPE, SIG_DFL)`, restoring the Unix-default
  disposition. An early-closing reader now terminates `inspect`
  silently like every other Unix CLI. Regression test
  `smoke_sigpipe_no_panic_on_early_pipe_close` spawns `inspect
  --help` with stdout piped, drops the reader after one line,
  and asserts neither exit 101 nor a `Broken pipe` / `panicked
  at` line on stderr.

- **`audit ls` ordering + projection: help text now spells out
  newest-first and the `revert`-block omission.** The smoke
  agent burnt two cycles assuming `audit ls --json | tail -1`
  yielded the most recent entry (it yields the OLDEST in the
  page; `audit ls` already sorts via
  `sort_by_key(Reverse(e.ts))`) and another cycle expecting
  `revert.kind` to be present in `audit ls --json` (the projection
  emits id/ts/verb/selector/exit/diff_summary/is_revert/reason
  only — the `revert` block lives on `audit show <id> --json`).
  No code change to `audit ls`; instead the help surface is
  hardened so the same trap can't repeat:
    - `LONG_AUDIT_LS` gains an "ORDERING + JSON PROJECTION
      (agent-recipe critical)" section with a `head -1` example
      and an explicit "round-trip through `audit show`" pointer
      for the revert block.
    - The clap `///` doc comments on `Ls`, `Show`, and `Grep`
      carry the same warning so it surfaces at `inspect audit
      ls --help` / `inspect audit show --help` / `inspect audit
      grep --help` (the leaf surface LLM agents hit first).
    - `docs/SMOKE_v0.1.3.md` LLM-trap §3 rewritten and every
      `inspect audit ls --tail N --json` recipe (which was also
      using a non-existent `--tail` flag — the real flag is
      `--limit`) replaced with the canonical `--limit N --json
      | jq '.[0]'` form, plus a put-roundtrip recipe that walks
      `audit show` for the `revert.kind`.

- **F11 `command_pair` capture-site discipline: `payload` is the
  literal command, never CLI prose.** Surfaced live during the
  v0.1.3 smoke after `inspect put` of a brand-new file: the
  audit's `revert.payload` was the human prose
  `inspect put created arte:/tmp/...` while the runnable
  `rm -f -- ...` was buried in `revert.preview` (args reversed).
  `revert_command_pair` dispatches `payload` through the runner,
  so direct revert would have tried to run that prose as a
  remote shell command and failed with `command not found`. The
  put-create site now passes the real shell command first; the
  contract is cemented by inline comments. Two adjacent capture
  sites that violated the same anti-pattern were swept in the
  same commit:
    - **`inspect ssh add-key`**: payload was
      `inspect ssh add-key <ns> --apply` (the *forward* CLI
      wrapper, not an inverse). Now `Unsupported` with the
      manual `sed -i '\\|<line>|d'` revoke command in the
      preview — matching the comment that already said "no clean
      automatic inverse" and the F11 contract "never silently
      no-op".
    - **`inspect bundle … compose: …` per-step entries** for
      `up` / `down` / `restart` / `build`: payload was an
      `inspect compose <inv> {sel} --apply` CLI wrapper, which
      the runner cannot dispatch on the remote target (`inspect`
      is not installed there). Direct
      `inspect revert <compose-step-audit-id> --apply` would
      have failed with `command not found`. All four are now
      `Unsupported` with the manual inverse verb in the
      preview; bundle-level rollback (`bundle apply --on-failure
      rollback`) is unaffected because it walks the composite
      parent audit locally with the original flag set, and that
      path was already correct. `LONG_BUNDLE` updated to match.
  Standalone `inspect compose <action>` verbs (outside the
  bundle runner) were already `Unsupported` and unaffected.

- **F11 `state_snapshot` revert: snapshot path prefix mismatch.**
  Surfaced during the v0.1.3 release smoke against an Alpine
  sandbox: `inspect revert <edit-audit-id> --apply` failed with
  `reading snapshot …/sha256-sha256:HEX … No such file or
  directory`. Capture sites stamp `previous_hash` as
  `"sha256:HEX"` (colon, the audit-entry convention), but
  `SnapshotStore::{get,path_for}` only stripped the `"sha256-"`
  on-disk filename prefix, so the colon-form leaked through and
  built a doubly-prefixed `sha256-sha256:HEX` path. Snapshot
  store now strips both prefixes via a shared
  `strip_sha256_prefix()` helper.

- **F11 atomic-write snippet was GNU-only (`chmod --reference`).**
  Same smoke turn, against `nginx:alpine` (BusyBox coreutils):
  every `inspect edit` apply spewed BusyBox `chmod` usage
  (`Usage: chmod [-Rcvf] MODE…`) because `chmod --reference=PATH
  FILE` and `chown --reference=PATH FILE` are GNU-only. The
  atomic-rename snippet (used by `edit`, `put`, `cp` via the
  shared `verbs/write/atomic.rs` and `verbs/transfer.rs`
  helpers) now reads the prior mode/owner with POSIX-portable
  `stat -c '%a' PATH` / `stat -c '%u:%g' PATH` and re-applies
  via plain `chmod` / `chown`. `chmod` failure still aborts the
  apply (mode preservation is required); `chown` failure is
  tolerated (root-only, the existing `2>/dev/null || true`).
  Unit tests `snippet_preserves_mode_via_stat`,
  `atomic_script_mirrors_prior_mode_and_owner`, and
  `atomic_script_applies_mode_override_after_mirror` were
  rewritten to assert the new `stat -c` form and to fail loudly
  if anyone reintroduces `--reference=`.

- **`inspect revert <id> --apply` no longer prompts interactively.**
  Surfaced during the v0.1.3 release smoke against arte: a
  targeted `revert <audit-id> --apply` blocked silently on a `[y/N]`
  stdin prompt routed to stderr, with no preceding summary line.
  Pipelines and agent callers can't answer the prompt and saw it as
  a hang at 0% CPU. The audit-id plus `--apply` are already an
  explicit, double-witnessed intent — a third signal is overkill.
  All three revert paths (`command_pair`, `state_snapshot`,
  `composite`) now use the large-fanout interlock (no-op at
  `target_count=1`) instead of the unconditional `Confirm::Always`
  prompt. Drift detection on `state_snapshot` reverts still
  requires `--force`; `revert --last N` against a wide selector
  still trips the >10-target fanout interlock unless `--yes-all`
  is passed. `LONG_REVERT` gained a CONFIRMATION section
  documenting the new contract.

- **F11 `command_pair` revert wrapping bug.** Surfaced in the same
  smoke turn after the prompt was removed: `inspect revert
  <stop-audit> --apply` against a stopped sandbox container failed
  with `Error response from daemon: container <hash> is not
  running`. Capture sites disagreed about whose responsibility the
  `docker exec` wrap was — `lifecycle.rs` baked host-level inverses
  (`docker start <c>`) into `revert.payload`, while the
  in-container verbs (`chmod`, `chown`, `mkdir`, `touch`) wrote
  bare in-container payloads (`chmod 644 -- /path`) and trusted
  `revert_command_pair` to wrap. `revert_command_pair` then wrapped
  unconditionally whenever `step.container()` was `Some`, so
  lifecycle reverts were double-wrapped into
  `docker exec <stopped-container> sh -c "docker start <c>"` —
  doomed by definition because the container that should run the
  revert is the very container being revived. The contract is now
  capture-site-authoritative: `revert.payload` is the literal
  command the runner dispatches, exactly mirroring how the original
  verb dispatched. The four in-container verbs pre-wrap their
  inverse in `docker exec` at capture time; lifecycle / compose /
  ssh / steps reverts remain host-level. `revert_command_pair`
  runs the payload as-is, no wrapping, no second-guessing. Field-
  validated end-to-end on arte:
  `reverted audit 1777883334190-055d → arte/inspect-smoke-…
  (audit 1777884723466-216a)` after stop+revert against a real
  sandbox container.

- **`audit ls` / `show` / `grep` / `gc` / `verify` `--json` now emit
  the standard L7 envelope.** Surfaced live during the v0.1.3
  release smoke when the agent's `audit ls --json | jq '.[0]'`
  recipe failed with `Cannot index object with number` — the
  audit verbs were the last `--json` surface still emitting
  bare-NDJSON / bare-object shapes from before the v0.1.3
  envelope contract. Every other verb already wraps payloads in
  `{schema_version, summary, data, next, meta}`, so an agent
  expecting `.data.entries[]` on `audit ls` got a top-level array
  and the canonical `head -1` recipe broke. Fix: all five audit
  verbs now go through the shared `format::Envelope::emit_json`
  helper. `ls` payload is `{entries: [...]}`, `show` is
  `{entry: {...}}` (singular — see follow-up below), `grep` is
  `{matches: [...]}`, `gc` is `{removed, kept}`, `verify` is
  `{ok, mismatched, missing}`. `summary` carries the human-form
  one-liner; `meta` carries `count` / `order=newest_first` / `total`
  on `ls`. The clap `///` doc on each variant and `LONG_AUDIT_LS`
  were updated with the new path; help-search index re-fits within
  the 80 KB cap. Field-validated against arte during the same
  smoke turn — `audit ls --json | jq '.data.entries[0].id'` works
  end-to-end and `head -1`/`.[0]` recipes round-trip.

- **`audit show <unknown-prefix>` error template no longer leaks
  the literal `{id_prefix}` placeholder.** Surfaced one smoke turn
  after the envelope standardization above: a deliberately bad
  prefix (`audit show deadbeef`) printed `error: no audit entry
  matches id prefix '{id_prefix}'` instead of interpolating the
  prefix the operator typed. Cause: the `crate::error::emit` call
  was passed a static `&str` with brace-syntax, never run through
  `format!`. Fix: wrap in `format!(...)` so the prefix interpolates.
  In the same commit, the `Show` clap `///` doc spells out the
  envelope path explicitly — `audit show <id> --json` returns the
  full `AuditEntry` under `.data.entry` (singular, parallel to
  `ls`'s `.data.entries`), with a copy-paste recipe so an agent
  reading `inspect audit show --help` doesn't expect the entry
  fields directly on `.data` and pull nulls. Field-validated on
  arte: error renders `error: no audit entry matches id prefix
  'deadbeef'` and `audit show <real-id> --json | jq '.data.entry'`
  yields the full populated record (`verb`, `exit`, `args`,
  `stdin_bytes`, `stdin_sha256`, …).

### Added — pain-point-audit documentation (G6 / G7 / G8)

- **`inspect help safety` — Encoding-bypass and multi-line
  limitations.** Added a "Known redaction limitations" subsection
  noting that L7 line-oriented masking does not unwrap
  `base64`, `xxd`, `hex`, gzip, or JSON-encoded payloads
  (G6) and does not mask multi-line shell-style assignments where
  the secret continues past line 1 (G8 — PEM keys are still safe
  via the dedicated PEM masker). Recommends `--show-secrets`
  discipline for known-encoded outputs and prefers single-line
  `KEY=value` env over heredoc continuation.
- **`inspect help watch` and `LONG_WATCH` — Point-in-time
  predicates.** Documented (G7) that `--until-cmd` /
  `--until-http` evaluate at one polling instant and a transient
  flap satisfies the predicate. Operators wanting "stable for N
  consecutive samples" should wrap `inspect watch` in a shell
  loop or use `--stable-for` (already present, see help). A
  first-class `--min-consecutive` is tracked for v0.1.4+.

### Added

- **L3 — Parameterized aliases.** Aliases were static strings; a
  recipe that wanted "logs for *any* service on arte" had to either
  define one alias per service or fall back to writing the full
  selector each time. Agentic callers couldn't compose aliases
  programmatically. L3 adds `$<ident>` placeholders in alias bodies,
  bound at call time via `@name(key=val,key=val)`. Bare `@name`
  still works for parameterless aliases, so every pre-L3 alias keeps
  working byte-identically. Aliases may chain other aliases up to
  depth 5; definitional cycles are rejected at `alias add` time
  with the cycle printed back. The on-disk `aliases.toml` schema
  gained an optional `parameters: []` cache field that pre-L3
  entries simply omit (deserializes unchanged).
  - **`$<ident>` syntax.** Placeholder names are
    `[a-zA-Z_][a-zA-Z0-9_]*`. `$$` is a literal-`$` escape for
    operators that genuinely want a `$` in the body (rare, but
    free to support). Placeholders are recognized everywhere in the
    body — including inside `"..."` quoting — so the canonical
    LogQL example `{server="arte", service="$svc"}` works exactly
    as expected. Extraction is one byte-walk; no full-grammar
    re-lex.
  - **`@name(k=v,k=v)` call sites.** A small parser handles
    `@name`, `@name()`, `@name(k=v)`, `@name(k=v,k=v)`, and
    `@name(k="v with spaces and, commas")`. Quoted values support
    `\"` / `\\` / `\n` / `\t` escapes; unquoted values stop at the
    next top-level comma or `)`. Param values that contain commas
    must be quoted. Same param twice → exit 2. Empty param name
    → exit 2. Missing `)` → exit 2. The error messages quote the
    alias name and the offending param so an agentic caller can
    correct without a separate help lookup.
  - **Chain expansion.** An alias body may reference other aliases
    via `@other(...)`. `expand_recursive` walks the references with
    a depth cap of 5; depth 6 errors with the full chain printed
    (`a -> b -> c -> d -> e -> f`) so the operator can see exactly
    which level overflowed. Definitional cycles are caught at
    `alias add` time by a depth-first walk over the would-be alias
    graph; the cycle is reported `a -> b -> a` and the alias is
    not written to disk. The runtime depth-cap is a belt-and-
    suspenders guard against hand-edited `aliases.toml` files that
    bypass the add-time check.
  - **Error envelope.** Five new `AliasError` variants —
    `MissingParam`, `ExtraParam`, `CircularReference`,
    `ChainDepthExceeded`, `BadCallSyntax` — each rendering with
    the alias name and the declared params so an agent gets
    `requires param 'svc' (declared params: svc, lvl; call as
    @svc-logs(svc=...,lvl=...))` rather than a generic "missing
    param" string. `MissingParam` and `ExtraParam` exit 2 (usage
    error) consistently with the existing alias-error pattern.
  - **`parameters: []` discovery.** `inspect alias show <name>
    --json` and `inspect alias list --json` include a
    `parameters` array per entry — the same names L3 extracts at
    `alias add` time. An agent can enumerate `inspect alias list
    --json | jq '.[] | {name, parameters}'` and discover every
    parameterized alias without trial-and-error. The text-side
    `alias list` and `alias show` output gain a parenthesized
    `(svc, lvl)` tag and a `parameters = [...]` data line
    respectively, on aliases that take params (parameterless
    aliases render unchanged so existing scripts/recipes parse
    byte-identically).
  - **Schema cache (`parameters: Option<Vec<String>>`).** Stored
    as `Option` with `skip_serializing_if = Option::is_none` so
    pre-L3 `aliases.toml` files (no `parameters` field) deserialize
    unchanged and re-serialize unchanged unless the alias is
    re-`add`ed. `Some(empty)` and `None` are operationally
    equivalent for parameterless aliases — the field is a cache,
    not a flag.
  - **LogQL pipeline integration.** `src/logql/alias_subst.rs`
    grew a parameter-aware resolver signature
    (`Fn(&str, &BTreeMap<String, String>) -> ResolverResult`); the
    default resolver in `src/logql/mod.rs` delegates to
    `alias::expand_recursive` so chain unwinding + `$param`
    substitution + cycle detection happen once before the LogQL
    parser sees the substituted text. Span tracking (audit §1.7)
    extended so a `@svc-logs(svc=pulse)` call site's downstream
    parse errors point at the **whole** call site, not just the
    `@svc-logs` prefix.
  - **Verb-side integration.** `src/selector/resolve.rs` already
    delegated to `alias::expand_for_verb`; that function now
    parses the call-site `(...)` group internally so every read
    and write verb's selector argument accepts parameterized
    aliases without per-verb wiring changes. A `MissingParam`
    error from the alias layer surfaces as the same exit-2 chain
    every other selector error already produces.
  - **Help-text discoverability.** New PARAMETERIZED ALIASES
    section in `LONG_ALIAS` and `src/help/content/aliases.md` with
    worked examples (define + use + chain + show), the
    placeholder-syntax rules, the cycle / depth-cap policy, and
    explicit guidance on how `parameters: []` is used for agent
    discovery. The `LONG_ALIAS` constant is now the single source
    of truth — the previous duplicated `AliasArgs.long_about`
    inline string was replaced with a `long_about = LONG_ALIAS`
    reference. `AliasAddArgs.selector` clap docstring updated to
    document the L3 contract on `--help`.
  - **Defaults via `${ident:-default}`.** A placeholder may declare
    a default value; when the call site omits the param the default
    is substituted, when the call site provides the param the
    provided value wins. Defaults make the param optional — they
    are not part of the required-params validation. The original
    L3 backlog spec scoped defaults out of v0.1.3 ("deferred to
    v0.2.0 to keep the parser minimal"); during the L3 commit
    review the maintainer flagged this as a self-authorized
    deferral that the no-silent-deferrals policy explicitly
    forbids, and defaults shipped as part of L3. Implementation
    is ~80 LOC of additional scanner state in `scan_placeholders`
    and `parse_braced_placeholder` (both private to `src/alias.rs`)
    plus `extract_defaults` (public, used by `alias show --json` to
    expose the per-parameter `parameter_defaults: {name: default}`
    map for agent discovery). Default values may not contain `}`
    directly; use `\}` to embed a literal closing brace.
  - **Test coverage.** 13 inline unit tests in
    `src/alias.rs::tests::l3_*` (extract / substitute / call-site
    parser / chain happy path / self-cycle / pre-L3
    deserialization) and 12 acceptance tests in
    `tests/phase_f_v013.rs::l3_*` (parameters in `show --json`,
    `list --json`, bare `@name` round-trip, `$param` resolves at
    verb time, missing-param error format, extra-param error
    format, chain depth 5 succeeds, chain depth 6 errors with
    chain printed, definitional cycle rejected at add time,
    `$$` escape, quoted comma in param value, `--help`
    discoverability). Pre-L3 invariants kept: the
    `phase3_selector::alias_*` integration tests pass after
    swapping the obsolete `alias_rejects_chaining` for an L3
    `alias_rejects_definitional_cycle` (chains are valid now;
    cycles are still rejected, with a tighter error message).
- **L13 — Parallel multi-target fan-out within a single `--steps`
  step.** F17 ships sequential per-target dispatch: a step against 5
  targets runs them one after another. For the migration-operator
  workflow against a fleet (a primary scale axis for inspect's
  value proposition), that turns a 60 s × 5-target snapshot from
  60 s to 5 minutes — kills the value of the verb on >2-target
  invocations. L13 adds opt-in parallel fan-out that preserves
  every existing audit / JSON / revert contract.
  - **`parallel: true` per-step manifest field** (default `false`,
    so existing manifests behave identically). When set with a
    multi-target selector, the step's per-target work runs in
    parallel batches via `std::thread::scope`. Wall-clock becomes
    ~`ceil(N / parallel_max) × max(target_durations)` instead of
    `sum(target_durations)`. RemoteRunner is already `Send + Sync`
    so the runner shim threads cleanly across the scope.
  - **`parallel_max: <int>` per-step cap.** Default 8 (matches
    `inspect fleet`'s concurrency cap), hard ceiling 64. Values
    above the ceiling clamp; `Some(0)` is treated as the default
    (operators expect 0 to mean "no limit", but our actual
    contract is "default 8" — surface the ambiguity by using the
    default rather than spawning unbounded threads). Above 64
    simultaneous targets, operators are pointed at `inspect fleet`
    in the help text.
  - **Per-line writer mutex.** Each parallel target's output goes
    through the existing `<target> | <line>` prefix, but the
    per-line emit is now wrapped in a `Mutex<()>` shared across
    threads. Two targets cannot interleave bytes mid-line; they
    can interleave full lines between newlines. The
    `<target> |` prefix is the demux signal an agent uses. This is
    a documented trade-off: sustained burst output from 8 targets
    shows lines in completion order rather than per-target
    contiguous.
  - **`target_idx: usize` field on `AuditEntry`.** New
    `Option<usize>` field with `#[serde(default,
    skip_serializing_if = "Option::is_none")]` so pre-L13 entries
    deserialize unchanged. Stamped on per-(step, target) audit
    entries produced by a parallel step, recording the manifest's
    target-list index. A post-mortem walk in completion order can
    sort by `target_idx` to recover manifest order. Sequential
    entries elide the field (log order == manifest order); agents
    don't need to handle it specially.
  - **`on_failure: stop` coordination via the global cancel flag.**
    Pre-L13 the cancel function was `#[cfg(test)]`-gated with a
    rationale ("production code never calls this; the SIGINT
    handler does the same work via the `extern "C"` path"). L13
    invalidates that — `inspect run --steps` with `parallel: true`
    + `on_failure: stop` trips the flag internally when a parallel
    target completes with a failure, so peers in the next batch
    see `is_cancelled()` at dispatch start and skip without
    running. Same observable end-state as a SIGINT; sharing the
    cancel surface keeps the streaming-dispatch's pre-flight
    check as the single chokepoint. In-flight peers in the SAME
    batch as the failing target run to completion (their results
    are still captured).
  - **F11 composite-revert composition.** `--revert-on-failure`
    walks per-(step, target) records in reverse manifest order —
    already supported by the `target_idx` field. No additional
    work needed for the revert path.
  - **Per-target body extracted into `run_one_target`.** F17's
    inline 270-line per-target body factored out into a free
    function with a `PerTargetCtx` references-only context
    struct. The same function drives both sequential (single
    call per target) and parallel (one call per thread) paths,
    so future regressions only need fixing in one place. The
    sequential path's audit shape is byte-identical to F17 (no
    `target_idx` field, no per-line lock); the parallel path
    stamps `target_idx` and serializes emits.
  - **Help surface.** `LONG_RUN`'s MULTI-STEP section gains an
    "L13 (v0.1.3)" paragraph documenting the `parallel: true`
    field, the `parallel_max` cap + ceiling, the per-line mutex
    contract, the `target_idx` audit field, and the
    `on_failure: stop` semantics. `MANUAL.md` §7.9 multi-target
    dispatch sub-section gains a worked YAML example +
    parallel-fan-out timing analysis + the audit-link-ordering
    rationale.
  - **Tests.** 6 acceptance tests in
    `tests/phase_f_v013.rs::l13_*`: `parallel: true` on a step
    runs targets concurrently (timing assertion: parallel
    wall-clock < 1.5× max-target-duration vs sequential ≈
    `sum`); `parallel_max` clamps batch size; per-(step, target)
    audit entries carry `target_idx` only on parallel steps;
    `targets[]` JSON array preserves manifest order regardless
    of completion order; per-line writer mutex prevents byte
    interleaving (lines never split mid-character); help-topic
    surfaces the L13 contract. Full suite green: 28 suites, 0
    failed.

- **L12 — Per-step live streaming under `--steps --stream` with L7
  redaction + F18-style step boundaries.** F17's per-step dispatch
  already emits lines via `tee_println!` as `run_streaming_capturing`
  fires its per-line callback, so the spec's headline concern
  ("buffered until step exits") was a misread of the F17 implementation.
  The actual gaps L12 closes:
  - **F17's live tee bypassed L7 redaction.** A step that emitted a
    `Bearer <token>` HTTP header, a `postgres://user:pass@host` URL,
    or a PEM private-key block leaked the secret BOTH to the
    operator's terminal AND into the captured `targets[].stdout`
    audit field. `inspect run` (without `--steps`) has applied the
    L7 four-masker pipeline (PEM → header → URL → env) since L7
    shipped; the F17 multi-step path silently bypassed it.
  - **F17 boundaries (`STEP <name> ▶/◀`) didn't match the F18
    transcript fence format**, so an operator skimming the live
    tail of a multi-step migration saw a different shape than what
    `inspect history show` would render for the same blocks.
  - **Per-step audit entries had no `secrets_masked_kinds` field**,
    so `inspect audit grep secrets_masked` wouldn't surface
    multi-step invocations even when secrets had been redacted.

  L12 fixes all three.
  - **L7 redaction in the per-step closure.** New per-(step, target)
    `OutputRedactor` (one per pair so PEM-block gate state can't leak
    across steps or targets). Every line goes through `mask_line`
    BEFORE both the live `tee_println!` and the captured
    `step_stdout` push. Lines inside a PEM block return `None` and
    are suppressed (single `[REDACTED PEM KEY]` marker on the BEGIN
    line, same contract as bare `inspect run`). `--show-secrets`
    bypasses everything (same flag, same path).
  - **F18-style step boundaries under `--stream`.** The opener is
    `── step N of M: <name> ──`; the closer is `── step N ◀
    exit=… duration=…ms audit_id=… ──`. The `audit_id` on the
    closer cross-links back to that step's `run.step` audit entry,
    so an operator copy-pasting a fence pair from the live tail
    into `inspect audit show <audit_id>` works without further
    translation. Multi-target form keeps the per-target sub-line
    inside the same step block, with the audit_id on each
    sub-line. Without `--stream`, the legacy `STEP <name> ▶/◀`
    form remains (non-streaming runs are typically short and
    don't benefit from the F18 fence's extra horizontal real
    estate).
  - **`secrets_masked_kinds` on per-step audit entries.** When the
    redactor for a (step, target) pair fired, the per-step
    `run.step` audit entry now carries the list of kinds that
    fired (canonical order: `pem`, `header`, `url`, `env`).
    `inspect audit grep` queries that already match against bare
    `inspect run` entries now match per-step entries too.
  - **Capture cap unchanged.** The 10 MiB per-(step, target) cap
    on `step_stdout` stays in effect — live output is uncapped
    (the operator's terminal is the bound), but the captured copy
    that lands in the `targets[].stdout` JSON field is still
    capped with the existing `[OUTPUT CAPTURE TRUNCATED AT 10
    MIB]` marker on overflow. Capture happens AFTER masking so
    the captured copy contains masked content only.
  - **Composes with L11.** `--steps --stream --stdin-script` works
    end-to-end: the manifest is read normally, `cmd_file` script
    bodies still ride F14's `bash -s` stdin path (no L11 two-phase
    needed because manifest steps don't take stdin from the
    operator's tty). The bare `--stdin-script` flag (the manifest
    body coming from stdin) is mutex with `--steps` per F17, so
    the L11 path doesn't apply to the steps mode.
  - **Help surface.** `LONG_RUN`'s MULTI-STEP section gains an
    "L12 (v0.1.3)" paragraph documenting the F18 fence format,
    the L7 redaction pipeline, and the audit_id cross-link
    contract. `MANUAL.md` §7.9 multi-step section gains a worked
    example showing the F18-style fence with the audit_id on each
    closer.
  - **Tests.** 5 acceptance tests in `tests/phase_f_v013.rs::l12_*`:
    `--steps --stream` renders F18-style step boundaries; the
    legacy `STEP <name> ▶/◀` form still appears without
    `--stream`; PEM block in step output is masked to a single
    marker (live-tee redaction working); URL credentials in step
    output are masked; per-step audit entry carries
    `secrets_masked_kinds` when a step emitted a PEM block.
    Full suite green: 28 suites, 0 failed.

- **L11 — Bidirectional `--stream` + `--stdin-script` composition via
  two-phase dispatch.** F14 (`--stdin-script`) and F16 (`--stream`)
  were clap-mutually-exclusive in v0.1.3 because feeding the script
  body via SSH stdin while forcing `-tt` PTY for streaming output
  put both directions through the same tty layer — line-discipline
  echo, cooked-mode munging, interactive bash prompts on a non-tty
  stdin. The clap rejection produced a clean error but blocked the
  migration-operator workflow ("stream a 200-line setup script in
  and tail its output live"); the workaround was `--file <path>
  --stream`, which loses the heredoc / pipeline ergonomic. L11 ships
  the composition without a custom framing protocol.
  - **Two-phase dispatch.** When `--stream` is set with a script
    source (`--stdin-script` or `--file`), the verb takes a new
    path that splits the directions in time:
    - Phase 1 (`cat > /tmp/.inspect-l11-<sha>-<pid>.sh && chmod 700
      <…>`): writes the script body to a remote temp file via SSH
      stdin in a single round-trip with NO PTY. `umask 077`
      ensures the file is operator-only. Errors here surface as
      "L11 phase 1 failed" with chained hints (check /tmp
      writability + disk space + `inspect put` as alternative).
    - Phase 2 (`<interp> <tempfile> -- <args>`): runs the temp
      script with `-tt` PTY for line-streaming output. No stdin
      payload (already on disk).
    - Phase 3 (`rm -f <tempfile>`): runs unconditionally after
      phase 2 so a non-zero script exit leaves no orphan. Failures
      here are warnings (the verb has already produced its
      output); `tee_eprintln!` so transcripts capture the warning.
  - **Per-(SHA, pid) temp filename.** `/tmp/.inspect-l11-<8 chars
    of SHA-256>-<local PID>.sh`. The SHA prefix maps the file back
    to the audit entry; the PID prevents concurrent
    `inspect run --stream --stdin-script` invocations on the same
    script from stomping on each other's temp.
  - **Container selectors take the same shape**, wrapping each
    phase in `docker exec -i <ctr> sh -c '…'` (phase 1 + 3) /
    `docker exec <ctr> sh -c '…'` (phase 2). Three round-trips
    per `--stream --stdin-script` invocation against a container
    selector — negligible for the migration-operator workflow
    (one invocation, many minutes of runtime).
  - **`bidirectional: true` audit field.** New `Option<bool>`
    field on `AuditEntry` (`#[serde(default,
    skip_serializing_if = "is_false")]` so pre-L11 entries
    deserialize unchanged). Stamped on every L11 invocation;
    pre-L11 paths leave it false. Lets a post-mortem query
    (`inspect audit ls --bidirectional` / `inspect audit grep
    bidirectional=true`) pull the L11 invocations apart from
    bare `--stream` runs.
  - **`--file <path> --stream` also routes through L11** for
    consistency. The old direct path under `--stream --file`
    technically piped the script body via the same PTY tty layer
    that L11 was designed to avoid; the docs claimed it worked
    but the underlying half-duplex conflict was the same. Two
    extra round-trips per invocation; one-time cost for a
    multi-minute script. Strictly more correct than the pre-L11
    path.
  - **No new framing protocol.** The L11 spec proposed a custom
    1-byte-tag + 4-byte-length-prefix multiplexing protocol over
    a single SSH channel. Two-phase dispatch achieves the same
    operator-visible contract with no remote helper script, no
    client-side framing layer, no fallback negotiation. The
    spec's "Likely shape" was a hypothesis; this is a simpler
    implementation that meets every acceptance test in the
    spec's contract list.
  - **Help surface.** `LONG_RUN`'s `STREAMING (F16, v0.1.3)`
    section gains an "L11 (v0.1.3)" paragraph documenting the
    two-phase dispatch + per-(SHA, pid) temp filename. The
    `--stdin-script` flag docstring drops the "deferred to
    v0.1.5" wording and replaces it with the L11 composition
    note. `MANUAL.md` §7.8 (streaming) gains a sub-section
    walking phase 1 / phase 2 / phase 3 with an end-to-end
    example.
  - **Tests.** 5 acceptance tests in
    `tests/phase_f_v013.rs::l11_*`: clap accepts `--stream
    --stdin-script` together (no longer rejected as mutex); the
    rendered phase 1 command shape matches `cat > /tmp/.inspect-
    l11-<sha>-<pid>.sh && chmod 700`; phase 2 shape invokes the
    temp file; phase 3 cleanup runs after phase 2; `bidirectional`
    audit field set when both flags compose. Full suite green: 28
    suites, 0 failed. Real bidirectional dispatch against a live
    host is exercised by the field-validation gate (the same
    release-time smoke that L7 PEM streaming and L4 password
    prompts rely on).

- **L10 — Port-level entries in `DriftDiff` (`port_changes` array
  with `Added` / `Removed` / `Bind` / `Proto` kinds).** The B4
  drift differ surfaced container-level changes only — an operator
  who reorganized a compose project from `5432:5432` to `5433:5432`
  (dodging a port collision; a real-world common change) saw no
  drift signal because every container was the same. v0.1.3 closes
  the gap with structured port-level diffing in the same cheap
  probe used for container ids.
  - **`Ports` column parser.** New `src/discovery/ports_parse.rs`
    handles every shape collected from the field corpus: IPv4 binds
    (`0.0.0.0:5432->5432/tcp`), bracketed IPv6 binds
    (`[::]:53->53/udp` and `[::1]:5353->5353/udp`), ranges
    (`0.0.0.0:8000-8002->8000-8002/tcp` expands to 3 records),
    unbound exposed ports (`5432/tcp` records `host: 0` to
    distinguish from an actual `:0` bind), comma-separated lists,
    and proto-less tokens (default `tcp`, matching docker's own
    default). Each parsed token is sorted canonically by
    `(container, proto, host)` so the diff layer compares
    `Vec<Port>` by index without reorder. Unrecognized tokens
    silently drop — better to under-report than mis-report (drift
    detection is a "did anything change" surface; the worst class
    of bug is "no, nothing changed" when something did).
  - **Cheap probe extended.** `cheap_rows` now includes
    `{{.Ports}}` in the `docker ps --format` template alongside the
    existing `{{.ID}}\t{{.Names}}\t{{.Image}}`. Same ssh round-trip;
    zero extra cost. Pre-L10 cached profiles deserialize unchanged
    (the legacy 3-column case parses to `ports: vec![]`).
  - **`DriftRow.fingerprint_line`** now folds the port set into the
    SHA-256 fingerprint. A bind-only change (`5432:5432` →
    `5433:5432`) flips the fingerprint and surfaces a drift signal —
    pre-L10 it was silent because the fingerprint only saw
    `id\tname\timage`.
  - **`PortChangeKind` taxonomy.** Closed enum with four variants:
    - `Added` — `before: None, after: Some(p)`
    - `Removed` — `before: Some(p), after: None`
    - `Bind` — same `(container_port, proto)`, different host
    - `Proto` — same `(host, container_port)`, different proto
    The naive set diff would surface a proto change as
    `Removed(tcp) + Added(udp)`; a coalescing pass folds those into
    one `Proto` entry when the host matches (the operator's intent
    was "flip this port's transport", not "remove one and add
    another").
  - **Container-level vs port-level scope.** Containers entirely
    added or removed surface in the existing `added` / `removed`
    container-level lists and do NOT also fan their per-port deltas
    into `port_changes` — that would double-count the operator's
    intent. Only containers present in both snapshots contribute to
    `port_changes`.
  - **Output.** Human form gains a `⚓N port-level changes:` block
    matching the existing `+N added` / `-N removed` / `~N changed`
    shape; payloads render as `<host>:<container>/<proto>` (or
    `exposed <container>/<proto>` when `host == 0`) so an
    `inspect ports` confirmation requires no mental translation.
    JSON envelope gains a `port_changes: [{container, kind, before,
    after}]` array with `before` / `after` as either `null` or a
    structured `{host, container, proto}` object.
  - **Composes with L9.** UDP port changes flow through the same
    path as TCP — L9 made `proto: "udp"` first-class on the cached
    side, and the parser already understood `/udp` tokens. No
    additional probe work needed.
  - **Help surface.** `discovery.md` editorial topic gains a "DRIFT
    DETECTION" expansion documenting the four `kind` values, the
    container-level-vs-port-level scope rule, and the example
    output. `MANUAL.md` §3.6 (new) walks the contract end-to-end
    with the structured JSON envelope shape.
  - **Tests.** 18 inline unit tests in
    `src/discovery/ports_parse.rs::tests::l10_*` covering every
    field-corpus shape (IPv4 / IPv6 / range / unbound / comma /
    proto-less / mixed-proto / arity-mismatch / canonical sort /
    legacy proto suffix / range with offset host:container). 12
    inline tests in `src/discovery/drift.rs::tests::l10_*`
    (port-added/removed/bind/proto each isolated; unchanged port
    set yields empty `port_changes`; added-container does NOT
    double-count; the spec's 5-container fixture produces exactly
    4 entries; human form renders the port block; JSON envelope
    includes the array; unbound port renders as `exposed N/proto`;
    `DriftRow.fingerprint_line` includes ports). Full suite green:
    28 suites, 618 unit + 256 acceptance, 0 failed.

- **L9 — UDP listener probe in `discovery::probes` + `--proto` filter
  on `inspect ports`.** The host-listener probe pre-L9 scanned only
  TCP (`ss -tlnp` / `netstat -tlnp`), so UDP services on managed
  appliances — DNS forwarders, mDNS responders, syslog receivers on
  `:514/udp`, IPSec daemons, WireGuard endpoints — were invisible to
  `inspect ports` and `inspect status`. Operators chasing UDP issues
  fell back to running raw `ss -uln` over `inspect run`. v0.1.3
  closes the gap.
  - **Probe extension.** `probe_host_listeners` now runs both probes
    (TCP first, UDP second) in independent attempts: a host that
    surfaces TCP listeners but rejects UDP probing still surfaces
    TCP. Each probe falls back to the matching `netstat -[tu]lnp`
    invocation when `ss` is missing. The "no host-port listing
    available" warning fires only when both axes fail. New
    `parse_listener_line_with_proto(line, proto)` parser shared by
    both axes (the line shape is identical between TCP and UDP;
    only the proto differs).
  - **Profile schema.** `HostListener.proto` was already a `String`
    field defaulting to `"tcp"` (pre-L9 hardcoded the value); L9
    stamps `tcp` or `udp` explicitly per probe. The discovery
    engine's `already_mapped` check is refined to match on
    `(host_port, proto)` rather than `host_port` alone — a TCP
    service on `:53` no longer suppresses a host UDP listener on
    `:53` (distinct sockets, distinct triage paths).
  - **`inspect ports --proto tcp|udp|all`.** New flag (default
    `all`) on `PortsArgs`. Rendered command is built by
    `ProtoAxis::build_probe_cmd` — for `all`, the verb runs
    `ss -tlnp; ss -ulnp` (or `netstat` fallbacks) in one ssh
    round-trip, with `--- tcp ---` / `--- udp ---` markers between
    probes so the local parser can attribute every line to a proto.
    For `tcp` or `udp`, only the matching probe runs.
    `--port` / `--port-range` compose with `--proto`; the proto is
    a separate axis.
  - **PROTO column + JSON `proto` field.** Each emitted row prefixes
    the data line with `[tcp]` or `[udp]` so the proto is visible
    at a glance; the JSON envelope carries an explicit `proto` key
    on every row so agent consumers can branch without re-parsing
    the rendered text.
  - **Container-port path (`docker port <ctr>`)** unchanged at the
    probe layer (docker already includes the proto in its
    `<port>/<proto>` output); the verb reads the proto out of the
    line and applies the same `--proto` filter client-side.
  - **Help surface.** `LONG_PORTS` updated with the `--proto` flag
    and the L9 examples; `discovery.md` editorial topic gains a
    "UDP LISTENERS (L9, v0.1.3)" section that's blunt about the
    probe's bound-socket-vs-receiving-traffic distinction (so
    operators don't mistake "port shows up in `inspect ports`" for
    "service is healthy"). `MANUAL.md` §"Filtering `inspect ports`"
    expanded with a "Why UDP matters" sub-section, the worked
    examples, and the JSON-envelope `proto` field contract.
  - **Tests.** 3 inline unit tests in
    `src/discovery/probes.rs::tests::l9_*` (UDP `ss` row, UDP
    `netstat` row, IPv6 UDP `[::]:port` bind). 7 in
    `src/verbs/ports.rs::tests::l9_*` (`ProtoAxis::parse` accepts
    tcp/udp/all + safe-default; `build_probe_cmd` emits both
    markers + commands for `all`, narrows correctly for `tcp` /
    `udp`, falls back to `netstat` only when `ss` is absent;
    marker recognition; docker-port `/udp` suffix detection).
    9 acceptance tests in `tests/phase_f_v013.rs::l9_*` (UDP +
    TCP probe rows feed into the cached profile via the engine's
    `already_mapped` check; `inspect ports --proto udp` filters
    correctly; `inspect ports --proto tcp` excludes UDP rows;
    PROTO column rendered in human output; JSON envelope carries
    `proto`; mock-driven probe runs both axes in one ssh
    round-trip; help-topic + help-search discoverability for the
    `--proto` flag and the UDP-coverage notice). Full suite green:
    28 suites, 0 failed.
  - **Composes.** L9 lays the groundwork for L10 (port-level diff
    in `DriftDiff`) — once port-level diffing lands, UDP additions
    and removals between snapshots will surface automatically
    through the same parser.

- **L8 — Round out the v0.1.3 compose surface (per-service narrowing,
  `compose logs` triage flags, bundle `compose:` step kind).** F6
  shipped the first-class `inspect compose` verb cluster but with
  three deliberate scope cuts that operators kept hitting:
  `compose up`/`down` were project-level only; `compose logs` lacked
  the cursor / match / exclude / merged surface that `inspect logs`
  carries; and `inspect bundle` had no compose-aware step kind, so
  bundles drove compose via `exec:` shell strings and lost the
  structured audit shape. L8 closes all three.
  - **Per-service narrowing on every compose write verb.** F6's
    `pull` and `build` already accepted `<ns>/<project>/<service>`;
    L8 extends `up` (straight passthrough — appends the service
    token) and `down` to match. Per-service `down` uses the explicit
    `docker compose -p <p> stop <svc> && docker compose -p <p>
    rm -f <svc>` shape (compose's `down <svc>` form is undocumented
    and behaves inconsistently across versions; the explicit
    two-step is what every operator's runbook uses). Other services
    in the project remain running.
  - **Per-service `--volumes` and `--rmi` rejected loudly.** Both
    are project-scoped operations — silently honoring them against
    one service would either no-op (confusing) or wipe data shared
    with siblings (worse). The rejection error chains to a hint
    pointing at the project-level invocation as the next operator
    action.
  - **Audit `[service=<svc>]` tag.** Every per-service write entry
    stamps the service portion alongside the existing
    `[project=<p>] [compose_file_hash=<sha-12>]` so post-mortem
    queries can filter for the per-service slice without re-parsing
    the rendered command. The audit entry's `selector` field is
    also extended to `<ns>/<project>/<service>` (was
    `<ns>/<project>` before L8) so a `--bundle <id>` query joins
    cleanly across project- and service-level entries.
  - **`compose logs` triage surface (matches `inspect logs`).** New
    `--match <REGEX>` / `--exclude <REGEX>` flags (repeatable;
    multiple OR within each, AND across the two) reuse
    `verbs::line_filter::build_suffix` so the filter compiles to a
    remote `grep -E` pipeline — the SSH transport never carries
    lines we are about to drop. New `--merged` flag is an
    assertion: this is a multi-service interleaved stream (compose
    already does this by default at the project level, but
    `--merged` makes the contract explicit and rejects per-service
    selectors). New `--cursor <PATH>` flag resumes from the
    ISO-8601 timestamp recorded in the cursor file: forces
    `--timestamps` on `docker compose logs`, reads the stored
    timestamp as `--since`, and writes the latest seen timestamp
    back atomically (`<file>.tmp.<pid>` → rename(2), mode 0600).
    `--cursor` is mutex with `--since` (both pin the start). The
    timestamp parser is a hand-rolled byte-walk (no regex —
    streaming hot path) handling the
    `service_name  | YYYY-MM-DDTHH:MM:SS[.fff][Z|±HH:MM]` shape
    with and without service prefix.
  - **Bundle `compose:` step kind.** New `StepBodyKind::Compose`
    variant + `ComposeStepSpec { project, action, service?, flags
    }` schema. `ComposeAction { Up, Down, Pull, Build, Restart }`
    with a per-action `allowed_flags()` allowlist (up:
    `force_recreate, no_detach`; down: `volumes, rmi`; pull:
    `ignore_pull_failures`; build: `no_cache, pull`; restart:
    `<none>`). Plan-time validation: project must exist on the
    namespace's cached profile; every key in `flags:` must match
    the action's allowlist (typo'd flags are caught at plan time,
    never silently no-op'd at execution). New `run_compose_branch`
    in `src/bundle/exec.rs` resolves project from the cached
    profile, captures `compose_file_hash` via the same
    `compose_file_sha_short` helper as the standalone verbs,
    renders the command (per-service `down` uses
    `build_compose_per_service_down_cmd`; everything else uses
    `build_compose_cmd`), runs via `runner.run_streaming_capturing`
    for live progress, and stamps an audit entry with
    `verb=compose.<action>` so `inspect audit grep` joins
    bundle-driven and ad-hoc invocations on the same query.
    `revert.kind` taxonomy mirrors operator intent within bundle
    scope: `command_pair` for up/down/restart/build (the inverse
    points at the matching `inspect compose <action>`),
    `unsupported` for pull (no un-pull). Bundle rollback path
    extended to recognize `Compose` step bodies (the rollback shell
    block remains operator-authored — same as exec/run).
  - **Help surface.** `compose.md` editorial topic gains
    "PER-SERVICE NARROWING", a "BUNDLE compose: STEP KIND" section
    with a worked example, and `--match` / `--exclude` / `--merged`
    / `--cursor` notes under JSON SCHEMAS. `LONG_COMPOSE` updated
    with the new flag set + per-service selector grammar.
    `LONG_BUNDLE` documents the new step kind with the per-action
    allowlist and the revert taxonomy. `MANUAL.md` §7.10 gains
    sub-sections on per-service narrowing, the logs triage
    surface, and the bundle `compose:` step shape with a YAML
    example.
  - **Tests.** 7 inline unit tests in
    `src/verbs/compose/logs.rs::tests::l8_*` covering the ISO-8601
    prefix parser (with/without service prefix, with offset, no
    fractional seconds, no timestamp at all, empty line) plus the
    cursor read/write round-trip (atomic write + read, trailing
    newline trim, empty file = None). 12 acceptance tests in
    `tests/phase_f_v013.rs::l8_*` covering: dry-run shape per-
    service for up/down/pull/build (--apply omitted, expects "would
    on <ns>/<p>/<svc>" + audit `[service=<svc>]` not yet stamped);
    per-service `compose down --volumes` chained-hint rejection;
    per-service `compose down --rmi` chained-hint rejection;
    `--merged` rejects per-service selector with chained hint;
    `--match` + `--exclude` flag presence on the rendered command;
    `--cursor` round-trip via mock (writes atomic, reads non-empty
    on next call); `--cursor` mutex with `--since`; bundle
    `compose:` schema rejects unknown flags per-action; bundle
    `compose:` schema rejects multiple bodies; bundle `compose:`
    plan validates project against cached profile; help-topic +
    help-search discoverability for per-service + bundle compose
    step.

  Full suite green: 28 suites, 0 failed.

- **L2 — OS keychain integration for opt-in cross-session credential
  persistence.** v0.1.2 onboarding feedback (and the L4 password-auth
  retrospective): operators on legacy hosts wanted "enter the
  passphrase / password once, survive a reboot" without leaving
  secrets in env vars, `.envrc` files, or shell history. ssh-agent +
  ControlMaster covers within-session reuse already (and remains
  the recommended default); L2 fills the cross-session gap with an
  explicit opt-in, never an automatic side-effect.
  - **Default behavior unchanged.** Without `--save-passphrase`, every
    code path is byte-identical to v0.1.2: ssh-agent / per-session
    prompt; nothing written to the keychain. Operators who don't opt
    in see no change.
  - **Opt-in flag.** New `--save-passphrase` on `inspect connect`
    (with `--save-password` alias for the password-auth case).
    Prompts once, opens the master, saves the credential to the OS
    keychain under service `inspect-cli`, account `<ns>`. Idempotent
    re-saves are silent. Backend unavailable → warns once and
    continues without saving (the master still comes up).
  - **Auto-retrieval in the credential chain.** New step in
    `start_master` (key auth) and `start_master_password` (L4 password
    auth): after the env-var check and before the interactive prompt,
    `crate::keychain::get(namespace)` is consulted. A hit feeds
    through the existing SSH_ASKPASS pipeline (same path as the
    interactive prompt; the keyring secret never lands on disk).
    Misses and backend errors silently fall through — we never spam
    stderr on every connect because the keychain happens to be
    uninitialized. Two new `AuthMode` variants for observability:
    `keychain-passphrase` and `keychain-password`. F13 reauth and
    fleet prewarm explicitly do NOT save (an interactive
    `--save-passphrase` is the only path to persistence; reauth /
    prewarm shouldn't quietly persist on the operator's behalf).
  - **`inspect keychain ...` verb cluster.** New `Keychain(KeychainArgs)`
    enum variant + `commands::keychain` module.
    - `inspect keychain list` — show stored namespaces (no values).
      Self-healing: index entries the backend no longer recognizes
      are pruned silently. JSON envelope:
      `{"schema_version":1, "namespaces":[...], "backend_status":"available|unavailable", "reason":"..." (optional)}`.
    - `inspect keychain remove <ns>` — delete one entry. Audited
      with `verb=keychain.remove, args="[was_present=...]"`,
      `revert.kind=unsupported` (we don't store the secret;
      can't replay). Idempotent — removing an absent entry exits 0.
    - `inspect keychain test` — write/read/delete round-trip probe.
      Exits 0 on success, 1 with a chained hint when the backend
      is unreachable. Hint taxonomy matches the keyring crate's
      common failure modes (no DBus session bus, locked vault,
      missing keyring daemon).
  - **Backends.** macOS Keychain Services (`apple-native`), Windows
    Credential Manager (`windows-native`; reachable from WSL2),
    Linux Secret Service via DBus (`sync-secret-service`); covers
    GNOME Keyring and KDE Wallet. Pure-Rust crypto (`crypto-rust`)
    so we don't depend on system OpenSSL. `vendored` builds libdbus
    from source so the binary works on systems without dev headers.
  - **Index file.** `~/.inspect/keychain-index` (mode 0600, atomic
    writes via `<file>.tmp.<pid>` → rename). Lists namespace names
    only; no secret material. Exists because `keyring` v3.6's
    enumeration support is platform-spotty (Linux Secret Service
    has it; macOS / Windows expose it less cleanly). Self-healing
    via `keychain::list_namespaces()` which probes each entry's
    existence on every list call.
  - **Audit shape.** `keychain.remove` records `args=[was_present=true|false]`
    plus `revert.kind=unsupported` (we deliberately do NOT store the
    secret in the audit, so there is no replay path). Save is
    implicit in `connect --save-passphrase` and audited by the
    existing connect entry.
  - **Help surface (load-bearing for agentic callers).** New
    `LONG_KEYCHAIN` cluster help + `LONG_KEYCHAIN_LIST` /
    `LONG_KEYCHAIN_REMOVE` / `LONG_KEYCHAIN_TEST` per-sub help in
    `src/cli.rs`. `src/help/content/ssh.md` gains a "Credential
    lifetime (L2, v0.1.3)" section enumerating the three options
    (default ssh-agent / `--save-passphrase` / env var) with
    resolution-order tables for both auth modes.
    `("keychain", &["ssh", "safety"])` row added to `VERB_TOPICS`.
  - **Dependency.** New `keyring = "3.6"` dep with feature combo
    `apple-native + windows-native + sync-secret-service +
    crypto-rust + vendored` (default features off). Pre-approved by
    the L2 spec (the keychain space is the canonical example of the
    Dependency Policy's "genuinely unsafe to reimplement" exception:
    Secret Service DBus, Keychain Services API, and Windows
    Credential Manager each require platform-specific FFI that we
    are not in the business of writing).
  - **Tests.** 3 inline unit tests in
    `src/keychain/mod.rs::tests::l2_*` (`SaveOutcome` variant
    discriminator; namespace validation rejects internal-prefix and
    invalid names). 6 in `src/keychain/index.rs::tests::l2_*`
    (round-trip preserves sort + dedup; empty handling; missing
    file reads as empty; whitespace stripping; mode 0600 on unix).
    13 acceptance tests in `tests/phase_f_v013.rs::l2_*` (verb
    discoverability via help-topic + help-search; `--save-passphrase`
    flag presence on connect; keychain list empty-state; remove
    idempotent; test backend unreachable in sandbox; resolution-
    order per CLAUDE.md). Full suite green: 28 suites, 0 failed.
    The actual keychain backend round-trip requires a live OS
    keychain and is exercised by the field-validation gate (the
    same release-time smoke that L7 PEM streaming and L4 interactive
    password prompts rely on).
  - **Composes.** L2 layers cleanly on L4 (password auth) — both
    auth modes share the same keychain code path; the namespace's
    `auth` field implicitly disambiguates whether the stored secret
    is a passphrase or a password. The on-disk shape is one entry
    per namespace; the keychain itself does not need to know which
    kind of credential it stores.

- **L4 — Password authentication, extended session TTL, and audited
  `inspect ssh add-key` migration helper.** v0.1.2 onboarding
  feedback: legacy boxes and locked-down bastions that only accept
  password auth had no first-class path through `inspect`; operators
  were shelling out to plain `ssh` and losing the discovery cache,
  ControlMaster reuse, and audit trail. v0.1.3 closes the gap with
  three coupled additions.
  - **Profile schema.** Three new `Option<...>` fields on
    `NamespaceConfig` (`auth = "key"|"password"`, `password_env`,
    `session_ttl`); `serde(default)` + `skip_serializing_if`, so
    pre-L4 `~/.inspect/servers.toml` round-trips byte-identical.
    `validate()` rejects unknown auth modes ("kerberos" → loud
    config error), refuses `password_env` without
    `auth = "password"` (no silent semantic flip from a typo),
    parses `session_ttl` via the existing `ssh::ttl::parse_ttl`,
    and hard-caps any value above 24h so a forgotten laptop cannot
    hold a live remote session indefinitely.
  - **Password auth at connect.** New `AuthSelection.password_auth`
    + `AuthSelection.password_env` plus `AuthMode::{EnvPassword,
    InteractivePassword}` carry the mode through `start_master` in
    `src/ssh/master.rs`. The password branch skips the agent / key
    attempt entirely (forces `PubkeyAuthentication=no`,
    `PreferredAuthentications=password`, and
    `NumberOfPasswordPrompts=1` at the ssh layer) and reuses the
    existing `SSH_ASKPASS` pipeline for both env-var and
    interactive paths. Up to `PASSWORD_MAX_ATTEMPTS = 3` interactive
    attempts before aborting with a chained
    `see: inspect help ssh` hint and per-attempt diagnostics. A
    one-time per-namespace warning ("password auth is less secure
    than key-based") fires on first successful password connect,
    tracked via `~/.inspect/.password_warned/<ns>` so subsequent
    connects stay quiet. The marker is cleared by
    `inspect ssh add-key --apply` when it flips a namespace off
    password auth, so re-onboarding the same namespace later
    re-warns.
  - **Session TTL plumbing.** New `TtlSource::{PerNamespace,
    PasswordDefault}` variants and `resolve_with_ns(flag, per_ns,
    password_auth)` in `src/ssh/ttl.rs` — priority chain
    `--ttl flag` → `INSPECT_PERSIST_TTL env` → namespace
    `session_ttl` → password-default 12h → codespace 4h / local 30m.
    Cap is re-applied after resolution so `--ttl 48h legacy-box`
    (where `legacy-box.auth = "password"`) is rejected the same way
    `session_ttl = "48h"` is. Key auth is unchanged — only password
    mode picks up the longer default and the cap.
  - **`inspect ssh add-key <ns>` (audited write verb, L4).** New
    `Ssh(SshArgs)` enum variant + `commands::ssh` module. Default
    is dry-run; `--apply` performs the install + audit-log entry.
    Without `--key`, generates a fresh ed25519 keypair at
    `~/.ssh/inspect_<ns>_ed25519` (private 0600, public 0644);
    `--key <path>` reuses an existing key but refuses if the
    matching `.pub` is absent (no silent regeneration of operator
    key material). Public-key install rides the open ssh master so
    the operator's password is entered exactly once during the
    migration; the install is idempotent
    (`grep -F -x` + append-if-absent) and verifies by re-reading
    `authorized_keys` after the write. Remote permissions are
    normalized (`~/.ssh` 0700, `authorized_keys` 0600). After a
    successful install, the verb prompts to rewrite
    `~/.inspect/servers.toml` to `auth = "key"`,
    `key_path = "<path>"` (drops `password_env` and
    `session_ttl`). `--no-rewrite-config` skips the prompt; non-tty
    stdin auto-declines (no config writes without explicit
    confirmation). Audit shape:
    `verb=ssh.add-key, target=<ns>,
    args="[key_path=...] [generated=true|false] [installed=true]
    [config_rewritten=true|false]"`, `revert.kind=command_pair`
    documenting the manual `authorized_keys` remove (the verb does
    not attempt to revoke automatically — that requires further
    operator intent).
  - **`inspect connections` surfaces the new state.** Extended row
    schema with `auth` (`key`/`password`/`?`), `session_ttl`
    (configured), and `expires_in` (upper bound — ControlPersist
    resets on traffic so the real lifetime is at least this long;
    measured against the socket's mtime). Both human and JSON
    output get the new fields. The L4 spec literal said
    `inspect connectivity` but `connections` is the SSH-session-state
    verb (`connectivity` is the service-to-service network-edge
    probe); the rationalization is documented in the commit body.
  - **Help surface (load-bearing for agentic callers).** New
    `LONG_SSH` cluster help and `LONG_SSH_ADD_KEY` per-sub help in
    `src/cli.rs` document every flag, the audit-tag taxonomy, the
    config-flip semantics, and the exit-code contract.
    `src/help/content/ssh.md` rewritten with full credential-
    resolution chains for both auth modes, password-auth migration
    walkthrough, audit-entry shape, and security notes.
    `VERB_TOPICS` gains `("ssh", &["ssh", "safety"])`.
  - **Tests.** 13 inline unit tests in
    `src/config/namespace.rs::tests::l4_*` (auth-mode validation,
    `password_env` cross-field rule, `session_ttl` parsing + 24h
    cap, merge precedence both directions); 5 in
    `src/ssh/ttl.rs::tests::l4_*` (per-ns precedence, password
    default, key auth unchanged, flag wins, 24h cap). 1 in
    `src/commands/connections.rs::tests::l4_*` (compact duration
    formatter for the `expires_in` column). 2 in
    `src/commands/ssh.rs::tests` (default key path, pubkey-path
    derivation). 11 acceptance tests in
    `tests/phase_f_v013.rs::l4_*` (round-trip of password-auth
    namespace through `show --json`; 24h cap surfaces at config
    load with chained hint; `password_env` without
    `auth = "password"` rejected; unknown auth mode rejected;
    `add-key` dry-run preview for both auth modes;
    `--no-rewrite-config` dry-run notice; `--apply` without live
    session emits the chained `inspect connect <ns>` hint;
    `--key` missing-pub rejection; help-topic surfaces add-key +
    password auth; help-search finds the password-auth path;
    `connections --json` empty-list invariant; default auth is
    never serialized as `password`). Full suite green; the
    interactive password prompt and remote install paths require a
    real host and are exercised by the field-validation gate.
  - **Sequencing note.** Cross-session passphrase persistence via
    the OS keychain is L2 — the next item in the v0.1.3
    implementation order. L4 lands first because it establishes
    the password-auth + add-key path that L2 layers onto; both
    ship in v0.1.3.

- **F6 — First-class `inspect compose` verb cluster.** v0.1.2 field
  feedback ("no obvious `inspect compose` integration … first-class
  compose verbs would replace 80% of my `run` usage") — operators
  were dropping back to `inspect run arte -- 'cd /opt/luminary-onyx
  && sudo docker compose …'` for ps / logs / config / restart and
  losing the structured output, audit trail, redaction, and selector
  grammar in the process. v0.1.3 ships a complete compose surface so
  this fallback is no longer the path of least resistance.
  - **Discovery + cache schema.** New `ComposeProject { name,
    status, compose_file, working_dir, service_count, running_count
    }` struct on `Profile` (`#[serde(default)]`,
    `skip_serializing_if = "Vec::is_empty"`, so pre-F6 profiles
    deserialize unchanged). New `discovery::probes::probe_compose_projects`
    runs `docker compose ls --all --format json` over the persistent
    socket and is wired into `discovery::engine::discover` so every
    `inspect setup` populates the cache. The `Status` field is parsed
    into `(running_count, service_count)` via a small native parser
    (no extra crate per the Dependency Policy); both `running(N)` and
    `running(2), exited(1)` shapes are handled, and unknown states
    contribute to total but not running. The `--all` flag is intentional —
    operators want stopped projects to remain visible without having
    to remember a docker-side flag.
  - **Read sub-verbs (no audit, no apply gate):**
    - `inspect compose ls <ns>` — reads from the cached profile;
      `--refresh` (alias `--live`) re-probes via `docker compose ls`
      live. Multi-namespace selectors fan out and tag each project
      with its owning namespace in the JSON envelope. Empty
      namespaces emit `(no compose projects)` with chained next-steps
      pointing at `inspect setup` (cold cache) and `--refresh`
      (just-deployed project).
    - `inspect compose ps <ns>/<project>` — runs `docker compose -p
      <p> ps --all --format json` over the socket and renders a
      per-service table (service / state / image / ports / uptime).
      Tolerates both modern v2 ndjson and older single-array output;
      `Publishers` are formatted as `host:container/proto`. JSON
      schema: `data.services = [{service, state, image, ports,
      uptime}, ...]`. Down-service rollups suggest `inspect compose
      logs` as the next step.
    - `inspect compose config <ns>/<project>` — runs `docker compose
      -p <p> config` and streams stdout through the L7 redaction
      pipeline (PEM / header / URL / env maskers); `--show-secrets`
      bypasses. Summary line includes `— secrets masked` when any
      masker fired. The full body is preserved in `data.config` for
      agent consumers.
    - `inspect compose logs <ns>/<project>[/<service>]` — wraps
      `docker compose -p <p> logs --no-color [--tail N] [--since X]
      [--follow] [<svc>]`. Streaming with redaction on every line;
      `--follow` bumps the timeout to 8h (matches `inspect logs
      --follow`). Without a service portion, every service in the
      project is aggregated; with one, narrowed to that service.
  - **Write sub-verbs (audited; require `--apply`):**
    - `inspect compose up <ns>/<project>` — `verb=compose.up`. Default
      is `-d`; `--no-detach` switches to foreground. `--force-recreate`
      passthrough. Audit args stamp `[project=…] [compose_file_hash=
      <sha-12>]` plus `[no_detach=true]` / `[force_recreate=true]`
      when set. Revert: `kind=unsupported` with a preview pointing
      at `inspect compose down`.
    - `inspect compose down <ns>/<project>` — `verb=compose.down`.
      `--volumes` is destructive and stamps `[volumes=true]` into the
      audit args; `--rmi` adds `--rmi local` and stamps `[rmi=local]`.
      The dry-run preview surfaces a `(DESTRUCTIVE: --volumes would
      remove named volumes)` warning when applicable. Revert:
      unsupported.
    - `inspect compose pull <ns>/<project>[/<service>]` —
      `verb=compose.pull`. Streams via `run_streaming_capturing` so
      progress is visible during multi-minute pulls; the audit entry
      records `streamed=true` and `lines_streamed`. 30-minute
      timeout. `--ignore-pull-failures` passthrough with audit tag.
    - `inspect compose build <ns>/<project>[/<service>]` —
      `verb=compose.build`. Streams identically to pull (some builds
      legitimately take 30+ minutes). 1-hour timeout.
      `--no-cache` and `--pull` passthrough with audit tags.
    - `inspect compose restart <ns>/<project>/<service>` —
      `verb=compose.restart`. Without a service portion, refuses to
      fan out unless `--all` is passed (defensive default — "you
      didn't tell me which service, prove you really mean every
      service"). With `--all`, enumerates services via `docker
      compose -p <p> config --services` and iterates per-service so
      each gets its own audit entry. F8 cache invalidation runs
      after the loop so the next `inspect status` reflects post-
      restart state.
  - **Inspect-run-style sub-verb (no audit, no apply gate):**
    - `inspect compose exec <ns>/<project>/<service> -- <cmd>` —
      mirrors `inspect run`'s contract exactly: no audit, no apply
      gate, output streams through the L7 four-masker pipeline
      (`--show-secrets` and `--redact-all` honored). Service portion
      is mandatory (compose exec without a target service is
      meaningless). Forces `docker compose exec -T` so the output is
      line-oriented for the redactor. `--user` (`-u`) and `--workdir`
      (`-w`) are passthrough flags. 8h timeout matches `inspect run
      --stream` so a long-running interactive query inside the
      container can complete.
  - **Selector grammar.** Parsed inline by `verbs::compose::resolve`
    (not through `selector::resolve`, which would treat `<ns>/<x>`
    as `<ns>/<service>` and lose the project context). Three forms:
    `<ns>` for `compose ls`; `<ns>/<project>` for project-scoped
    verbs; `<ns>/<project>/<service>` for service-scoped verbs.
    The colon-shape selector (`<ns>:/path`) is rejected with a
    chained hint pointing operators at the file-path verbs. Unit
    tests pin every parse case.
  - **Project resolution.** `project_in_profile(ns, project)` looks
    up the project in the namespace's cached profile and returns a
    chained-hint error when the namespace has no cached profile, or
    when the project name doesn't match — the unknown-project error
    enumerates the *known* projects on that namespace so the
    operator's next typo recovery is one keystroke away.
  - **`inspect status` integration.** Status now reads
    `profile.compose_projects` for every selected namespace and
    emits a `compose_projects: N` line in the human DATA section
    (suppressed when N=0 to avoid noise on plain container hosts)
    plus an always-present `compose_projects` array in `--json`.
    Each JSON entry carries `{namespace, name, status, compose_file,
    working_dir, service_count, running_count}` — the same shape as
    `compose ls --json` so agents can navigate without re-binding.
  - **Help.** New editorial topic `compose` under `src/help/content/`
    documenting the read / write / exec sub-verb tiers, the audit-tag
    table, the selector grammar, the per-verb JSON schemas, the
    revert-kind = unsupported policy, and the discovery + status
    integration. Wired into `TOPICS`, `VERB_TOPICS`, the
    `SEE_ALSO_COMPOSE` cross-link, the `topic_count_matches_bible`
    invariant (14 → 15), and the `topic_ids` golden snapshot.
  - **Audit shape.** Five new `verb` values: `compose.up`,
    `compose.down`, `compose.pull`, `compose.build`,
    `compose.restart`. All five record `revert.kind = unsupported`
    with a preview that names the exact rollback command (when one
    exists) so `inspect revert <id>` returns useful chained hints
    instead of silently no-opping. The `args` field is a bracketed-
    tag string (`[project=<name>] [service=<name>] [compose_file_hash=
    <sha-12>] [<flag>=true] …`) so `inspect audit grep` filters by
    project, service, or specific flag with a substring match.
  - **Tests.** New `f6_*` acceptance suite in
    `tests/phase_f_v013.rs` covers the parse layer, the discovery
    probe's JSON parsing tolerance, the per-verb dispatch including
    the deferred-stub-replacement (no `intentionally not
    implemented` exit-2 anywhere), the audited write-verb shape,
    and the status-integration count line + JSON field. Unit tests
    in each verb module pin the protocol parsing
    (`docker compose ls`, `docker compose ps` ndjson + array forms,
    `Status` count parser, etc.).
  - **Out of scope.** Compose verbs ship without `inspect bundle`
    integration (a v0.1.5+ design topic) and without per-service
    `compose up <ns>/<p>/<svc>` (compose itself supports targeted
    `up <svc>`, but the audit-tag taxonomy needs another pass before
    it lands in inspect's contract).

- **L6 — Per-branch rollback tracking in bundle matrix steps + new
  `inspect bundle status <id>` verb.** v0.1.2 retrospective:
  `parallel: true` + `matrix:` steps rolled back the WHOLE matrix
  when any one branch failed, including the succeeded branches whose
  downstream effects may already have been used. Worse, the rollback
  body was rendered with an EMPTY matrix map, so any
  `{{ matrix.<key> }}` reference in a `rollback:` block silently
  expanded to an empty string. v0.1.3 fixes both: branches succeed
  or fail independently and rollback inverts only the succeeded ones
  with per-branch matrix interpolation.
  - **Per-branch ledger.** New `BranchResult { branch_id, status,
    matrix_value, matrix_key }` struct in `src/bundle/exec.rs`.
    Each parallel-matrix step populates a `Vec<BranchResult>`;
    succeeded branches mark `Ok`, the failing branch marks
    `Failed`, branches that the stop-on-first-error policy
    short-circuited mark `Skipped`. Branches are sorted by
    `branch_id` so the rollback walk and post-mortem queries see
    a deterministic order regardless of worker scheduling.
  - **Branch-aware rollback.** `do_rollback` now consults the
    per-step `step_branches` map. For matrix steps it iterates
    ONLY the succeeded branches, building a per-branch matrix
    map (`{matrix_key → matrix_value}`) and feeding it through
    `interpolate(rb_cmd, &bundle.vars, &mtx)`. The v0.1.2 bug
    where `{{ matrix.<key> }}` in a rollback block expanded to
    nothing is fixed end-to-end — guarded by
    `l6_matrix_rollback_template_resolves_per_succeeded_branch`.
    Failed/skipped branches log an audit note via the new
    `bundle.rollback.skip` verb (with the branch label and a
    why-skipped explanation) so post-mortem queries can see why
    no inverse fired.
  - **Audit schema.** Two new optional `AuditEntry` fields:
    `bundle_branch: Option<String>` (format
    `<matrix-key>=<value>`, e.g. `volume=atlas_milvus`) stamped
    on every per-branch entry from a matrix step;
    `bundle_branch_status: Option<String>` (`"ok"` | `"failed"` |
    `"skipped"`) recorded in lockstep. Both `Option<T>` with
    `skip_serializing_if`, so pre-L6 entries deserialize
    unchanged. The pre-existing `is_revert: bool` is now also
    set on `bundle.rollback` audit entries (which previously
    elided it) so revert queries return the full inversion
    history.
  - **`BranchFailureCarrier` thread-local sidecar.** When a
    `parallel` matrix step fails partway, `run_parallel_matrix`
    threads the partial branch ledger back to the apply loop via
    a thread-local cell keyed by the call site. This avoids
    changing `Result<T, E>`'s shape (the alternative was widening
    every error-bearing helper). The apply loop drains the
    sidecar at the failure handler and stashes the branches in
    `step_branches[idx]` before dispatching to `do_rollback`.
  - **New `inspect bundle status <bundle_id>` verb.** Prefix-
    matches the bundle id against the audit log, walks every
    entry tagged with that id, groups by `bundle_step`, and
    renders the per-branch outcome table:
    ```
    bundle status: id=...  2 step(s), 6 audit entries
      step `tar-volumes` (matrix):
        ✓ volume=atlas_milvus  (12300ms)
        ✓ volume=atlas_etcd    (4100ms)
        ✗ volume=aware_milvus  (1500ms)
    ```
    Markers: ✓ (ok forward), ✗ (failed), · (skipped), ↶ (ok
    revert/rollback). `--json` emits a `{bundle_id,
    entries_total, steps[{step, kind, branches[{branch, status,
    audit_id, verb, exit, duration_ms, is_revert}]}]}` envelope
    for agent consumption. Ambiguous prefix exits 2 with the
    full match list; missing prefix exits 1 with a chained hint.
  - **Composes with `on_failure: rollback_to: <id>`.** The
    existing checkpoint semantics are preserved: when rolling
    back TO a checkpoint that *includes* a fully-succeeded
    matrix step, all branches stay in place (rollback target
    is "everything after this checkpoint"); when rolling back
    INTO a partially-failed matrix step itself, only succeeded
    branches are inverted. Regression-guarded by
    `l6_full_matrix_success_then_later_step_fails_rolls_back_all_branches`.
  - **6 acceptance tests in
    `tests/phase_f_v013.rs::l6_*`**: matrix-failure-rollback
    (4-branch fanout, branch c fails, only a/b inverted, c/d
    skip-audited), template resolves per-succeeded-branch (the
    headline regression guard), bundle-status human-form (per-
    branch ✓ markers + matrix labelling), bundle-status json
    schema, full-matrix-success-then-later-step-fails (regression
    guard for the existing checkpoint path), bundle-status-help
    (subcommand discoverability), bundle-status-unknown-id
    (no-match exit + chained hint).

- **F18 — Per-namespace, per-day human-readable session transcripts
  at `~/.inspect/history/<ns>-<YYYY-MM-DD>.log` (mode 0600), plus
  `inspect history show / list / clear / rotate` and the
  `[history]` config block in `~/.inspect/config.toml`.**
  Migration-operator field feedback: *"After a 4-hour migration I
  had to scroll my chat history to find which exact `inspect run`
  command did what. An `~/.inspect/history/arte-2026-04-28.log`
  (rotating, all in/out) would let me audit and post-mortem
  migrations cleanly. **High value for compliance / audit trails of
  destructive ops.**"* The structured audit log already answered
  "what verbs ran with what arguments + what changed"; F18 ships a
  complementary human-readable transcript that captures the full
  input + output of every namespace-scoped verb invocation as the
  operator saw it on the terminal.
  - **Fenced-block format.** Each verb invocation produces one
    `── <ts> <ns> #<token> ──...` header → argv line → buffered
    stdout/stderr → `── exit=N duration=Mms audit_id=<id> ──`
    footer block. The fence pattern is `awk '/^── /,/^── exit=/'`
    friendly so block extraction stays trivial without a
    parser; the trailing `audit_id=` cross-links back to the
    structured audit entry for forensic round-trip.
  - **Captured surface.** The transcript reflects what the
    operator's terminal showed: the `Renderer::print()`,
    `JsonOut::write`, `OutputDoc::print_*`, and `format::render`
    paths plus every streaming verb's per-line emit (run / logs /
    cat / grep / find / merged / steps / watch / status / health /
    cache / why / ports / connectivity / network / correlation /
    cursor / dispatch / transfer / ls / ps / images / volumes —
    swept via two new `tee_println!` / `tee_eprintln!` macros).
    `error::emit` tees to stderr too, so failure cases are
    captured. F14 script-mode verb output is captured; F15 file
    transfer verb output is captured; F16 streamed lines are
    captured byte-for-byte; F17 multi-step output is captured
    block-by-block per (step, target). The argv line itself runs
    through a small additional masker for `--password=` /
    `--token=` flags before being recorded.
  - **L7 redaction passthrough.** Every line tee'd to the
    transcript runs through the existing four-masker pipeline
    (PEM / Authorization / URL credentials / KEY=VALUE) using a
    fresh `OutputRedactor` per line; `None` returns from the PEM
    masker (interior of an active PEM block) suppress the line in
    the transcript too — secret bytes never reach the transcript
    file. Per-namespace `[namespaces.<ns>.history].redact = "off"`
    disables redaction in the transcript only (file mode 0600
    already restricts exposure); `--show-secrets` on the
    originating verb bypasses redaction in both stdout and
    transcript (single flag, single bypass — same contract as
    L7).
  - **Per-namespace disable.**
    `[namespaces.<ns>.history].disabled = true` in
    `~/.inspect/servers.toml` skips transcript writes for that
    namespace entirely. The audit log is still written — F18 and
    F11 are independent contracts.
  - **`inspect history` subcommand tree** — `show [<ns>] [--date
    YYYY-MM-DD] [--grep <pattern>] [--audit-id <id>]` renders
    fenced blocks (transparently decompresses `.log.gz` on read);
    `list [<ns>]` walks the history dir grouping by (namespace,
    date) with byte sizes; `clear <ns> --before YYYY-MM-DD` deletes
    files older than the cutoff for one namespace (gated by
    `--yes`); `rotate` applies the retention policy now. All four
    have `--json` variants for agent consumption.
  - **`[history]` config block in `~/.inspect/config.toml`.**
    Three knobs with the spec-mandated defaults: `retain_days = 90`
    (older files deleted on rotate), `max_total_mb = 500` (cap
    across all namespaces; oldest-first eviction; today's file is
    never evicted), `compress_after_days = 7` (older files gzipped
    in place — `<ns>-<YYYY-MM-DD>.log` becomes `.log.gz` so the
    date stays parseable). Atomic compress via `<name>.part` →
    `rename(2)` so a crash mid-encode doesn't leave a half-written
    `.gz` next to the original.
  - **Lazy rotation trigger.** A once-per-day marker
    (`~/.inspect/history/.rotated`) gates the lazy rotate fire
    from `transcript::finalize` — a busy session does not pay an
    FS scan per verb. The 23-hour stale window allows one fire
    per UTC day even with mild clock skew. Errors from the lazy
    path are swallowed so a transient rotation failure cannot
    break the just-emitted transcript block.
  - **Performance.** Output is accumulated in memory during the
    verb and written in one shot at finalize. **One `fdatasync(2)`
    per verb invocation.** A 10-minute streaming verb that
    produces 100 MB of output produces exactly 1 fsync against
    the transcript file — satisfies the F18 ≤ 70-fsyncs-per-10-min
    performance gate by orders of magnitude. Buffer is capped at
    16 MiB; overflow is replaced with a `[transcript truncated:
    buffer cap reached]` marker so a runaway verb cannot OOM.
  - **Scope contract.** "Every verb invocation **against a
    namespace**" gets a transcript. Operator-tooling verbs that do
    not resolve a namespace (`inspect help`, `inspect list`,
    `inspect audit ls`, `inspect history ...` itself) produce no
    transcript file — they would always be near-empty and would
    pollute the history dir. The hook fires from
    `verbs::runtime::resolve_target` for every dispatch verb that
    crosses it; verbs that handle namespaces directly (e.g.
    `inspect cache clear <ns>`) call
    `transcript::set_namespace(ns)` explicitly.
  - **Module structure.** New `src/transcript.rs` (~430 LOC) holds
    the per-process `TranscriptContext` (stored in
    `OnceLock<Mutex<Option<...>>>`) plus `init`, `set_namespace`,
    `set_audit_id`, `tee_stdout`, `tee_stderr`, `emit_stdout`,
    `finalize`, the L7 redaction passthrough, the `tee_println!` /
    `tee_eprintln!` macros, and 5 unit tests. Submodule
    `src/transcript/rotate.rs` (~330 LOC) holds `HistoryPolicy`,
    `RotateReport`, `run_rotate`, `maybe_run_lazy`,
    `parse_transcript_name`, `read_transcript`, plus 5 unit tests.
    New `src/commands/history.rs` (~400 LOC) holds the four
    subcommand dispatchers and a fenced-block parser with 3 unit
    tests. `GlobalConfig` extended with `HistoryConfig` (3 fields:
    `retain_days`, `max_total_mb`, `compress_after_days`);
    `NamespaceConfig` extended with optional `history:
    HistoryNsOverride { disabled, redact }`. New `flate2` dep
    (default features = pure-Rust miniz_oxide backend).
  - **AuditStore::append** now also calls
    `transcript::set_audit_id(&entry.id)` (first-write-wins) so
    the transcript footer cross-links to the umbrella audit
    entry on multi-audit verbs (F17 `--steps` parent).
  - **Help-text discoverability.** New `LONG_HISTORY` constant on
    `inspect history --help` documents the GC + RETENTION knobs,
    `[history]` config, per-ns overrides, redaction integration,
    and 6 worked examples; `inspect help safety` already linked.
  - **10 acceptance tests in `tests/phase_f_v013.rs::f18_*`** plus
    5 unit tests in `transcript::tests`, 5 in
    `transcript::rotate::tests`, and 3 in
    `commands::history::tests`: status-writes-block; help-no-global;
    rotate deletes old files (retain_days = 7 against a 6-file
    fixture); rotate compresses + show decompresses (round-trip);
    per-ns disabled = no file but audit still writes; show
    --audit-id cross-references; list emits structured records per
    file; clear requires --yes then deletes; help documents all
    subcommands; argv `--password=` is masked in the recorded argv
    line. Full suite green (28 suites; 201 phase_f tests).

- **L5 — `inspect audit gc` retention + orphan-snapshot sweep, plus
  optional lazy GC trigger via `[audit] retention` in
  `~/.inspect/config.toml`.** v0.1.2 retrospective: `~/.inspect/audit/`
  grew unbounded; a team running 50 bundle ops per day would
  accumulate years of JSONL plus orphaned snapshot directories with no
  cleanup verb and no retention policy. After F11/F14/F15/F16/F17 each
  added new audit fields and (in F17's case) per-(step, target) and
  per-revert entries that multiply the per-mutation footprint by an
  order of magnitude, the maintenance gap had to close before the
  next field rollout.
  - **New `inspect audit gc --keep <X>` subcommand.** `<X>` accepts
    duration suffixes `90d` / `4w` / `12h` / `15m` (days, weeks,
    hours, minutes) or a bare integer (newest N entries kept *per
    namespace*, where namespace is parsed from the entry's
    `selector` field — `arte/atlas-vault` → `arte`; entries with no
    namespace prefix group under the sentinel `_`). `--dry-run`
    previews counts and freed bytes without touching the
    filesystem; the same code path computes the deletion set so
    the dry-run report is byte-for-byte what an `--apply` run
    would produce. `--keep 0` is rejected loudly with a chained
    hint — refusing to silently delete every entry is the only
    safe default.
  - **Stable `--json` envelope.** Fields: `dry_run`, `policy`
    (canonical formatted form: `90d`, `4w`, `12h`, `15m`, or the
    integer count), `entries_total`, `entries_kept`,
    `deleted_entries`, `deleted_snapshots`, `freed_bytes`,
    `deleted_ids` (full list, not truncated), and
    `deleted_snapshot_hashes` (without the `sha256-` prefix). The
    envelope sits at top level (no `data` wrapper) and includes
    the standard `_source: "audit.gc"` / `_medium: "audit"` /
    `server: "local"` discriminators so a fleet of GCs can be
    streamed through one log pipeline.
  - **Snapshot orphan sweep.** Walks
    `~/.inspect/audit/snapshots/sha256-<hex>` and deletes any file
    whose hash is not referenced by a *retained* audit entry.
    "Referenced" means: `previous_hash`, `new_hash`, the file-name
    portion of `snapshot`, and — critically for v0.1.3 — the
    `revert.payload` of a `state_snapshot` revert AND nested
    `state_snapshot` payloads inside a `composite` revert (F17's
    parent `run --steps` entries store per-step inverses as a
    JSON array; the GC must recurse into that array or it would
    treat F17 step snapshots as orphans). A snapshot pinned by
    any retained entry is **never** deleted — that is the F11
    revert contract and the GC enforces it as the only invariant
    that cannot be relaxed by config. Pre-existing acceptance test
    `l5_gc_keeps_snapshot_referenced_by_retained_entry` is the
    headline regression guard.
  - **Atomic JSONL rewrite.** Each affected JSONL file is
    rewritten via `tmp.gctmp.<pid>` → `rename(2)` (mode 0600 from
    the start on unix). A file whose entries are *all* deleted is
    `unlink(2)`-ed entirely — the GC never leaves zero-byte
    JSONLs lying around. `freed_bytes` covers BOTH JSONL
    shrinkage AND snapshot file sizes, so an operator can size
    their next retention window from the report alone.
  - **`[audit] retention` config block.** New
    `~/.inspect/config.toml` with `[audit] retention = "90d"`
    (or any value `--keep` accepts) opts an installation in to
    automatic lazy GC. The trigger fires on every successful
    `AuditStore::append` — i.e. every write verb that produced an
    audit record — guarded by a once-per-minute cheap-path
    marker (`~/.inspect/audit/.gc-checked`). Within the cheap
    path: only the oldest JSONL file's mtime is checked against
    the retention threshold; if it is fresher, the GC no-ops
    immediately (no full FS scan). Count-based policies always
    run a full pass per check window since mtime alone cannot
    decide them. Errors from lazy GC are deliberately swallowed
    so a transient GC failure can never break the just-appended
    audit record (the `let _ = ...maybe_run_lazy_gc()` call site
    in `AuditStore::append`).
  - **`~/.inspect/config.toml` is a fresh global-policy file.**
    Distinct from `servers.toml` (per-namespace runtime config) —
    L5 is the first item to land it; future cross-cutting
    behavior toggles (cache TTLs, history rotation) can plug in
    here without polluting per-server schema. Missing file is
    not an error: it deserializes to `GlobalConfig::default()`
    and lazy GC stays off.
  - **Help-text discoverability.** `inspect audit --help` gains
    a `GC + RETENTION (L5, v0.1.3)` section listing `--keep`
    syntax, the `[audit] retention` config hook, and the
    cheap-path-marker semantics. `inspect audit gc --help`
    documents `--keep`, `--dry-run`, and the full `FormatArgs`
    flags.
  - **11 acceptance tests in `tests/phase_f_v013.rs::l5_*`** plus
    9 unit tests in `safety::gc::tests` and 3 in
    `config::global::tests`: dry-run-doesn't-delete, apply
    deletes old entries + orphan snapshots, retained-entry
    snapshots are NEVER deleted (the F11 contract guard), JSON
    envelope schema, count-policy keeps newest per namespace,
    invalid `--keep` value chains a hint, `--keep 0` rejected,
    `--help` documents the contract surface, empty audit dir
    yields zero counts, lazy GC marker prevents double-scan
    within a minute, lazy GC fires on next audit append when the
    oldest file's mtime crosses the threshold (uses
    `std::fs::File::set_modified` to backdate the seed file).

- **F17 — `inspect run --steps <file.json>` multi-step runner with
  per-step exit codes, structured per-step output, F11 composite
  revert, and `--revert-on-failure` auto-unwind (migration-operator
  field feedback: *"When I run a 5-step heredoc, all 5 steps are
  one 'run' with one exit code. If step 3 fails I see it in the
  output but the SUMMARY still says `1 ok` because the **outer
  ssh** succeeded. That made me build defensive `set +e; … || echo
  MARKER` patterns. A `--steps` mode that took an array of commands
  and returned per-step exit codes would be amazing for migration
  scripts."*).** Promotes the defensive `set +e; … || echo MARKER`
  pattern from a widespread workaround to a first-class verb mode
  with **structured per-step output that an LLM-driven wrapper can
  reason about step-by-step**. Without F17, agentic callers cannot
  reliably build "run these N steps, stop on first failure, give
  me the per-step result table" workflows on top of `inspect run`
  — the outer `bash -c`'s exit code masks per-step failures, and
  the SUMMARY trailer says `1 ok` even when step 3 of 5 failed.
  - **`--steps <PATH>` flag on `inspect run`.** PATH is a JSON
    manifest file (or `-` for stdin). Each manifest step has
    `name` (required, unique), `cmd` (required unless `cmd_file`
    is set), `cmd_file` (alternative — local script path shipped
    via `bash -s`; F14 composition), `on_failure` (`"stop"`
    default | `"continue"`), optional `timeout_s` (per-step
    wall-clock cap, default 8 hours), optional `revert_cmd`
    (declared inverse for the F11 composite revert; absent ⇒
    `revert.kind = "unsupported"` for that step).
  - **Per-step structured output.** Human format: one
    `STEP <name> ▶` / `STEP <name> ◀ exit=N duration=Ms` block
    per step, with the existing `<ns> | …` line-prefixing inside
    each block, then a STEPS summary table with ✓/✗/⏱/· markers
    and a count line `STEPS: N total, K ok, M failed, S skipped`.
    JSON format (`--json`): one structured object containing
    `steps: [{name, cmd, exit, duration_ms, stdout, stderr,
    status: "ok"|"failed"|"skipped"|"timeout", audit_id}]` plus
    `summary: {total, ok, failed, skipped, stopped_at,
    auto_revert_count}` plus `manifest_sha256` + `steps_run_id`
    + `verb: "run.steps"`. **This is the contract LLM-driven
    wrappers can reason about** — no prose parsing, no defensive
    markers.
  - **`AuditEntry` gains four F17 fields.** `steps_run_id:
    Option<String>` links every per-step entry plus the parent
    composite entry (same UUID-shaped id, in `<ms>-<4hex>`
    format matching the rest of the audit log); `step_name:
    Option<String>` is stamped on per-step entries only;
    `manifest_sha256: Option<String>` is the canonical
    sha256 of the JSON manifest body, stamped on the parent only;
    `manifest_steps: Option<Vec<String>>` is the ordered name
    list, stamped on the parent only. All `Option<T>` with
    `skip_serializing_if` so pre-F17 entries deserialize
    unchanged. Plus a new `Revert::composite(payload_json,
    preview)` constructor since F11's composite variant was
    declared but had no constructor; `RevertKind::Composite`
    payload shape is now nailed down: a JSON-encoded ordered
    list of `{step_name, kind, payload}` records executed in
    reverse order.
  - **F11 composite revert (`inspect revert <steps_run_id>`).**
    `src/commands/revert.rs` gains `revert_composite()` (replaces
    the v0.1.3-as-of-F11 "not yet implemented" stub) — walks the
    parent's composite payload list in reverse manifest order,
    dispatching each `command_pair` inverse against the same
    selector as the original `--steps` invocation and writing one
    `run.step.revert` audit entry per inverse, all linked back to
    the parent via `reverts: <parent-id>`. Items with `kind:
    "unsupported"` (steps with no declared `revert_cmd`) are
    skipped without aborting the unwind. Dry-run by default;
    `--apply` executes; the dry-run preview lists the inverses in
    reverse-manifest order so the operator sees exactly what
    `--apply` will run.
  - **`--revert-on-failure` flag (requires `--steps`).** When a
    step fails with `on_failure: "stop"`, the runner walks the
    inverses of the steps that already ran in reverse manifest
    order **in the same invocation** and dispatches each as its
    own audit-logged auto-revert entry stamped with
    `auto_revert_of: <original-step-id>`. Exactly the
    migration-operator's missing primitive: a 5-step manifest
    where step 3 fails with `--revert-on-failure` correctly
    unwinds steps 1 and 2 without a separate `inspect revert`
    invocation.
  - **F14 composition via `cmd_file`.** A step can declare
    `"cmd_file": "./step3.sh"` instead of `"cmd": "..."`; the
    runner reads the local file, ships its body via `bash -s`,
    and stamps `script_sha256` + `script_bytes` + `script_path`
    on the per-step audit entry — the same fields F14 stamps for
    `inspect run --file`. Lets multi-step migrations whose
    individual steps are non-trivial scripts compose F14 + F17
    cleanly without the operator hand-rolling a wrapper.
  - **F12 / F13 composition.** Each step inherits the namespace
    env overlay (F12) automatically — overlays validated once
    before the per-step loop so a typo short-circuits the whole
    run. Stale-socket failures during a step trigger F13's
    auto-reauth path identically to bare `inspect run` (the
    composite payload is only walked after every step completes,
    so a transparent reauth mid-pipeline is invisible at the F17
    layer).
  - **Multi-target fanout (sequential).** F17 originally scoped
    single-target only; expanded mid-implementation to multi-target
    sequential after the audit-link semantics were worked out
    (per-(step, target) entries share `steps_run_id`, parent
    `run.steps` entry references the manifest hash). See the
    "Multi-target fanout" sub-bullet below for the shipped shape.
    Parallel-within-step lands in L13 in this release (own entry
    at the top of v0.1.3 Added).
  - **YAML input (`--steps-yaml <PATH>`).** Same manifest schema
    as `--steps`, just YAML-encoded — convenient for operators
    who maintain migration manifests alongside CI/CD pipelines.
    Mutex with `--steps`. Both flags participate in a clap
    `manifest_source` ArgGroup so `--revert-on-failure` accepts
    either spelling.
  - **`--steps --stream` per-step PTY allocation.** Forwards
    `args.stream` into each per-(step, target) `RunOpts` via the
    F16 `with_tty(true)` builder. The remote process for each
    step line-buffers (real-time output instead of 4 KB bursts)
    and Ctrl-C propagates through the PTY layer to the active
    step's remote process group. Per-step audit entries record
    `streamed: true`; the parent `run.steps` entry records
    `streamed: true` once for the whole pipeline so post-hoc
    audit can tell streaming-mode pipelines apart from buffered
    ones.
  - **Multi-target fanout.** When the selector resolves to N>1
    targets, each manifest step fans out across all N targets
    sequentially within the step. Per-step aggregate `status` is
    `ok` only when every target succeeded; `failed` when any
    target's exit was non-zero; `timeout` when any target overran
    `timeout_s`. `on_failure: "stop"` applies globally — any
    target's failure aborts the next manifest step on every
    target. Each (step, target) pair writes its own `run.step`
    audit entry with the target's label as the entry's
    `selector`; `--revert-on-failure` fans the inverse out
    across every target the step ran on. The JSON output's per-
    step record carries a `targets[]` array of per-target results
    (`label`, `exit`, `duration_ms`, `stdout`, `stderr`,
    `output_truncated`, `status`, `audit_id`, `retried`); the
    summary's new `target_count` field exposes N. Multi-target is
    sequential within each step (parallel fan-out is intentionally
    out of scope for v0.1.3 — output interleaving + audit-link-
    ordering races would need a separate design pass).
  - **F13 stale-session auto-reauth per (step, target).** Each
    per-target dispatch is wrapped in `dispatch_with_reauth`, so
    a stale-socket failure mid-pipeline triggers the F13
    transparent reauth + retry path on that exact (step, target)
    pair without aborting the rest of the pipeline. Retried
    entries stamp `retry_of` + `reauth_id` for cross-correlation
    with the inserted `connect.reauth` entry; the per-target
    result's `retried: true` flag surfaces in the JSON output.
  - **Per-step output cap (10 MiB per (step, target)).** Live
    printing is unaffected (the operator always sees every line);
    the captured copy that feeds the audit + JSON output stops
    growing past 10 MiB and stamps `output_truncated: true` on
    the per-target result. Protects the local process from OOM
    on a step that emits many GB. Cap matches the F9 `--stdin-max`
    default for consistency.
  - **`--reason` plumb-through.** `inspect run --steps --reason
    "JIRA-1234 atlas vault migration"` echoes the reason to
    stderr (matches bare `inspect run` semantics) **and** stamps
    it onto the parent `run.steps` audit entry's `reason` field
    so a 4-hour migration's operator intent is recoverable from
    the audit log alone, no terminal scrollback required.
  - **Parallel-within-step (sequenced after F17).** F17 ships
    sequential per-target; parallel-within-step is L13 in this
    release — output interleaving handled by a per-line writer
    mutex, audit-link ordering preserved by stamping `target_idx`
    on per-(step, target) entries when the step runs in parallel.
    See L13 entry at the top of v0.1.3 Added for the full design.
  - **Test coverage.** 23 acceptance tests in
    `tests/phase_f_v013.rs` (`f17_*`) covering: 3-step
    stop-on-failure produces the correct STEPS table + audit
    shape with `steps_run_id` linkage; `on_failure=continue` runs
    every step; `--json` output matches the documented schema
    (per-step records with `targets[]` array + summary +
    `manifest_sha256` + `target_count`); `--revert-on-failure`
    walks inverses in reverse manifest order with `auto_revert_of`
    linkage; unsupported steps are skipped during auto-revert
    rather than aborting; `--steps` + `--file` clap mutex,
    `--steps` + `--steps-yaml` mutex; `--revert-on-failure`
    requires either manifest source via the `manifest_source`
    ArgGroup (accepts `--steps-yaml` too); `inspect revert
    <parent-id>` walks the composite payload in reverse (dry-run
    preview ordering verified); `cmd_file` F14 composition stamps
    `script_sha256` on the per-step entry; YAML manifests parse
    and dispatch identically; `--steps --stream` records
    `streamed: true` on every per-step + parent entry;
    `--reason` is echoed to stderr AND stamped on the parent
    audit; per-step output cap leaves small payloads
    untruncated; per-step `timeout_s` is accepted and reflected
    in audit; F13 mid-pipeline reauth fires on a stale-socket
    failure between steps and the pipeline continues; multi-
    target fanout writes one entry per (step, target) all sharing
    `steps_run_id`; multi-target step status aggregates to
    `failed` when any target fails; multi-target
    `--revert-on-failure` unwinds per-target; help-text gate
    enforces that `--steps`, `--steps-yaml`, and
    `--revert-on-failure` appear in `inspect run --help`;
    invalid manifest exits 2 with a JSON-mentioning error.
    Plus 8 unit tests in `src/verbs/steps.rs::tests` for the
    manifest parser + validator (JSON + YAML round-trip, empty
    manifest, duplicate names, neither cmd nor cmd_file, both
    cmd and cmd_file, revert_cmd round-trip, timeout round-trip).

- **F16 — `inspect run --stream` / `--follow` line-streaming for
  long-running remote commands (migration-operator field feedback:
  *"`docker compose up -d --force-recreate aware aware-embedder
  2>&1 | tail -30` is fine, but I'd kill for `inspect run --stream
  arte -- 'docker logs -f aware'` that line-streams back to the
  client until I Ctrl-C. I worked around with `inspect logs
  arte/<service>` (which exists) but it's a separate command shape
  and doesn't compose with arbitrary scripts."*).** Adds a
  `--stream` flag (alias `--follow`) on `inspect run` for the long
  tail of non-logs commands that produce output indefinitely until
  SIGINT (`tail -f /var/log/syslog`, `journalctl -fu vault`,
  `python -m monitor`, etc.). Pre-F16, every such command either
  buffered until exit (silent until the operator gave up and
  Ctrl-C'd the local `inspect`, which often killed the process
  locally without notifying the remote) or worked only by accident
  if the remote happened to flush eagerly. F16 wires the existing
  line-streaming SSH executor (already used by `inspect logs
  --follow`) into the bare `inspect run` path, plus the SSH PTY
  trick that makes the remote process line-buffer and propagates
  Ctrl-C end-to-end.
  - **`--stream` / `--follow` flag on `inspect run`.** Forces SSH
    PTY allocation (`ssh -tt`) on the dispatch. Two effects flow
    from the PTY: (1) the remote process flips from block-buffered
    to line-buffered output, so lines arrive locally in real time
    instead of in 4 KB bursts; (2) local Ctrl-C propagates through
    the PTY layer to the remote process, so the command actually
    dies instead of being orphaned on the remote host. The
    `<ns> | …` line-prefixing is unchanged from the existing
    streaming path (`inspect logs --follow` already used the same
    `run_streaming` path); the only new behavior is the `-tt`
    flip and the audit-field stamp.
  - **Default timeout bumped to 8 hours under `--stream`.**
    Streaming runs are expected to terminate via Ctrl-C, not by
    reaching the per-target timeout. The non-streaming default
    stays at 120 s; both can be overridden with
    `--timeout-secs <N>`. The 8 h matches the existing
    `inspect logs --follow` default so operators do not have to
    learn two timeout regimes.
  - **`RunOpts.tty: bool` builder hook in
    `src/ssh/exec.rs`.** New field on the executor's per-call
    options struct, threaded through all three SSH dispatch paths
    (`run_remote`, `run_remote_streaming`,
    `run_remote_streaming_capturing`). Off by default for
    non-streaming runs because PTY allocation can change command
    behaviour (CRLF endings, color output, prompt suppression);
    `--stream` is the only call site that flips it today. Builder
    style: `RunOpts::with_timeout(secs).with_tty(true)`.
  - **`AuditEntry.streamed: bool` field (audit-schema, behavior
    flag).** New field, `Option<T>`-shaped via
    `skip_serializing_if = "is_false"` so pre-F16 entries
    deserialize unchanged. Stamped `true` on every `--stream` /
    `--follow` invocation and absent otherwise. Recorded so
    post-hoc audit can tell `tail -f`-shaped invocations apart
    from short-lived commands in the same audit log without
    parsing the args text — the same separation the F15
    `transfer_direction` field provides for uploads vs downloads.
  - **`inspect run` is now audited on every `--stream`
    invocation.** Pre-F16 the verb was un-audited unless stdin
    was forwarded (F9) or the dispatch wrapper retried under F13.
    `--stream` joins those triggers: every streaming run produces
    an audit entry with `streamed: true`, `failure_class`, the
    rendered command, and the wall-clock duration, so a
    multi-hour migration's `--stream` blocks are recoverable from
    the audit log alone.
  - **Mutex with `--stdin-script`** (clap-enforced). Streaming a
    script body over local stdin while also streaming output back
    is a half-duplex protocol headache deferred to v0.1.5;
    `--stream --file <script>` is fine (the body is delivered in
    one shot, then the running script's output streams back).
    `--stream --stdin-script` exits 2 with a clean clap message
    naming both flags.
  - **`inspect logs` interop (no overlap).** F16 does **not**
    replace `inspect logs --follow`; the dedicated logs verb keeps
    its existing semantics (selector-aware, source-tier-aware,
    structured). F16 is for the case when the operator wants
    streaming for a non-logs command. The `inspect logs`
    discoverability hint added in F10.6 remains the canonical
    pointer for log tailing.
  - **First-Ctrl-C → SIGINT-via-PTY, second-Ctrl-C-within-1s →
    channel-close-SIGHUP escalation (added late in v0.1.3).**
    The streaming SSH executor now writes the ASCII INTR byte
    (`\x03`) into the SSH stdin pipe on the first Ctrl-C — the
    remote PTY's terminal driver sees ETX and delivers SIGINT to
    the remote process group, which is the OpenSSH-idiomatic way
    to forward SIGINT through a PTY (no reliance on the unreliable
    SSH `signal` channel-request protocol message). The local
    cancel flag is then cleared so the verb surfaces the remote's
    real exit code (matches the field-validation gate's
    "Ctrl-C terminates `docker logs -f`, exit code is the
    docker-logs exit code"). On the second Ctrl-C within 1
    second, the executor escalates to channel close (`child.kill()`
    on the local SSH process), which triggers the remote sshd to
    deliver SIGHUP to the remote process group via PTY teardown
    — covering the corner case of a remote process that ignores
    SIGINT but exits on SIGHUP. Counter-based detection in
    `src/exec/cancel.rs` (new `signal_count()` + `reset_cancel_flag()`
    APIs) so a third signal cannot be lost between polls.

- **F16 follow-up — SIGHUP escalation in the streaming SSH
  executor.** First-Ctrl-C → SIGINT-via-PTY (write `\x03` into
  the remote PTY's terminal driver, which delivers SIGINT to the
  remote process group); second-Ctrl-C-within-1s → channel close
  (kill the local SSH child, which triggers sshd to deliver
  SIGHUP to the remote process group via PTY teardown). New
  signal-count API in `src/exec/cancel.rs` (`signal_count()`,
  `reset_cancel_flag()`) so the streaming executor can detect a
  *new* signal between polls without losing the trip. The local
  SSH child's stdin is now `Stdio::piped()` (instead of
  `Stdio::null()`) when `opts.tty` is set so we have a write
  handle for the `\x03`. Test coverage: 2 new unit tests in
  `src/exec/cancel.rs::tests` (`signal_count_increments_per_cancel`,
  `reset_cancel_flag_clears_only_the_flag`); the real-SSH SIGHUP
  escalation behaviour is exercised by the field-validation gate
  (a remote command that ignores SIGINT but exits on SIGHUP
  receives SIGHUP via channel close on the second Ctrl-C within
  1 second).
  - **Test coverage.** 6 acceptance tests in
    `tests/phase_f_v013.rs` (`f16_*`) covering: `--stream`
    records `streamed: true` on success, `--follow` alias is
    accepted and produces the same audit shape, non-streaming
    runs omit the `streamed` field entirely (the F9 audit path
    is exercised to prove this), `--stream` on a failing command
    still records `streamed: true` + `failure_class:
    "command_failed"`, `--stream --stdin-script` is clap-rejected
    with both flag names in the error, and `inspect run --help`
    documents `--stream`, the `--follow` alias, and the PTY
    (`-tt`) mechanism. Real-SSH SIGINT propagation and the
    line-by-line *timing* of the flush are exercised by the
    field-validation gate (the migration-operator's destructive-
    migration smoke test) since the in-process mock medium
    cannot model PTY semantics.

- **F15 — `inspect put` / `inspect get` / `inspect cp` native file
  transfer over the persistent ControlPath master (migration-operator
  field feedback: *"I had to context-switch between
  `inspect run arte -- 'cat > /tmp/x.sh'` (works for tiny payloads,
  breaks on quoting) vs `scp /tmp/x ...:arte:...` (works but bypasses
  the inspect connection). For a tool whose job is 'be the way I
  touch this server,' not having `inspect put`/`inspect get` is a
  notable hole. Especially during compose-file rewrites."*).**
  Replaces the v0.1.2 base64-in-argv `cp` implementation (4 MiB
  cap, audit-thin) with a streaming-stdin pipeline that has no
  fixed size cap, captures pre-existing remote content as
  `revert.kind = state_snapshot` (or a delete inverse for
  brand-new files), and records direction / bytes / sha256 in the
  audit log on every transfer. Rides the same multiplexed SSH
  master used by every other namespace verb, so it inherits F11
  revert capture, F12 env overlay, F13 stale-session auto-reauth.
  - **`inspect put <local> <selector>:<path>`** — uploads via
    `cat > <tmp>; chmod/chown --reference; mv <tmp> <path>` with
    the local body streamed through SSH stdin (F9). Container
    targets dispatch via `docker exec -i <ctr> sh -c '...'`.
    Captures the prior remote file content as `revert.kind =
    state_snapshot` so `inspect revert <id>` restores byte-for-byte;
    when the target did not exist, captures a `command_pair` rm
    inverse so revert deletes the freshly-created file.
  - **`inspect get <selector>:<path> <local>`** — downloads via
    `base64 -- <path>` (binary-safe over the SSH text pipe) and
    decodes locally. `<local>` of `-` writes to stdout for piping.
    `inspect get` is read-only on the remote, so `revert.kind` is
    `unsupported` (the operator deletes the local file to undo);
    the audit entry still records `transfer_bytes` +
    `transfer_sha256` so a later `put` of the same content is
    verifiable byte-for-byte from the audit log.
  - **`inspect cp <source> <dest>`** — bidirectional convenience.
    Inspects arg shape and routes to `put` (push) or `get` (pull)
    based on which side carries the `<selector>:<path>`. Operator
    types `cp`, audit records the canonical verb (`put` or `get`).
    The pre-F15 base64-in-argv backend is removed; the 4 MiB hard
    cap is gone with it.
  - **Selector forms.** Both verbs accept the existing selector
    grammar: `<ns>/_:/path` (host filesystem; F7.2 shorthand
    `<ns>:/path` resolves to host), and `<ns>/<svc>:/path`
    (container filesystem, dispatched via `docker exec -i`).
    Host vs container is decided unambiguously by the selector,
    never by a flag.
  - **Flags on `put` / `cp`.** `--mode <octal>` chmod the remote
    after upload; `--owner <user[:group]>` chown the remote after
    upload (requires the SSH user have permission); `--mkdir-p`
    create missing parent dirs on the remote before writing
    (without this, a missing parent surfaces as `error: remote
    parent directory does not exist` and the transfer aborts).
    Operator overrides land *after* the mode/owner mirror from
    the prior file, so the override always wins.
  - **Audit shape.** `AuditEntry` gains five F15 fields
    (`transfer_direction: "up"|"down"`, `transfer_local`,
    `transfer_remote`, `transfer_bytes`, `transfer_sha256`), all
    `Option<T>` with `skip_serializing_if` so pre-F15 entries
    deserialize unchanged. The existing `previous_hash` /
    `new_hash` / `snapshot` / `diff_summary` fields continue to
    record what they did for v0.1.2 cp, so audit-driven revert
    keeps working for every entry.
  - **Hint surface.** `inspect run`'s F9 stdin-cap hint and
    F14's `--file` size-cap hint now point at `inspect put`
    (the canonical name) for bulk transfer instead of the
    pre-F15 `inspect cp`. `inspect exec --apply` still references
    `cp` in its "structured-write-verb" hint, now interpreted as
    the bidirectional alias to `put` / `get`.
  - **Out of scope for v0.1.3 (deferred to v0.1.5).**
    `--since <duration>` / `--max-bytes <size>` on `get`
    (log-retrieval ergonomics; the dedicated `inspect logs`
    verb already covers that domain), `--resume` for partial
    transfers (chunked-protocol design pass).
  - **Test coverage.** 12 acceptance tests in
    `tests/phase_f_v013.rs` (`f15_*`) covering: host-fs upload
    via stdin with audit fields recorded, container-fs upload
    via `docker exec -i`, state_snapshot revert when target
    exists, command_pair (rm) revert when target does not
    exist, dry-run no-dispatch, `--mode` override path,
    `--mkdir-p` parent creation path, host-fs download via
    `base64 --` decode, `-` local writes to stdout, `get`
    audit records `transfer_direction = down` +
    `revert.kind = unsupported`, `cp` regression dispatching to
    `put` / `get` by arg shape (audit verb canonicalised).
    Plus 8 unit tests in `src/verbs/transfer.rs::tests` for the
    atomic-write script builder and the `looks_remote`
    selector-vs-local-path detector.

- **L7 — Header / PEM / URL credential redaction in stdout
  (v0.1.2 retrospective: agent workflows pipe remote stdout into
  LLM context windows; the existing P4 line-oriented `KEY=VALUE`
  masker missed the three common shapes — `Authorization: Bearer
  …` headers in `curl -v`, PEM private-key blocks in
  `cat /etc/ssl/private/*.pem`, and credentials embedded in URLs
  like `postgres://user:pass@host/db` — so a single `inspect run`
  could leak a live token into a prompt).** The single-file
  `src/redact.rs` is replaced with a four-masker pipeline under
  `src/redact/` that runs on every line emitted by `inspect run`,
  `inspect exec`, `inspect logs`, `inspect cat`, `inspect grep`,
  `inspect search`, `inspect why`, `inspect find`, and the merged
  follow stream.
  - **PEM masker (`src/redact/pem.rs`).** Multi-line state
    machine. Recognized BEGIN forms: `PRIVATE KEY` (PKCS#8),
    `ENCRYPTED PRIVATE KEY` (PKCS#8 enc), `RSA PRIVATE KEY`
    (PKCS#1), `EC PRIVATE KEY` (SEC1), `DSA PRIVATE KEY`,
    `OPENSSH PRIVATE KEY`, and `PGP PRIVATE KEY BLOCK`. The
    BEGIN line emits a single `[REDACTED PEM KEY]` marker;
    every interior line plus the matching END line is suppressed
    by the composer. Public certificates
    (`-----BEGIN CERTIFICATE-----`) and public keys
    (`-----BEGIN PUBLIC KEY-----`) pass through unchanged.
  - **Header masker (`src/redact/header.rs`).** Case-insensitive
    word-bounded match on `Authorization`, `X-API-Key`, `Cookie`,
    `Set-Cookie` followed by `:`. Replaces the entire value
    portion with `<redacted>` so a `Cookie:` value containing
    its own URL credential is also covered. Word boundary on the
    name prevents false positives on prose like `MyAuthorization`.
  - **URL credential masker (`src/redact/url.rs`).** Masks the
    password portion of `scheme://user:pass@host` patterns to
    `user:****@host`, preserving scheme, username, and host so
    the diagnostic is still readable. Covers `postgres`, `mysql`,
    `redis`, `mongodb`, `mongodb+srv`, `amqp`, `http`, `https`,
    and any other scheme matching the userinfo grammar. Lines
    without the pattern are zero-allocation pass-through.
  - **Env masker (`src/redact/env.rs`).** The pre-existing P4
    line-oriented `KEY=VALUE` masker, preserved verbatim — same
    `head4****tail2` partial-mask shape, same suffix list, same
    `SECRETS_REDACT_ALL` opt-in, same audit-args `[secrets_masked]`
    text tag. Existing tests pass unchanged.
  - **Composer ordering.** Maskers run PEM → Header → URL → Env
    on every line. Inside a PEM block, the gate suppresses the
    other three so an interior line that happens to look like a
    header or URL credential is replaced by the single marker
    rather than partially leaked. Header and URL compose on a
    single line (a `Cookie:` value containing a URL credential is
    masked once by the header masker; the URL masker has nothing
    to do).
  - **`--show-secrets` bypass.** A single boolean on every read
    verb already wired in v0.1.2 now bypasses **all four** maskers
    in one place (`OutputRedactor::mask_line` short-circuits at
    the top), so the existing operator opt-in shape is unchanged
    for end users.
  - **Audit linkage.** `AuditEntry` gains
    `secrets_masked_kinds: Option<Vec<String>>` recording the
    deterministic ordered list of masker kinds that fired for a
    given step (`["pem", "header", "url", "env"]` — subset, in
    canonical order). The text tag `[secrets_masked=true]` on
    `inspect run` / `inspect exec` audit-args is preserved; the
    new field lets post-hoc reviewers tell two redacted runs
    apart by *which* pattern almost leaked. Pre-L7 entries omit
    the field via `skip_serializing_if`.
  - **API.** New `OutputRedactor::new(show_secrets, redact_all)`
    constructor; `mask_line(&str) -> Option<Cow<str>>` (returns
    `None` for suppressed PEM-interior lines); `was_active()`
    and `active_kinds()` for audit stamping. Public constants
    `REDACTED = "<redacted>"` and `PEM_REDACTED_MARKER`. One
    redactor is constructed per remote step so PEM gate state
    cannot leak across SSH dispatches.
  - **Performance.** All regexes compiled once via
    `once_cell::sync::Lazy`. Lines that match no masker return
    `Cow::Borrowed` with no allocation (verified by the
    `l7_redactor_unit_no_alloc_for_clean_lines` test).
  - **Test coverage.** 20 acceptance tests in
    `tests/phase_f_v013.rs` (`l7_*`) covering: PEM block
    collapse-to-marker on `cat` / `logs`, PGP private-key block,
    PKCS#8 unencrypted block, certificate pass-through,
    `Authorization` header on `curl -v` output via `grep` and
    `run`, `Set-Cookie` and `X-API-Key` case-insensitivity, URL
    credentials in path / `DB_URL` env var (double-masked by URL
    + env), URL-credentials-inside-prose on `find`,
    `--show-secrets` bypass on `cat`, `logs`, and `run` for all
    four patterns, audit-args `kinds=…` recording the canonical
    ordered subset, `secrets_masked_kinds` field absent when no
    masker fires, env-masker behavior unchanged from P4, the
    PEM-gate-suppresses-other-maskers contract, and the
    no-allocation pass-through invariant.

- **F14 — `inspect run --file <script>` / `--stdin-script` script
  mode (field feedback: *"the biggest individual time-sink was
  nested quoting. Every layer (your shell → ssh → bash →
  docker exec → psql/python -c) needs its own escape pass… I'd pay
  almost any feature in tradeoffs to never quote-escape across
  layers again."*).** Reads the entire bash payload from a file or
  stdin and ships it as the remote command body via `bash -s`
  (or the interpreter declared in the script's shebang) so nested
  `psql -c "..."`, `python -c '...'`, and `cypher-shell <<CYPHER`
  heredocs reach the remote interpreter byte-for-byte without
  any local escape pass.
  - **`--file <path>`.** Reads the script body from the local
    filesystem; rejects directories and missing paths with a
    chained recovery hint. Honors the F9 `--stdin-max` cap
    (default 10 MiB; raise with `--stdin-max <SIZE>`, set to `0`
    to disable, or use `inspect cp` for bulk transfer + remote
    execution).
  - **`--stdin-script`.** Reads the script body from local stdin
    (heredoc form: `inspect run arte --stdin-script <<'BASH' …
    BASH`). Mutually exclusive with `--file`, `--no-stdin`, and
    a tty stdin (clap-rejected or runtime-rejected with chained
    `--file`-pointing hint).
  - **Args after `--` become positional.** `inspect run <ns>
    --file s.sh -- alpha beta` runs `bash -s -- alpha beta` on
    the remote so the script's `$1` / `$2` are `alpha` / `beta`.
    Args are POSIX-shell-quoted before crossing the SSH boundary;
    the script body itself is never re-quoted.
  - **Shebang-driven interpreter dispatch.** A leading
    `#!/usr/bin/env <interp>` or `#!/path/to/<interp>` line
    selects the remote interpreter (`bash` / `sh` / `zsh` / `ksh`
    / `dash` use `-s`; `python3` / `node` / `ruby` / etc. use
    POSIX `-`). Without a shebang, defaults to `bash -s`.
    Interpreter names are sanitized to `[A-Za-z0-9_.-]`; anything
    else falls back to `bash` (defense against malformed or
    hostile shebangs).
  - **Container-targeted dispatch.** Selectors that resolve to a
    container render as `docker exec -i <ctr> <interp> -s …` so
    the script body flows in via the docker-exec stdin pipe.
  - **Audit shape.** Every script-mode invocation writes a
    per-step audit entry with `script_path`, `script_sha256`,
    `script_bytes`, and `script_interp`. With
    `--audit-script-body`, the full body is inlined under
    `script_body`; without it, the body is dedup-stored once at
    `~/.inspect/scripts/<sha256>.sh` (mode 0600 inside the 0700
    home) so audit reconstruction works even after the operator
    deletes the local file.
  - **Composes with F9 / F12 / F13.** Script mode dispatches
    through the same SSH executor as bare `inspect run`, so the
    namespace env overlay (F12) is applied, stale-session
    auto-reauth (F13) fires identically, and the size cap shares
    F9's `--stdin-max` byte budget.
  - **`AuditEntry` schema.** Five new optional fields
    (`script_path`, `script_sha256`, `script_bytes`,
    `script_body`, `script_interp`). Pre-F14 entries omit the
    fields via `skip_serializing_if`.
  - **Test coverage.** 14 acceptance tests in
    `tests/phase_f_v013.rs` (`f14_*`) covering: byte-for-byte
    heredoc fidelity, `--file` ↔ `--stdin-script` equivalence,
    mutual-exclusion clap rejections, positional-args dispatch,
    audit shape (path / sha / bytes / interp / inline body /
    dedup store), shebang-driven `python3 -` dispatch,
    container-targeted `docker exec -i` form, missing-path /
    directory / size-cap / empty-stdin rejection, and the
    "no `--` required for script mode" regression guard.

### Added (prior in v0.1.3)

- **F13 — Stale-session auto-reauth + distinct transport exit class
  (field feedback: the primary operator's most painful pattern was
  a stale `ControlPersist` socket returning `exit 255` minutes into
  a multi-host migration with stderr indistinguishable from a real
  command failure — every wrapper script's
  `if inspect run …; then` branch lit up the wrong way, requiring
  manual `inspect disconnect && inspect connect` and a from-scratch
  retry).** `inspect run` and `inspect exec` now classify every
  dispatch failure into one of three transport buckets and, by
  default, re-auth + retry once on stale sessions transparently.
  - **New exit codes.** `12` = `transport_stale`,
    `13` = `transport_unreachable`, `14` = `transport_auth_failed`.
    These three never collide with `ExitKind::Inner` (clamped to
    `1..=125`) so wrappers can branch on
    `case $? in 12) …;; 13) …;; 14) …; esac` reliably. Mixed
    multi-target failures still collapse to the existing
    `ExitKind::Error`.
  - **Auto-reauth.** When a step's stderr classifies as
    `transport_stale` (OpenSSH's `Connection closed`,
    `Control socket … unavailable`, `master process … exited`,
    or `mux_client_request_session: session request failed`),
    the dispatch wrapper writes a one-line
    `note: persistent session for <ns> expired —
    re-authenticating…` notice to stderr, records a
    `connect.reauth` audit entry, calls the same code path as
    interactive `inspect connect <ns>`, and re-runs the original
    step exactly once. A failed reauth escalates to
    `transport_auth_failed` (exit 14) with a chained
    `inspect connect <ns>` recovery hint.
  - **SUMMARY trailer.** When every failed step shares the same
    transport class, the human-format summary appends
    `(ssh_error: <class> — <recovery hint>)` after
    `run: N ok, M failed`.
  - **JSON contract.** Every `run` / `exec` invocation now emits
    a final `phase=summary` envelope with `ok`, `failed`, and
    `failure_class ∈ {ok, command_failed, transport_stale,
    transport_unreachable, transport_auth_failed,
    transport_mixed}`. Streaming line envelopes earlier in the
    run remain unchanged.
  - **Audit linkage.** `AuditEntry` gains three optional fields
    (`retry_of`, `reauth_id`, `failure_class`). The
    `connect.reauth` entry's id is stamped onto the post-retry
    audit entry's `reauth_id` so a downstream consumer can
    trivially correlate the failed attempt and its retry across
    the audit log.
  - **Opt-out.** Per-invocation `--no-reauth` on `run` / `exec`
    surfaces stale failures as exit 12 instead of retrying.
    Per-namespace `auto_reauth = false` in
    `~/.inspect/servers.toml` does the same persistently.
  - **Implementation.** New module `src/ssh/transport.rs` houses
    the pure classifier and `summary_hint` strings; new
    `src/exec/dispatch.rs` houses the reauth-aware dispatch
    wrapper that both `verbs/run.rs` and `verbs/write/exec.rs`
    flow through. The `RemoteRunner` trait gains a `reauth`
    method; `LiveRunner::reauth` delegates to a new
    `commands::connect::reauth_namespace` helper that tears
    down the dead master socket and re-runs `ssh::start_master`
    with the same `AuthSelection` as interactive connect.

- **F12 — Per-namespace remote env overlay (field feedback: the
  primary operator repeatedly hit
  `bash: cargo: command not found` /
  `LANG: cannot set locale` after `inspect connect arte` because
  the SSH non-login shell PATH on the target jumped over
  `~/.cargo/bin` and locale was unset, forcing a per-session
  `export PATH=…` ritual before every `inspect run`).** Each
  namespace can now persist a small environment overlay that is
  prefixed onto every remote command issued by `inspect run` and
  `inspect exec` for that namespace.
  - **Config.** New optional `[servers.<ns>.env]` table in
    `~/.inspect/servers.toml` (string→string map). Keys are
    POSIX-validated (`[A-Za-z_][A-Za-z0-9_]*`); invalid keys
    fail the dispatch boundary in `verbs/runtime::resolve_target`
    so every verb path catches them, not just `connect`.
  - **Dispatch.** `NsCtx` now carries an `env_overlay: BTreeMap`
    populated from `resolved.config.env`. The overlay is rendered
    deterministically as
    `env KEY1="VAL1" KEY2="VAL2" -- <cmd>` and prepended to the
    remote command before quoting/transport. Values are
    double-quoted so `$VAR` still expands on the remote, but
    `;`/`&`/`|` stay literal; `"`, `\`, and backtick are
    escaped.
  - **Per-call overrides.** `inspect run` and `inspect exec` gain
    `--env KEY=VAL` (repeatable; user wins on collision),
    `--env-clear` (drop the namespace overlay for this call only),
    and `--debug` (prints the rendered command to stderr before
    transport — useful when you don't yet trust the overlay).
  - **`inspect connect` overlay management.**
    - `--show` — print the current overlay (`PATH:` line +
      `ENV:` block; exits 0 when none).
    - `--set-path <p>` — pin remote PATH for this namespace.
    - `--set-env KEY=VAL` (repeatable) and
      `--unset-env KEY` (repeatable) — atomic 0600 round-trip
      through `write_atomic_0600`. The `[env]` table is dropped
      from the TOML when the last entry is removed, so the file
      stays tidy.
    - `--detect-path` — opens an SSH probe, compares login vs
      non-login PATH, and offers to pin the login PATH when it
      adds entries the non-login shell is missing. Prompts on a
      tty; auto-declines (with a one-line note) when stdin is not
      a tty so CI runs are deterministic.
  - **Audit.** `AuditEntry` gains `env_overlay` and `rendered_cmd`
    so the JSONL log captures exactly what shipped to the remote.
  - **Tests.** 18 acceptance tests in `phase_f_v013.rs` cover:
    overlay applied to `run` and `exec`, audit fields recorded,
    `--env-clear` clears, `--env` user-wins-collision merge,
    no-overlay path stays clean (no `env --` prefix), `--debug`
    stderr, semicolon stays literal in values, invalid keys
    rejected at config and CLI boundaries, `connect --show`
    output, `--set-path` idempotency, `--set-env`/`--unset-env`
    round-trip, last-entry drops the `[env]` table, invalid
    `KEY=VAL` rejected, unknown namespace rejected,
    `--show`/`--set-env` mutual exclusion at clap.

- **F10 — 4th-user polish bundle (seven first-hour friction points
  surfaced by a fresh operator on a partly-discovered namespace).
  All seven sub-items shipped.**
  - **F10.1 — Namespace-flag-as-typo chained hint.** Operators with
    `kubectl -n <ns>` muscle memory commonly type
    `inspect why atlas-neo4j --on arte` (also `--in`/`--at`/
    `--host`/`--ns`/`--namespace`). The pre-clap detector in
    `main.rs` now emits
    `error: --on is not a flag — selectors are <ns>/<service>. Did
    you mean 'inspect why arte/atlas-neo4j'?` and exits 2.
    Conservative shape detection scoped to known selector-taking
    verbs.
  - **F10.2 — `inspect cat --lines L-R` server-side line slice.**
    Inclusive 1-based range with `--start`/`--end` synonym
    (mutually exclusive). JSON path emits structured `{n, line}`
    envelopes per kept line so agents get line numbers without
    parsing prose.
  - **F10.3 — `why <ns>/<container>` chained hint when the token
    is a running container but not a registered service.** Catches
    `SelectorError::NoMatches` / `EmptyProfile`, looks the literal
    token up against the runtime inventory, and on a hit emits a
    three-line chained hint (`inspect logs … / inspect run …
    docker inspect / inspect setup --force`). Exit 0
    (informational). Genuine typos still exit 2 unchanged.
  - **F10.4 — `inspect grep` / `inspect search` MODEL/EXAMPLE/NOTE
    help preface.** Both `--help` outputs now declare their model
    (grep shells out to remote `grep -r`; search runs a LogQL
    query against the profile-side index) and cross-reference
    each other so the right verb can be picked from `--help`
    alone.
  - **F10.5 — `--quiet` pipeline regression test.** F7.4's
    `status --quiet` (no envelope trailers) and
    `logs --tail N --quiet | wc -l == N` contracts are now
    explicit acceptance tests in `phase_f_v013.rs`.
  - **F10.6 — `inspect logs` on the top-level `--help` index.**
    `LONG_ABOUT` reorganized into COMMON / DIAGNOSTIC + READ /
    WRITE + LIFECYCLE blocks with
    `inspect logs arte/atlas-vault --since 5m --match 'panic'` as
    a worked example.
  - **F10.7 — `inspect run --clean-output` (alias `--no-tty`).**
    Strips ANSI CSI/OSC escape sequences from captured output and
    prepends `TERM=dumb` to the remote command's env. Mutually
    exclusive with `--tty`. ANSI strip is allocation-free when no
    ESC byte is present.

### Added (prior in v0.1.3)

- **F7 — Selector + output ergonomic papercuts (field feedback:
  five separate small papercuts collected over a v0.1.2 destructive
  migration session that each cost an extra round-trip to the docs
  or to `--help` to recover from). Five of six sub-items shipped;
  one (F7.6 `inspect connect` publickey ordering) is deferred to
  v0.1.5 because the interactive ssh path is not amenable to a
  reliable, mock-driven contract test.**
  - **F7.1 — Empty-profile hint redirects to `inspect setup`, not
    `inspect profile`.** When a service-specific selector
    (`inspect why arte/atlas-vault`, `inspect logs arte/onyx-vault`)
    targets a registered namespace whose cached profile contains
    zero services, the diagnostic now leads with
    `hint: run 'inspect setup <ns>' to discover services on this
    namespace` instead of the generic refresh-the-cache hint. The
    pre-existing `inspect profile / inspect setup --force` hint
    is preserved for the genuine "selector typo against a
    populated profile" case (`SelectorError::NoMatches`).
  - **F7.2 — `arte:/path` shorthand is no longer rejected by the
    selector parser.** The shape `arte:/etc/hostname` (sugar for
    `arte/_:/etc/hostname` — host-level read against the
    namespace's SSH host) used to surface as
    `invalid selector character ':' ...` because the parser
    tokenized the absolute path's leading slash as the
    server/service separator. `parse_selector` now detects the
    shorthand by checking whether the first colon (outside any
    regex `/.../`) precedes the first slash; if so, the colon
    is the service/path separator and the implicit `_` host
    service is supplied automatically.
  - **F7.3 — `inspect ports` server-side port filters:
    `--port <n>` and `--port-range <lo-hi>`.** Filter each
    output line through a token-aware port matcher (handles
    both `0.0.0.0:8200` and `8200/tcp` shapes); the SUMMARY's
    listener count reflects the filtered total. Flags are
    mutually exclusive at the clap level.
  - **F7.4 — Global `--quiet` flag suppresses the SUMMARY/NEXT
    envelope on the Human path.** Pipeline-friendly: data rows
    are emitted without the leading two-space indent prefix so
    output is safe to feed directly into `grep`, `awk`, `tail`,
    `head`, `wc -l`. Mutually exclusive with `--json` /
    `--jsonl`. Wired through `OutputDoc::with_quiet` and
    `Renderer::quiet` so every read verb inherits the contract.
  - **F7.5 — `inspect status` empty-state phrasing + explicit
    `state` JSON field.** Three states distinguished instead of
    folding into `0 service(s): 0 healthy, 0 unhealthy, 0 unknown`:
    `state: "ok"` (≥1 service classified), `"no_services_matched"`
    (non-empty inventory but zero profile entries; SUMMARY phrases
    as a config condition and NEXT chains
    `inspect ps <ns>` + `inspect setup <ns> --force`),
    `"empty_inventory"` (host clean / docker down). A small
    `dispatch::plan` change ensures a wildcard selector
    (`arte/*`) against an empty profile still binds the
    namespace's `NsCtx` so the verb can still call `docker ps`.
  - **F7.7 — `inspect status --json` carries the new `state`
    field.** Already supported via the standard `FormatArgs`
    flatten.

- **F4 — `inspect why` deep-diagnostic bundle (field feedback: the
  primary operator's multi-hour Vault triage where `why` said
  "unhealthy <- likely root cause" and stopped, forcing a hand-rolled
  walk through `logs`, `inspect`, and `ports` to learn that the
  entrypoint wrapper was injecting `-dev-listen-address=0.0.0.0:8200`
  on top of a config-declared listener; the same port bound twice).**
  When the target service of `inspect why <selector>` is unhealthy or
  down, three artifacts are now attached inline under `DATA:`:
  - **`logs:`** — the recent log tail (default 20 lines, configurable
    via `--log-tail <N>`, hard-capped at 200 with a one-line stderr
    notice — protects redaction + transport).
  - **`effective_command:`** — the container's effective `Entrypoint`
    + `Cmd` from `docker inspect`, plus a `wrapper injects:` line
    when the entrypoint script contains a flag-injection pattern
    (`-dev-listen-address=`, `-listen-address=`, `-bind-address=`,
    `-api-addr=`, `--listen-address=`).
  - **`port_reality:`** — per-port table cross-referencing
    `PortBindings` + `ExposedPorts` from `docker inspect`,
    entrypoint-injected listeners, and host listener state from
    `ss -ltn` (or `netstat -ltn` fallback). Ports declared by both
    config *and* a wrapper-injected flag are flagged
    `container: bound (twice!)` — the headline reproducer pattern.
  Hard-capped at **≤4 extra remote commands per service per bundle
  invocation** (`docker logs --tail`, one combined `docker inspect`,
  `docker exec ... cat /docker-entrypoint.sh`, `ss -ltn`). All four
  fail independently — partial bundles still surface what worked.
  New flags: `--no-bundle` (suppress; restores the v0.1.2 terse
  output) and `--log-tail <N>` (default 20, capped at 200).
  Smart `NEXT:` hints derived from the bundle: a "bound (twice!)"
  port pushes `inspect run <ns>/<svc> -- 'cat /docker-entrypoint.sh'`,
  and `address already in use` in the logs pushes `inspect ports
  <ns>`. JSON adds three fields to each per-service object —
  `recent_logs[]`, `effective_command{entrypoint, cmd, wrapper_injects}`,
  `port_reality[{port, host, container, declared_by}]` — always
  present (empty arrays / `null` on healthy services so agents don't
  need optional-chaining gymnastics). Healthy services are
  byte-for-byte unchanged: no extra round-trips, no bundle headers.
  Eight acceptance tests in `tests/phase_f_v013.rs` covering
  unhealthy/healthy paths, `--no-bundle`, `--log-tail` clamp, JSON
  schema stability, wrapper-injection detection, and smart-NEXT
  emission.

- **F5 — Container-name vs compose-service-name uniform resolution
  (field feedback: 2nd v0.1.2 user typed `arte/luminary-onyx-onyx-vault-1`
  — the docker name from `docker ps` — got "no targets", then
  `arte/onyx-vault` — the compose service name — worked; both forms
  appeared in the discovered inventory).** Selector resolver in
  `src/selector/resolve.rs::resolve_services_for_ns` now accepts the
  docker `container_name` as an exact-match synonym for the canonical
  compose service `name`, and globs match against either axis. The
  resolved target is always the canonical name so every downstream
  verb (`logs`, `run`, `restart`, …) addresses the same profile row
  regardless of which form the operator typed. When the typed form
  was the docker container name, `inspect` emits a one-line
  breadcrumb on stderr — `note: '<typed>' is the docker container
  name; the canonical selector is '<ns>/<canonical>'` — so the
  operator learns the canonical form. Hint is suppressible via
  `INSPECT_NO_CANONICAL_HINT=1` for strict-stderr JSON consumers.
  `inspect status --json` now exposes a stable `aliases: [<docker
  container name>]` field on every service row (empty array when the
  canonical name already matches the docker name) so agents can
  enumerate equivalences without trial-and-error. 6 acceptance tests
  in `tests/phase_f_v013.rs` cover both selector forms, the
  canonical-form breadcrumb, the silent-when-no-distinct-alias case,
  the JSON `aliases` field shape (present + populated, present +
  empty), and globs across either axis.

- **F3 — `inspect help <command>` is now a true synonym for
  `inspect <command> --help` (carry-over field-feedback ergonomic gap;
  every operator coming from `git help log` / `cargo help build` /
  `kubectl help get` hit it on first try).** The argv rewrite happens
  in `src/main.rs::rewrite_help_synonym` *before* clap parses,
  guaranteeing the rendered output is **byte-for-byte identical** to
  the `--help` form (no drift on `Usage:` line, no missing
  `-V, --version` row). Editorial topics keep precedence: `inspect
  help search` still renders the curated LogQL DSL guide, and
  `inspect search --help` still shows clap's flag list — the new
  rewrite only fires when the token is a verb *and not* a topic.
  `tests/phase_f_v013.rs::f3_help_verb_byte_for_byte_matches_dash_dash_help`
  pins the contract for 22 representative top-level verbs (read /
  write / lifecycle / discovery / safety / diagnostic).
- Unknown `inspect help <foo>` now exits **2** (Error) with `error:
  unknown command or topic: '<foo>'` and the canonical `see: inspect
  help examples` chained hint. Pre-F3 it was exit 1 + `unknown help
  topic` — the change aligns with `git`/`cargo`/`kubectl` and reflects
  that `<foo>` could be either a verb *or* a topic. The
  `ERROR_CATALOG` `UnknownHelpTopic` row was updated in lockstep so
  the see-line still attaches automatically.

- **F2 — `docker inspect` timeout warning noise eliminated (field-feedback
  regression: two of two v0.1.2 first-time users hit a spurious `warning:`
  on every healthy `inspect setup` against hosts with 30+ containers).**
  The probe now classifies its outcome into one of four buckets — `Clean`,
  `SlowButSuccessful`, `PartialTimeout`, `GenuineFailure` — and routes
  each to the right channel:
  - **Clean** (every container inspected): silent. Healthy hosts no
    longer emit a single `warning:` line on first setup.
  - **SlowButSuccessful** (batch slow, fallback rescued every container):
    debug-level only. Visible under `INSPECT_DEBUG=1` or `RUST_LOG=debug`.
  - **PartialTimeout** (`N` of `M` per-container probes failed): one
    summary line — `warning: docker inspect timed out for N/M containers;
    rerun with --force or check daemon load`. Replaces the old
    one-warning-per-failed-container noise.
  - **GenuineFailure** (zero containers inspected, daemon down): probe
    escalates `ProbeResult.fatal`; engine returns `Err`; setup exits
    non-zero with a chained hint (`inspect run … 'sudo systemctl status
    docker'` → `inspect setup --force`).

  The batched timeout is now scaled with inventory size:
  `timeout = max(10s, 250ms * container_count)` capped at 60s. Operators
  on pathological daemons can pin a fixed budget via
  `INSPECT_DOCKER_INSPECT_TIMEOUT=<seconds>` (verbatim, not re-clipped).

  Three-bucket classification + scaling formula are documented in
  `docs/RUNBOOK.md` §8 ("Probe author checklist") so the next probe
  author follows the same rule. 8 unit tests in `src/discovery/probes.rs`
  pin every contract: empty host, field-scale 37/37 = `Clean`, slow-but-
  successful = no warning, 0/N = `GenuineFailure` with chained hint, 3/50
  = `PartialTimeout` summary line, override bypasses scaling, formula
  floor / scaling / cap.

- **F11 — Universal pre-staged `--revert` on every write verb
  (load-bearing for agentic safety; non-negotiable before v0.2.0
  freezes the audit schema).** Every write verb now captures its
  inverse *before* dispatching the mutation. Each `AuditEntry`
  carries a new `revert: { kind, payload, captured_at, preview }`
  block (plus `applied`, `no_revert_acknowledged`, and
  `auto_revert_of` fields), with four kinds:
  - `command_pair` — single inverse remote command (e.g. `chmod
    0644 …`, `docker start <ctr>`); used by lifecycle stop/start,
    `chmod`, `chown`, `mkdir`, `touch`.
  - `state_snapshot` — restore from snapshot store; used by `cp`,
    `edit`, `rm` (rm now snapshots the target file before deleting
    it).
  - `composite` — ordered list of inverses; reserved for bundle
    integration in F17.
  - `unsupported` — verb has no general inverse this invocation
    (`restart`, `reload`, `exec --no-revert`, legacy v0.1.2 entries
    on read).

  **`inspect exec --apply` now refuses without `--no-revert`** with
  a chained hint pointing at `cp` / `chmod` / `restart`, since
  free-form shell payloads are not generally invertible. Operators
  acknowledge the trade-off explicitly; the audit entry records
  `no_revert_acknowledged: true` so post-hoc readers can tell
  free-form mutations apart from mutations that simply pre-date the
  contract.

  **`inspect revert` upgraded** to dispatch on `revert.kind`:
  `command_pair` runs the captured inverse remote command;
  `state_snapshot` follows the existing snapshot-restore path;
  `unsupported` exits 2 with a chained explanation. New
  `--last [N]` walks the N most recent applied entries in reverse
  chronological order. The `audit-id` argument is now optional (use
  with `--last`). Dry-run output gains a `REVERT:` block showing
  the captured `preview` and the inverse command.

  **`--revert-preview` flag** on every write verb prints the
  captured inverse to stderr before applying, so operators (and
  driving agents) can see exactly what `inspect revert <new-id>`
  will undo.

  **Backward compatibility:** v0.1.2 audit entries (which lack the
  `revert` field) are read as `kind: unsupported` and refuse with a
  loud chained hint pointing at `inspect audit show` rather than
  silently no-opping. Snapshot-style legacy entries (with
  `previous_hash`) still revert through the existing path.

  8 phase_f_v013 acceptance tests cover all four kinds, the
  `--no-revert` refusal, `--revert-preview`, `--last`, and the
  legacy-entry refusal.

- **F9 — `inspect run` forwards local stdin to the remote command.**
  3rd field user (BUG-3 follow-up): `inspect run arte 'docker exec
  -i atlas-pg sh' < ./init.sql` returned `SUMMARY: run: 1 ok, 0
  failed` and exit 0, but no SQL ran — the script's stdin never
  reached the remote `sh`. **Behavior change:** when `inspect run`'s
  own stdin is non-tty (piped or redirected from a file), it is now
  forwarded byte-for-byte to the remote command's stdin and closed
  on EOF, matching native `ssh host cmd <stdin>` semantics. When
  local stdin is a tty, behavior is unchanged from v0.1.2 — no
  forwarding, no hang.

  New flags on `inspect run`:
  - `--no-stdin` refuses to forward; if local stdin has data waiting,
    exits 2 BEFORE dispatching the remote command (never silently
    discards input). With an empty pipe (`< /dev/null`,
    `true | inspect run …`), the run proceeds normally.
  - `--stdin-max <SIZE>` overrides the default 10 MiB cap (`k`/`m`/`g`
    suffixes; `0` disables). Above the cap, exits 2 with a chained
    hint pointing at `inspect cp` for bulk transfer.
  - `--audit-stdin-hash` records `stdin_sha256` (hex SHA-256 of the
    forwarded payload) in the audit entry. Off by default for perf;
    opt-in for security-sensitive runs.

  Audit-log additions: every `inspect run` invocation that forwards
  stdin now writes a one-line audit entry with `verb=run`,
  `stdin_bytes=<N>`, and (with `--audit-stdin-hash`)
  `stdin_sha256=<hex>`. Without forwarded stdin, `inspect run`
  remains un-audited (matches v0.1.2 read-verb behavior). The audit
  schema gains optional `stdin_bytes` (skip-on-zero) and
  `stdin_sha256` (skip-on-none) fields.

  Tests: 8 new acceptance tests in `tests/phase_f_v013.rs` covering
  the field reproducer (byte-for-byte forwarding through a mock
  with `echo_stdin: true`), `--no-stdin` loud-failure with
  pre-dispatch exit, size cap with chained hint, `--stdin-max 0`
  disables the cap, no-piped-input regression guard, `stdin_bytes`
  audit field, `stdin_sha256` audit field, and `--no-stdin` with
  empty pipe being a silent pass.

- **F8 — Cache freshness, runtime-tier cache, `SOURCE:` provenance,
  `inspect cache` verb.** Three field reports converged on the same
  failure mode: `inspect status` happily served pre-mutation data
  for an unbounded window, with no way to ask for fresh data and no
  way to even tell the data was cached. v0.1.3 introduces a tiered
  cache:
  - **inventory tier** (existing): `~/.inspect/profiles/<ns>.yaml`,
    refreshed by `inspect setup`.
  - **runtime tier** (new): `~/.inspect/cache/<ns>/runtime.json`,
    populated by every read verb on a cache miss. Default TTL 10s
    via `INSPECT_RUNTIME_TTL_SECS` (`0` disables the cache, `never`
    sets infinite TTL).
  Every read verb (`status`, `health`, `why`) now:
  - prints a leading `SOURCE: <live|cached|stale> Ns ago …` line in
    human/table/markdown output (omitted for machine formats so
    JSON/CSV/TSV grammar stays clean);
  - carries a stable `meta.source` field on its JSON envelope with
    `mode`, `runtime_age_s`, `inventory_age_s`, `stale`, `reason`;
  - accepts `--refresh` (alias `--live`) to force a live fetch;
  - serves cached data with `mode=stale` and a stderr warning when
    a refresh fails on top of an existing cache, plus a
    `inspect connectivity <ns>` chained hint.
  Mutation verbs (`restart`, `stop`, `start`, `reload`, and
  `bundle apply`) automatically invalidate the runtime cache for
  every namespace they touched, so the next read is guaranteed
  live. `inspect cache show` lists every cached namespace with
  runtime age, inventory age, staleness, refresh count, and
  on-disk size; `inspect cache clear [<ns> | --all]` deletes
  cached snapshots and writes an audit entry per cleared
  namespace. Concurrent refreshers are serialized via a per-
  namespace `flock(2)` advisory lock so two parallel `status`
  calls don't double-fetch. Hot-path correctness is pinned by
  reproducer tests in `tests/phase_f_v013.rs` (cache hit issues
  zero remote commands; refresh count is monotonic; post-mutation
  reads are live; bundle apply invalidates).

### Fixed

- **F1 — `inspect status <ns>` returns 0 services on healthy hosts
  (regression).** Bare-namespace selectors (`inspect status arte`,
  `inspect status prod-*`) were resolving to a single host-level
  step and the status loop, which only renders service steps,
  dropped the host fall-through silently, yielding a misleading
  `0 service(s): 0 healthy, 0 unhealthy, 0 unknown` summary on a
  healthy 30+-container host. Status now rewrites a service-less
  selector to its all-services form (`<sel>/*`) before resolution,
  so `inspect status arte` and `inspect status arte/*` produce the
  same fan-out. Aliases (`@name`) and explicit selectors (with `/`)
  pass through unchanged. Independently confirmed by 2nd and 3rd
  field users; ship-blocker for v0.1.3 under the "one critical
  issue" rule. Regression guards in `tests/phase_f_v013.rs` cover
  1, 10, and multi-namespace cases plus parity with the explicit
  `arte/*` form.

## [0.1.2] — v0.1.2 backlog (B1-B10)

Closes the v0.1.2 backlog (`INSPECT_v0.1.2_BACKLOG.md`). Two new
top-level verbs (`watch`, `bundle`), six refinements to existing
verbs, and a tightened audit schema.

### Added

- **B9 — `inspect bundle plan|apply <file.yaml>`.** YAML-driven
  multi-step orchestration. A bundle declares preflight checks, an
  ordered list of steps (`exec`/`run`/`watch`), per-step rollback
  actions, an optional bundle-level rollback block, and postflight
  checks. Step `on_failure:` routes failures via `abort` (default),
  `continue`, `rollback`, or `rollback_to: <id>`. Steps may use
  `parallel: true` + `matrix:` to fan out across N values with
  bounded concurrency (cap 8). Templating: `{{ vars.x.y }}` and
  `{{ matrix.k }}` interpolate into any string field. Every `exec`
  step (and rollback action) writes one audit entry tagged with a
  bundle-correlation `bundle_id` and the step's `bundle_step`. Run
  `inspect bundle plan` for a no-touch dry-run; `inspect bundle apply
  --apply` to execute. CI mode: `--no-prompt` skips the rollback
  confirmation prompt. First-class checks: `disk_free`,
  `docker_running`, `services_healthy`, `http_ok`, `sql_returns`, plus
  an `exec` escape hatch.

- **B10 — `inspect watch <selector> --until-…`.** Single-target
  block-until-condition verb. Four predicate kinds, mutually
  exclusive: `--until-cmd`, `--until-log`, `--until-sql`,
  `--until-http`. `--until-cmd` accepts comparators `--equals`,
  `--matches`, `--gt`, `--lt`, `--changes`, `--stable-for <DUR>`;
  default is "exit code 0 means match". `--until-http` accepts a DSL
  via `--match` (`status == 200`, `body contains foo`,
  `$.json.path == "x"`). Status line uses TTY in-place rewrite;
  pass `--verbose` (or run non-TTY) for newline-per-poll. Audit
  entry per watch (verb=`watch`). Exit codes: 0 match, 124 timeout,
  130 cancelled, 2 error.

- **B5 — `inspect search` cross-service grep.** Single command that
  greps a pattern across logs and configs in selected containers and
  reports a per-service summary plus highlighted matches. Pushes the
  regex down to `grep -E` server-side and respects `--since`.

- **B6 — Selectors carry container kind.** `ns/svc:logs` and
  `ns/svc:config` selector suffixes route the same verb (e.g. `grep`)
  to the correct file class without per-verb flags.

- **B3 — Configurable per-line byte cap with `--no-truncate`.** Run /
  exec / search default to a 4 KiB per-line cap (sanitized for
  ANSI/C0). `--no-truncate` lifts the cap. Truncation marker now
  shows the byte count.

- **B2 — Server-side line filter pushdown.** `--filter-line-pattern`
  (alias `--match`) pushes through `grep -E` on the remote so
  irrelevant lines never cross the wire.

- **B1 — Streaming captured stdout.** `inspect exec` now streams
  output live AND captures it for audit; the audit `args` field
  reflects exactly what the operator saw.

- **B4 — `--reason` audit ergonomics.** Reason is validated up-front
  (240-char cap), echoed to stderr at run start, and stored in the
  audit log. `inspect audit ls --reason <substring>` filters by it.

- **B8 — `--no-truncate` propagation.** Threaded through `run`,
  `exec`, and `search` consistently.

- **B7 — `inspect run` exit code propagation.** Inner exit codes
  surface through `inspect run`/`exec` (clamped to 8 bits) so shell
  scripts can branch on them.

### Changed

- **Audit schema** carries two optional fields: `bundle_id` and
  `bundle_step`. Backward-compatible — entries written by 0.1.1 and
  earlier parse and render unchanged.

- **`inspect audit ls`** gains `--bundle <id>` to filter to a single
  bundle invocation.

### Defensive hardening (post-implementation audit pass)

A code-reality audit of the whole tree (independent of phase
numbering) surfaced and fixed five hardening items before release:

- **`bundle/exec.rs::run_parallel_matrix`**: matrix workers now run
  inside `std::panic::catch_unwind`. A panic inside a single branch
  converts to a normal failure recorded in `first_err` with
  `stop_flag` set, so rollback fires correctly instead of the panic
  unwinding the scope and bypassing rollback semantics.
- **`bundle/exec.rs`** (4 sites): replaced `.expect()`/`.unwrap()`
  on validated invariants (matrix presence, watch body presence)
  with `?` returning `anyhow!()` errors. Defense in depth — if
  validation ever regresses, the operator sees a clean error
  instead of a panic.
- **`safety/audit.rs::append`**: append now calls `f.sync_data()`
  after `flush()`. Audit entries survive power loss on conformant
  filesystems. Best-effort: warns and continues on filesystems that
  don't implement `fsync` (some FUSE/network mounts) rather than
  refusing to write the record.
- **`bundle/checks.rs::http_ok`**: `curl` invocation gains
  `--connect-timeout 5 --max-time 15`. A stuck HTTP endpoint can no
  longer pin SSH for the full per-check timeout budget.
- **`verbs/watch.rs::probe_http`** + `WatchArgs.insecure` +
  `WatchStep.insecure`: same `curl` timeout guards, plus an opt-in
  `--insecure` flag for self-signed staging endpoints. Disabled by
  default; documented as not-for-production.

### Notes

- New CLI surface: `inspect watch --help`, `inspect bundle --help`,
  `inspect bundle plan --help`, `inspect bundle apply --help`.
  All carry the canonical `See also: inspect help …` footer.
- 27 test suites, 555+ tests. CI gates (`cargo fmt --all -- --check`,
  `cargo build --locked`, `cargo test --locked`) all green. Clippy
  reports only the pre-existing `result_large_err` advisories,
  which are intentional for an SRE tool that maps services and
  errors as first-class values.

## [0.1.1] — Phase C field-feedback patches

Phase C of `INSPECT_v0.1.1_PATCH_SPEC.md`. Builds on Phases A + B;
same v0.1.1 release.

### Added

- **P4 — Secret masking on `run`/`exec` stdout.** Lines that look like
  `KEY=VALUE` and whose key matches a known secret pattern (suffixes
  `_KEY`, `_SECRET`, `_TOKEN`, `_PASSWORD`, `_PASS`, `_CREDENTIAL(S)`,
  `_APIKEY`, `_AUTH`, `_PRIVATE`, `_ACCESS_KEY`, `_DSN`,
  `_CONNECTION_STRING`; exact: `DATABASE_URL`, `REDIS_URL`, `MONGO_URL`,
  `POSTGRES_URL`, `POSTGRESQL_URL`) are masked to `head4****tail2` by
  default. Values shorter than 8 characters become `****`. The
  `export ` prefix and matching quote pairs are preserved so the
  output remains paste-friendly. Opt-out flags: `--show-secrets`
  (verbatim, also stamps `[secrets_exposed=true]` into the audit
  args on `exec`) and `--redact-all` (mask every KEY=VALUE line, not
  just keys we recognize). When masking actually fired during an
  `exec`, the audit args is stamped with `[secrets_masked=true]` so
  reviewers can tell verbatim runs apart from masked ones.
- **P5 — `--merged` multi-container log view.** `inspect logs <sel>
  --merged` interleaves output from every selected service into a
  single `[svc] <line>`-prefixed stream sorted by RFC3339 timestamp
  (we inject `--timestamps` into the underlying `docker logs`
  invocation). Batch mode does a parallel fan-out via
  `std::thread::scope` then k-way merges the per-source buffers
  through a `BinaryHeap<Reverse<MergeLine>>`. Follow mode pipes every
  stream through a single `mpsc::channel` and prints in arrival
  order. Lines without a parseable timestamp sink below dated lines
  but preserve their per-source order.
- **P9 — Progress spinner on slow log/grep fetches.** Hand-rolled
  100ms-frame Unicode spinner drawn to stderr after a 700ms warm-up.
  Suppressed automatically in JSON mode, when stderr is not a TTY,
  and when `INSPECT_NO_PROGRESS=1` is set (used by CI and the
  acceptance tests). No new dependencies.
- **P11 — Inner exit code surfacing.** `ExitKind::Inner(u8)` is the
  fourth process exit category, alongside `Success`/`NoMatches`/
  `Error`. `inspect run -- 'exit 7'` and `inspect exec --apply --
  'exit 9'` now propagate the remote command's exit code to the
  shell. Multi-target invocations with mixed inner exits still fall
  back to the generic `Error` (=2). High bits of the inner code are
  clamped to the low 8 via `clamp_inner_exit`.
- **P13 — Discovery `docker inspect` per-container fallback.** A
  single wedged container used to take the entire host's
  `inspect setup` down with it: the batched `docker inspect` would
  hit the 30s budget and the whole result was discarded. We now
  budget the batch call at 10s and on failure (timeout, partial JSON,
  daemon hiccup) re-probe each container individually with a 5s
  budget. Containers whose individual probe also fails are recorded
  as a warning AND the corresponding `Service.discovery_incomplete`
  bit is set in the persisted profile so later verbs can detect
  partial data. `inspect setup --retry-failed` re-runs discovery and
  merges in only the previously-incomplete services, leaving the
  rest of the profile cached.

### Acceptance

- New `tests/phase_c_v011.rs`: 6 acceptance tests pinning P4 (Anthropic
  key masking + `--show-secrets` audit breadcrumb), P9 (no spinner in
  JSON mode), P11 (run + exec inner-exit propagation), P13
  (`discovery_incomplete` round-trips through YAML).

## [0.1.1] — Phase B field-feedback patches

Phase B of `INSPECT_v0.1.1_PATCH_SPEC.md`. Builds on Phase A; same
v0.1.1 release. No deprecation paths -- v0.1 is a single-user
pre-release, see the README banner.

### Added

- **P6 — `inspect run <sel> -- <cmd>` (read-only).** New verb for the
  90% case where operators want a one-shot remote command (`ps`,
  `cat /proc/...`, `redis-cli info`) without paying for the
  write-verb interlock. Streams stdout line-by-line via the P1
  streaming primitive. No `--apply`, no audit log, no fanout
  threshold. Accepts `--filter-line-pattern <regex>` for server-side
  pushdown of the same `grep -E` logic logs/grep use.
- **P3 — `--match` / `--exclude` line filters.** Both `inspect logs`
  and `inspect grep` now accept `--match <regex>` (`-g`) and
  `--exclude <regex>` (`-G`), each repeatable. We push them down to
  the remote host as a `grep -E` / `grep -vE` pipeline suffix, with
  `--line-buffered` in `--follow` mode so live streams aren't
  block-buffered behind the filter. Multiple `--match` flags OR
  together (`(?:p1)|(?:p2)`).
- **P10 — `--since-last` resumable cursor.** `inspect logs --since-last`
  and `inspect grep --since-last` resume from the previous run's
  start time, persisted under `~/.inspect/cursors/<ns>/<svc>.kv`
  (mode 0600, dir 0700). Cold-start fallback: `INSPECT_SINCE_LAST_DEFAULT`
  (default `5m`). `--reset-cursor` deletes the file. `--since` and
  `--since-last` are mutually exclusive.
- **P12 — `--reason <text>` on every write verb.** Added to
  `restart`/`stop`/`start`/`reload`, `exec`, `cp`, `edit`, `rm`,
  `mkdir`, `touch`, `chmod`, `chown`. Recorded in the audit log as
  `AuditEntry.reason`, rendered as a trailing column in
  `inspect audit ls` and on its own line in `inspect audit show`.
  `inspect audit ls --reason <substr>` filters case-insensitively.
  240-character cap; oversize values rejected up-front with a clean
  error.

### Changed

- **P7 — `--allow-exec` removed from `inspect exec`.** The double-gate
  rationale dissolved with P6: read-only ad-hoc commands belong to
  `inspect run`, write-y ones belong to `inspect exec --apply`. The
  apply gate still fires; the second confirmation flag is gone.

### Module-level diff

- New: `src/verbs/run.rs`, `src/verbs/line_filter.rs`,
  `src/verbs/cursor.rs`, `tests/phase_b_v011.rs`.
- New paths: `paths::cursors_dir()`, `paths::cursor_file()`.
- `safety::AuditEntry.reason: Option<String>` (serde-skipped when None).
- `safety::validate_reason()` shared validator (≤ 240 chars).

## [0.1.1] — Phase A field-feedback patches

Phase A of `INSPECT_v0.1.1_PATCH_SPEC.md`: three patches that came out
of the first real ~60-call production debugging session.

### Fixed

- **P2 — phantom service names.** Discovery now records the real
  `docker ps` name in `Service.container_name` alongside the
  user-facing `name`. Every `docker logs|exec|restart|stop|start|kill`
  call site uses the container name; selectors and labels keep using
  the friendly name. Eliminates the v0.1.0 footgun where
  `inspect logs arte/api` produced `docker logs api` on a host whose
  actual container was `luminary-api`.
  - Schema: `Service.container_name: String` is **required**. Old
    profiles fail with a clean "run `inspect setup <ns>` to regenerate"
    error.
  - New helper `Step::container()` chooses the right token in one
    place; `cp`, `edit`, `mkdir`, `touch`, `chmod`, `chown`, `rm`,
    `exec`, `logs`, lifecycle (`restart`/`stop`/`start`/`reload`), and
    the lower-level `exec/reader/{logs,file}.rs` all switched.

### Added

- **P1 — streaming `--follow`.** New `ssh::exec::run_remote_streaming`
  pumps stdout line-by-line from the SSH child instead of waiting for
  the command to exit. `inspect logs --follow` now renders every line
  the moment it crosses the wire. The verb wrapper retries the SSH
  call up to three times with 1s/2s/4s backoff so a transient drop
  doesn't end the operator's session; Ctrl-C still cancels promptly.
  The `RemoteRunner` trait grew a default `run_streaming` method so
  mock-backed tests keep working unmodified.
- **P8 — `inspect help <verb>` fallback.** When the named topic has no
  editorial body, the dispatcher falls through to clap's long-help
  renderer for the matching subcommand. Users can type either
  `inspect help logs` or `inspect logs --help` and get help. The
  "did you mean" suggester now also considers verb names, so
  `inspect help serch` hints `did you mean: search?`.

### Tests

- New `tests/phase_a_v011.rs` (6 cases) pins the P2 round-trip and
  P1 streaming wire-up against regressions.
- `tests/help_contract.rs` gained 3 P8 guards including a
  one-test-per-verb fallback assertion.

## [Unreleased]

### Added — documentation

- `docs/MANUAL.md`: end-user manual covering install, concepts, every
  verb, the LogQL DSL, recipes, fleet ops, configuration, and
  troubleshooting. Mirrors the in-binary `inspect help <topic>` content.
- `docs/RELEASING.md`: maintainer notes for cutting a tag, hosting the
  install script, hotfix flow, and updating the Homebrew tap.
- `CONTRIBUTING.md`, `SECURITY.md`: standard public-repo files
  documenting the dev loop, quality gates, and the vulnerability
  disclosure process.
- `archives/README.md`: marks the planning archive as historical and
  points readers at the active docs.

### Changed — documentation

- Root `README.md` rewritten for the public release: clearer pitch,
  table of contents, "How it works" diagram, documentation map, and
  an explicit note that the install URL is served by GitHub directly
  (no separate server to deploy).
- `.gitignore` extended to cover common editor/OS artifacts.

## [0.1.0] — 2026-04-26

First public release.

### Added — capabilities (bible §1)

- Fleet-wide selector grammar: `@alias`, `ns/svc`, regex (`^pulse-.*$`),
  unions (`a,b`), groups (`@storage`), host steps (`_`).
- Read verbs: `ps`, `status`, `health`, `logs`, `cat`, `grep`, `find`,
  `ls`, `network`, `images`, `volumes`, `ports`.
- Write verbs (dry-run by default, `--apply` to enact): `cp`, `edit`,
  `chmod`, `chown`, `mkdir`, `rm`, `touch`, `restart`, `stop`, `start`,
  `exec`. Diff preview, atomic writes, audit trail with snapshot
  rollback.
- LogQL-style query engine: `inspect search '{server="arte"} |= "x"'`
  with stages `json | logfmt | pattern | regexp | line_format |
  label_format | drop | keep | <field op value> | map { ... }`,
  and metric forms `count_over_time`, `rate`, `bytes_over_time`,
  `bytes_rate`, `absent_over_time`, plus vector aggregations
  (`sum`, `avg`, `min`, `max`, `topk`, `bottomk`, `quantile_over_time`)
  with `by`/`without` grouping.
- Discovery + profile cache (`~/.inspect/profiles/<ns>.yaml`, mode 0600,
  TTL 7d). Drift detection with non-blocking probe and `setup --force`
  remediation.
- Recipe system (`inspect recipe <name>`) with builtin and YAML user
  recipes; `--apply` lifts dry-run gates on mutating steps only.
- Help system: in-binary topic catalog (`inspect help <topic>`),
  keyword search (`inspect help search <query>`), pager-aware rendering,
  `--json` machine-readable variant, no-network guarantee.
- Output contract: `--json`, `--jsonl`, `--csv`, `--table`, `--md`,
  `--format` (Go-template), `--raw`. Stable schema versioned in JSON
  envelope (`schema_version`).
- Safety: secret redaction (RFC-style, deterministic), 0600 file modes,
  no secrets-at-rest, SIGINT/SIGTERM-aware cancel with partial-result
  envelope.

### Added — distribution (Phase 12)

- GitHub Actions release workflow producing static-musl Linux
  (`x86_64`, `aarch64`) and Apple Darwin (`x86_64`, `aarch64`) tarballs,
  per-artifact `sha256`, aggregate `SHA256SUMS`, and keyless cosign
  signatures via GitHub OIDC.
- One-shot installer at `scripts/install.sh` with checksum + cosign
  verification, atomic install, and rollback-safe behavior.
- Static musl `Dockerfile` (two-stage build).
- Homebrew formula template at `packaging/homebrew/inspect.rb` (publish
  to a custom tap; not homebrew/core for v0.1.0).
- `cargo install inspect-cli` path (gated behind `vars.PUBLISH_CRATE`).
- CI workflow with fmt + clippy (`-D warnings`) + test on Linux and
  macOS, plus an MSRV (1.75) build job.

### Quality gates locked

- `cargo build` and `cargo test` are warning-free.
- `[lints.rust] dead_code = "deny"` in `Cargo.toml`.
- Contract test `tests/no_dead_code.rs` enforces:
  - H3: every `#[allow(dead_code)]` carries `// v2: <tag>`.
  - H4: zero module-wide `#![allow(dead_code)]`.
  - H5: total surviving suppressions ≤ 1.
- Test count: 488 passing across 18 suites.

### Out of scope for v0.1.0

Items deferred to v2 (tracked in `archives/INSPECT_BIBLEv6.2.md` §27):
TUI mode, k8s discovery, distributed tracing, OS keychain integration,
per-user policies, russh fallback, parameterized aliases, password
auth, remote agents.

[0.1.0]: https://github.com/jpbeaudet/inspect/releases/tag/v0.1.0
