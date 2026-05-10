Read [](file:///workspaces/inspect/INSPECT_v013_PAIN_POINT_AUDIT.md)

Let me dive into the codebase against this checklist. I'll investigate the priority items first via parallel searches:            

Search complete: F15 transfer audit

Searched codebase for "F15 put/get/cp transfer implementation atomic write temp rename O_EXCL mode 600", 18 results

Searched for files matching `**/src/verbs/transfer.rs`, 1 match

Searched for regex `atomic.*write|temp.*rename|O_EXCL|mode.*600|snapshot.*revert|revert.*before.*overwrite` (`**/src/verbs/transfer.rs`), 5 results

Searched for regex `large.*file|stdin.*max|STDIN_MAX|large_file` (`**/src/verbs/**`), 16 results

Read [](file:///workspaces/inspect/src/verbs/transfer.rs#1-1), lines 1 to 200

Searched for regex `atomic-write|base64 <path>|cat > /tmp && mv /tmp` (`**/src/verbs/transfer.rs`), 6 results

Searched for regex `O_EXCL|mode 600|temp file|rename` (`**/src/verbs/transfer.rs`), 3 results

Searched for regex `snapshot.*revert|revert.*before.*overwrite|capture timing` (`**/src/verbs/transfer.rs`), 1 result

Searched for regex `stdin-max|STDIN_MAX|--stdin-max` (`**/src/verbs/transfer.rs`), no results

Read [](file:///workspaces/inspect/src/verbs/transfer.rs#1-1), lines 1 to 260

Searched for files matching `**/src/verbs/transfer*.rs`, 1 match

Searched for text `cat > /tmp && mv /tmp` (`**/src/verbs/**`), 1 result

Searched for text `base64 <path>` (`**/src/verbs/**`), 1 result

Searched for text `atomic-write` (`**/src/verbs/**`), 4 results

Read [](file:///workspaces/inspect/src/verbs/transfer.rs#260-260), lines 260 to 580

Search complete: SSH lifecycle audit

Search complete: L7 redaction audit

Searched codebase for "L7 redaction PEM private-key block masking multi-line BEGIN..END", 14 results

Searched codebase for "URL-embedded credentials postgres://user:pass@host masking", 21 results

Searched codebase for "header redaction Authorization Cookie", 15 results

Searched codebase for "env-var masking base64-encoded secret bypass", 12 results

Searched for files matching `**/redact/**`, 5 matches

Searched for files matching `**/pem*.rs`, 1 match

Searched for files matching `**/url*.rs`, 1 match

Searched for regex `BEGIN.*PRIVATE.*KEY|PRIVATE.*KEY.*BEGIN` (`**/redact/**`), 26 results

Searched for regex `postgres://.*user:pass|postgres\\://.*user:pass` (`**/redact/**`), no results

Searched for regex `base64|Base64` (`**/redact/**`), no results

Searched for files matching `**/redact/**`, 5 matches

Searched for regex `PEM|pem` (`**/redact/**`), 50 results

Searched for files matching `**/redact/pem*.rs`, 1 match

Searched for regex `BEGIN.*PRIVATE.*KEY|PRIVATE.*KEY.*BEGIN` (`**/redact/**`), 26 results

Searched for files matching `**/redact/url*.rs`, 1 match

Searched for regex `url-embed|url-embedded|credentials.*url` (`**/redact/**`), no results

Searched for files matching `**/redact/header*.rs`, 1 match

Searched for regex `Authorization|Cookie|header` (`**/redact/**`), 50 results

Read [](file:///workspaces/inspect/src/redact/mod.rs#1-1), lines 1 to 260

Read [](file:///workspaces/inspect/src/redact/pem.rs#1-1), lines 1 to 260

Read [](file:///workspaces/inspect/src/redact/header.rs)

Read [](file:///workspaces/inspect/src/redact/url.rs)

Search complete: Audit + bundle + signal audit

Search complete: L3 alias audit

Searched codebase for "L3 parameterized aliases $param substitution missing-param error vs silent empty", 17 results

Searched codebase for "cycle detection at define-time vs use-time max chain depth shell-metacharacter handling injection prevention", 26 results

Searched for files matching `**/alias.rs`, 2 matches

Searched for regex `\$\w+|\$\{.*\}|param.*alias|alias.*param` (`**/alias.rs`), 73 results

Searched for regex `missing-param|missing param|parameterized.*alias`, 38 results

Searched for regex `cycle.*detect|detect.*cycle|max.*chain.*depth`, 17 results

Searched for regex `shell-metachar|inject|injection.*prevent|escape.*quote|quote.*escape`, 130 results

Read [](file:///workspaces/inspect/src/alias.rs#1-1), lines 1 to 200

Read [](file:///workspaces/inspect/src/alias.rs#1-1), lines 1 to 260

Read [](file:///workspaces/inspect/src/alias.rs#260-260), lines 260 to 520

Read [](file:///workspaces/inspect/src/logql/alias_subst.rs)

Read [](file:///workspaces/inspect/src/logql/alias_subst.rs#216-216), lines 216 to 216

Read [](file:///workspaces/inspect/src/commands/alias.rs)

Read [](file:///workspaces/inspect/src/alias.rs#520-520), lines 520 to 820

Search complete: Output format security

Searched codebase for "Output format security Go-template field syntax inspect output template injection", 19 results

Searched codebase for "CSV cell escaping formula injection = + - @ prefixes prefix-quote", 20 results

Searched codebase for "unicode-width handling CJK emoji table alignment unicode_width crate", 21 results

Searched for files matching `**/format/**`, 4 matches

Searched for regex `{{\.field|\.field}}|template\.Inject|text\.template|html\.template` (`**/format/**`), 2 results

Searched for regex `prefixquote|prefix.*quote|quote.*prefix|\=|\+|\-|\@` (`**/format/**`), 200 results

Searched for regex `unicode-width|unicode_width|CJK|emoji|east asian|wide char` (`**/format/**`), 6 results

Searched for files matching `**/inspect/**`, no matches

Read [](file:///workspaces/inspect/src/format/mod.rs)

Read [](file:///workspaces/inspect/src/format/safe.rs)

Read [](file:///workspaces/inspect/src/format/template.rs#1-1), lines 1 to 260

Read [](file:///workspaces/inspect/src/format/render.rs#1-1), lines 1 to 280

Read [](file:///workspaces/inspect/src/format/render.rs#420-420), lines 420 to 580

Read [](file:///workspaces/inspect/src/format/template.rs#259-259), lines 259 to 420

Search complete: Compose + discovery audit

Search complete: Watch + script + exec

Strong data on transfer + redaction. Let me direct-investigate the missing areas (alias, audit args, watch, signals, format, ssh):                          

Searched for regex `missing|cycle|MAX_DEPTH|chain|param|expand` (`**/src/alias.rs`), 20 results

Searched for regex `hash_chain|prev_hash|verify|fsync|sync_all|hash_prev` (`**/src/safety/audit.rs`), 4 results

Searched for regex `redact_args|args_for_audit|mask.*args|secrets.*command` (`**/src/**/*.rs`), 3 results

Searched for regex `interval|elapsed|progress|stderr|tick|stack` (`**/src/verbs/watch.rs`), 20 results

Searched for regex `SIGINT|signal|ctrl_c|kill_remote|terminate` (`**/src/**/*.rs`), 20 results

Searched for regex `csv|template|formula|injection|unicode_width|UnicodeWidth` (`**/src/format/**/*.rs`), 20 results

Searched for regex `stale|ControlPath|MaxSessions|password|attempt|exit -O|UNIX_PATH_MAX|sun_path` (`**/src/ssh/**/*.rs`), 20 results

Searched for regex `com\.docker\.compose\.service|compose.project|service_name|container_name` (`**/src/**/*.rs`), 20 results

Now drill into the specific items I haven't confirmed yet:                                                                               

Searched for regex `chain|integrity|prev_id|prev_sha|tamper|verify_chain` (`**/src/safety/audit.rs`), 3 results

Searched for regex `verify|integrity|chain|tamper` (`**/src/commands/audit.rs`), 12 results

Searched for regex `password.*attempt|max.*tries|3.*attempt|brute.force|attempts_left` (`**/src/**/*.rs`), 18 results

Searched for regex `stale|StaleSocket|socket_alive|check_master|-O check|sun_path|UNIX_PATH_MAX` (`**/src/ssh/**/*.rs`), 20 results

Searched for regex `.*` (`**/src/format/safe.rs`), 20 results

Searched for regex `SIGINT|SIGTERM|kill|forward|remote|propagate|orphan` (`**/src/exec/cancel.rs`), 20 results

Searched for regex `audit.*args|entry\.args|secrets_in_args|redact.*command|mask.*cmd` (`**/src/verbs/run.rs`), 4 results

Searched for regex `missing.*param|MissingParameter|required|UnknownParameter|cycle` (`**/src/alias.rs`), 20 results

Searched for regex `csv|=|@|formula|leading.equals` (`**/src/format/**/*.rs`), 20 results

Searched for regex `socket.*length|path.*108|UNIX_PATH|too long|sun_path` (`**/src/ssh/**/*.rs`), no results

Read [](file:///workspaces/inspect/src/verbs/run.rs#1100-1100), lines 1100 to 1139

Read [](file:///workspaces/inspect/src/format/safe.rs#20-20), lines 20 to 100

Read [](file:///workspaces/inspect/src/verbs/watch.rs#160-160), lines 160 to 220

Read [](file:///workspaces/inspect/src/ssh/master.rs#240-240), lines 240 to 300

Searched for regex `csv|Csv|render_csv|to_csv|"="` (`**/src/**/*.rs`), 20 results

Searched for regex `ControlPath.*length|socket.*108|too long for|sun_path` (`**/src/**/*.rs`), no results

Searched for regex `stamp_args|entry\.args|secrets_masked|secrets_exposed` (`**/src/verbs/exec.rs`), no results

Searched for regex `ctrl_c|cancel|rollback|partial|completed_steps|prompt` (`**/src/verbs/steps.rs`), 13 results

Searched for regex `transcript.*rotate|transcript.*size|MAX_TRANSCRIPT|history.*stream` (`**/src/**/*.rs`), 7 results

Read [](file:///workspaces/inspect/src/verbs/watch.rs#230-230), lines 230 to 270

Read [](file:///workspaces/inspect/src/alias.rs#490-490), lines 490 to 560

Searched for regex `socket.*mode|0o600.*sock|0o700|set_permissions.*sock` (`**/src/**/*.rs`), 7 results

Searched for regex `current_hash|new_hash|mismatch|--force|drifted` (`**/src/commands/revert.rs`), 14 results

Searched for regex `per.target|revert.*step|partial|completed.*revert|rollback.*targets` (`**/src/verbs/steps.rs`), 20 results

Searched for regex `password|redact|REDACTED|env_redact` (`**/src/commands/show.rs`), 11 results

Searched for regex `args|secrets_masked|secrets_exposed|stamp` (`**/src/verbs/exec.rs`), no results

Searched for regex `stdin.*tty|empty.*stdin|hang.*stdin|stdin.*close|null.*stdin` (`**/src/**/*.rs`), 20 results

Searched for regex `docker exec -i|"docker exec"|exec_args.*stdin|with_stdin.*docker` (`**/src/**/*.rs`), 8 results

Read [](file:///workspaces/inspect/src/verbs/run.rs#430-430), lines 430 to 470

Read [](file:///workspaces/inspect/src/verbs/run.rs#700-700), lines 700 to 730

I have enough data. Here's the audit report against the pain-point checklist:

---

# Pain-Point Checklist — Findings Report

I checked the v0.1.3 codebase against every item in the checklist. Mostly strong coverage; **9 confirmed gaps and 5 documented limitations** below. Numbering matches the checklist sections.

## ✅ Already protected (no action)

| § | Pain point | Code path |
|---|---|---|
| 1.1 | Stale socket recovery | master.rs — `check_socket` → `remove_file` if stale, then re-create |
| 1.4 | MaxSessions diagnostic | exec.rs — explicit "MaxSessions hit" mapping with operator hint |
| 1.5 | Password 3-attempt cap | master.rs — `pub const PASSWORD_MAX_ATTEMPTS: usize = 3` |
| 1.5 | Password not in audit/show | `password_env` stores the **env-var name**, not the value; show.rs renders `redact::REDACTED` for `key_inline` |
| 1.6 | Sockets dir mode 0700 | master.rs `ensure_sockets_dir` |
| 2.1 | Docker exec empty-stdin hang | run.rs — TTY/empty-buf detection skips `-i`, falls through to plain `docker exec ... sh -c ...` (no `-i`) |
| 2.5 | SIGINT to remote process | cancel.rs — two-tier: first Ctrl-C forwards via PTY; second escalates to channel close + SIGHUP |
| 3.2 | PEM block masking (multi-line) | pem.rs — stateful BEGIN/END machine, 7 algorithm forms incl. PGP+OPENSSH, public certs/keys deliberately pass through |
| 3.3 | URL-embedded credentials | url.rs — `scheme://user:****@host` rewrite |
| 5.1 | Atomic remote write | transfer.rs — `cat > <tmp>; mv <tmp> <path>`; `set -e` aborts on any failure |
| 5.4 | Revert capture **before** overwrite | transfer.rs — `let prev_text = read_remote(...)` then `let prev_hash = snaps.put(...)` runs **before** the `runner.run(write...)` call |
| 6.3 | Revert hash-drift detection | revert.rs — `current_hash != recorded_new_hash` exits with `Re-run with --force to override` |
| 6.4 | Audit fsync + lock | audit.rs — `flock(LOCK_EX)` + `sync_all`, with degraded warning on FUSE/network mounts |
| 7.1 | Per-target step results | steps.rs — per-target `TargetStepResult` (ok/failed/timeout); aggregate from per-target slice; cancel propagates via global flag |
| 9.1 | Template injection | template.rs — custom parser; data values flow through `value_to_str` and `eval_expr` as text only, never re-parsed as template |
| 9.2 | CSV formula injection | render.rs — `csv_escape` defuses leading `=`, `+`, `-`, `@` per OWASP guidance |
| 10.1 | Watch interval drift / no stacking | watch.rs — `let next = Instant::now() + interval; sleep(wake - now)` runs **after** the probe; next poll cannot start until previous completes |
| 10.3 | Watch progress on long timeout | watch.rs — `\r\x1b[K[inspect] watching {label} (poll N, Xs elapsed)` on stderr |
| 11.1 | Missing alias param error | alias.rs — `AliasError::MissingParam { declared, example }` |
| 11.2 | Alias cycle at define-time | alias.rs — `find_cycle` runs in `add()`, prints `a -> b -> a` chain |
| 11.3 | Alias param shell injection | Param values land in the **selector grammar**, not a shell. `;`, `$()`, backticks fail to parse as service names. No shell-context code path exists for substituted values |

Plus safe.rs — ANSI-escape stripping (CVE-2017-7768 class), C0 control byte rewrite, line-length budget — exceeds the spirit of §9.x even though it's not in the checklist.

---

## 🔴 Confirmed gaps (real production risk)

### G1. §1.3 — ControlPath length not validated
No code path checks whether `~/.inspect/sockets/<namespace>.sock` would exceed the sun_path limit (104 on macOS, 108 on Linux).
**Repro:** namespace name 80+ characters → `ssh` fails with `unix_listener: ... too long for Unix domain socket` rather than a clear inspect-level error.
**Where to fix:** master.rs `ensure_sockets_dir` / `socket_path` should validate length and emit a structured "namespace name too long for ControlPath; rename or set INSPECT_SOCKETS_DIR=/tmp/i" error.

### G2. §6.1 — Audit `args` field NOT redacted
`stamp_args` in run.rs only **appends a tag** (`[secrets_masked=true]` or `[secrets_exposed=true]`); the raw `user_cmd` flows in verbatim. An operator running `inspect exec arte -- "psql -p s3cret ..." --apply` writes `s3cret` into the audit log forever. The L7 redactor only runs on **stdout/stderr**, not the command string.
**Where to fix:** route `user_cmd` through a one-shot version of the L7 composer (header/URL maskers run unchanged on a single string; PEM doesn't apply; env masker works on `KEY=VALUE` substrings) before stamping.

### G3. §6.2 — Hash chain integrity NOT implemented (documented limitation)
Read [](file:///workspaces/inspect/INSPECT_v013_PAIN_POINT_AUDIT.md)

Let me investigate the remaining items I hadn't confirmed:      

Searched for regex `audit\s*verify|verify_chain|prev_hash|chain_hash` (`**/src/**/*.rs`), 20 results

Searched for regex `inspect-l11|/tmp/\.inspect|inspect-script-` (`**/src/**/*.rs`), 3 results

Searched for regex `stdin_max|max_stdin|--stdin-max|MAX_STDIN` (`**/src/**/*.rs`), 17 results

Searched for regex `com\.docker\.compose|compose\.service|compose\.project` (`**/src/**/*.rs`), 4 results

Searched for regex `depends_on|service_started|service_healthy|restart.*depend` (`**/src/**/*.rs`), 10 results

Searched for regex `network_mode.*host|HostNetwork|NetworkMode` (`**/src/**/*.rs`), no results

Read [](file:///workspaces/inspect/src/commands/audit.rs#220-220), lines 220 to 310

Read [](file:///workspaces/inspect/src/verbs/run.rs#600-600), lines 600 to 630

Read [](file:///workspaces/inspect/src/verbs/run.rs#970-970), lines 970 to 1010

Read [](file:///workspaces/inspect/src/verbs/transfer.rs#255-255), lines 255 to 295

Searched for regex `build_stream_atomic_script|O_EXCL|set -C|noclobber|umask` (`**/src/verbs/transfer.rs`), 8 results

Read [](file:///workspaces/inspect/src/verbs/transfer.rs#533-533), lines 533 to 605

Searched for regex `min[-_]consecutive|consecutive|sustained` (`**/src/**/*.rs`), 2 results

Searched for regex `unicode_width|UnicodeWidth|display_width|width\(\)` (`**/src/**/*.rs`), 8 results

Searched for regex `transcript.*rotat|transcript.*max|transcript.*size|max_transcript|rotate_transcript` (`**/src/**/*.rs`), 8 results

Searched for regex `RUN_TIMEOUT|probe_timeout|probe.*timeout|TIMEOUT_FAST|TIMEOUT_SLOW` (`**/src/**/*.rs`), no results

Searched for regex `redact_kind|RedactKind|env_kind|secrets_masked_kinds` (`**/src/**/*.rs`), 15 results

Searched for regex `rollback.*partial|partial.*rollback|skip.*failed|completed_steps|undo_branch` (`**/src/**/*.rs`), 4 results

Read [](file:///workspaces/inspect/src/bundle/exec.rs#1500-1500), lines 1500 to 1540

Searched for regex `cancel|sigint|ctrl.c|interrupt.*bundle` (`**/src/bundle/**/*.rs`), 4 results

Searched for regex `network_mode|NetworkMode|host.network` (`**/src/**/*.rs`), no results

Searched for regex `key.based.*more secure|key-based.*recommend|warn.*password.*auth|password.*key.based` (`**/src/**/*.rs`), 5 results

Searched for regex `MaxSession|channel open|too many sessions` (`**/src/**/*.rs`), 20 results

Read [](file:///workspaces/inspect/src/verbs/run.rs#1100-1100), lines 1100 to 1139

Read [](file:///workspaces/inspect/src/verbs/write/exec.rs#280-280), lines 280 to 320

Read [](file:///workspaces/inspect/src/redact/env.rs#1-1), lines 1 to 100

Searched for regex `ttl|expir|persist` (`**/src/commands/connections.rs`), 20 results

Searched for regex `--ttl|control_persist|ControlPersist|session_ttl` (`**/src/**/*.rs`), 20 results

Searched for regex `ControlMaster|control_master|-o\s+ControlMaster` (`**/src/ssh/**/*.rs`), 7 results

Read [](file:///workspaces/inspect/src/ssh/master.rs#430-430), lines 430 to 450

Searched for regex `ControlMaster|ControlPath|auto|-o\s` (`**/src/ssh/exec.rs`), 5 results

Read [](file:///workspaces/inspect/src/ssh/exec.rs#110-110), lines 110 to 130

I have enough data. Continuing the report from G3:

---

### G3. §6.2 — Hash-chain integrity is **honestly absent** (documented, not a gap)
[`src/commands/audit.rs#L223-L235`](src/commands/audit.rs "src/commands/audit.rs#L223-L235") explicitly documents: *"Honest scope: this catches accidental loss/truncation of snapshot files and silent on-disk corruption. It does **not** prove the JSONL log itself was not rewritten — a privileged local user can always edit `~/.inspect/audit/*.jsonl` and recompute matching snapshots."* `inspect audit verify` only validates each entry's snapshot file against `previous_hash`. **No prev-entry hash chain.** This is a known design choice (per the inline comment, the documented mitigation is to forward to syslog/journald/remote sink), not a forgotten feature. Treat as documented limitation, not bug.

### G4. §1.2 — `ControlMaster=auto` self-promotion not guarded
When the master socket is dead, [`src/ssh/exec.rs#L116-L121`](src/ssh/exec.rs "src/ssh/exec.rs#L116-L121") falls through to direct `ssh ... -o BatchMode=yes` without setting `ControlMaster=no`. Default OpenSSH is `no`, so most users are fine, but **operators with `ControlMaster auto` in their personal `~/.ssh/config` will see the inspect-spawned ssh promote itself to a master and background — exactly the zest.releaser scenario the checklist describes**.
**Fix:** add `.arg("-o").arg("ControlMaster=no")` in the `else` arm of the `use_socket` branch (the path that's used when the socket is missing/stale).

### G5. §1.3 — ControlPath length not validated
No length check on `~/.inspect/sockets/<namespace>.sock` against the sun_path limit (104 macOS / 108 Linux). A 50+ char namespace plus a long `$HOME` produces a cryptic `Connection refused` from the kernel, not an inspect-level error. **Fix:** add a length validation in [`socket_path`](src/ssh/master.rs )/[`ensure_sockets_dir`](src/ssh/master.rs ) that returns a structured error pointing at `INSPECT_SOCKETS_DIR=/tmp/i` as the workaround.

### G6. §3.1 — Base64-encoded secrets bypass masking (acknowledged limitation, not documented)
The L7 redactor only sees raw values; `echo $API_KEY | base64` produces an output the env masker doesn't recognize. **No code path** decodes base64 to compare against known secret values. The checklist explicitly invites either preventing this or **documenting** it. **Currently:** neither prevented nor documented in `inspect help safety`.
**Fix:** add a paragraph to [`src/help/content/safety.md`](src/help/content/safety.md ) (the existing L7 editorial topic) acknowledging that arbitrary post-processing (`base64`, `xxd`, `hex`, gzip, JSON-encoding) defeats line-oriented masking — operators must use `--show-secrets` consciously when running such commands and must NOT redirect their output to logs.

### G7. §10.2 — `--min-consecutive` not implemented
`grep -n 'min[-_]consecutive|sustained'` returns zero hits. Watch verb has `--until-cmd`, `--until-http`, `--timeout` but no way to require N consecutive passes. The checklist explicitly tags this as a known limitation but says **"Verify: documentation explicitly states that watch checks a point-in-time condition"** — the MANUAL/help does not currently call this out.
**Fix:** add a "known limitation" note to [`src/cli.rs`](src/cli.rs ) `LONG_WATCH` and to `inspect help watch` saying the predicate is checked at one polling instant; suggest an outer shell loop or wait until v0.1.4 ships `--min-consecutive`.

### G8. §3.4 — Multi-line secret values masked only on the first line
[`src/redact/env.rs`](src/redact/env.rs "src/redact/env.rs") `EnvMasker::mask_line` is line-oriented and only matches `KEY=VALUE` on a single line. A here-doc style:
```
export PRIVATE_KEY="-----BEGIN
MIIEvgIBADAN...
-----END"
```
masks line 1 but the Base64 lines 2-N pass through *unless* the [`PemMasker`](src/redact/pem.rs ) state machine fires (which it does for actual PEM keys — so PEM is safe). For non-PEM multi-line shell values (JSON service-account keys assembled with `\` continuations, multi-line API tokens) only the first line is masked.
**Status:** PEM keys are safe (the dedicated PEM masker handles them). Generic multi-line `KEY=VALUE` is the residual risk, identical to the GitHub Actions limitation the checklist cites. Documenting this in `inspect help safety` is the lowest-cost fix.

### G9. §5.2 — Atomic-write temp file does not refuse symlink targets (low-severity hardening)
[`build_stream_atomic_script`](src/verbs/transfer.rs ) emits `cat > $tmp` without `set -C` (noclobber). On Linux/POSIX, shell `>` redirection follows symlinks. **Mitigation in place:** the temp filename includes 8 hex chars of the *new* content's SHA-256 (`{path}.inspect.{hash[..8]}.tmp`), so an attacker cannot pre-plant a symlink without preimaging the new content — this defeats the classical attack in practice. But it's defense-in-depth only, not a structural defense.
**Fix:** prepend `set -C; ` to [`build_stream_atomic_script`](src/verbs/transfer.rs ) so bash uses `O_EXCL` on the temp open. If the temp path already exists (symlink or real file), the redirect fails fast.

---

## 🟡 Documented limitations (no action — verify wording)

| § | Limitation | Where verified |
|---|---|---|
| 6.2 | Hash chain absent | [`src/commands/audit.rs#L223-L235`](src/commands/audit.rs "src/commands/audit.rs#L223-L235") inline comment |
| 7.4 | Preflight TOCTOU | OS-error passthrough is fine |
| 10.2 | Watch is point-in-time only | **Currently undocumented** — see G7 |
| 3.1 | Base64 bypass | **Currently undocumented** — see G6 |
| 3.4 | Multi-line non-PEM secrets | **Currently undocumented** — see G8 |
| 4.4 | `network_mode: host` DNS | Not detected by discovery (zero matches for `network_mode\|HostNetwork`); accept as out-of-scope |

---

## 🟢 Sections fully passing

- **§1.4** MaxSessions: not just diagnosed at exit ([`src/ssh/exec.rs#L190-L196`](src/ssh/exec.rs "src/ssh/exec.rs#L190-L196")) but **proactively probed** at discovery time ([`src/discovery/probes.rs#L289-L321`](src/discovery/probes.rs "src/discovery/probes.rs#L289-L321")) and **throttled locally** by a per-host semaphore ([`src/ssh/concurrency.rs`](src/ssh/concurrency.rs "src/ssh/concurrency.rs")) — exceeds the checklist requirement.
- **§1.5** Password auth: 3-attempt cap, env-var name (not value) stored, [`maybe_warn_password_auth`](src/ssh/master.rs ) emits the recommend-keys warning, `show` redacts the env var.
- **§1.6** Sockets mode 0700, `connections` shows TTL+EXPIRES_IN, `--ttl` override on `connect`.
- **§5.1, §5.4** F15 atomic write + revert-before-overwrite verified line by line.
- **§7.1, §7.2** Bundle: per-branch skipped status (`peer branch failed first`), explicit `[inspect] STOPPING rollback — bundle is in mixed state` on rollback failure, `[inspect] bundle cancelled by signal` on Ctrl-C.
- **§8.1** Compose V1 vs V2: discovery uses the `com.docker.compose.service` **label** ([`src/discovery/probes.rs#L334-L339`](src/discovery/probes.rs "src/discovery/probes.rs#L334-L339")), not container-name string parsing — handles both separators automatically.
- **§9.3** Unicode width: [`src/format/render.rs#L20`](src/format/render.rs "src/format/render.rs#L20") imports `unicode_width::UnicodeWidthStr` and there's a regression test [`ascii_table_aligns_with_unicode_width`](src/format/render.rs ).

---

## Priority Triage (recommended fix order)

| Priority | Item | Effort | Class |
|---|---|---|---|
| **P0 (security)** | **G2** — redact `args` field in audit | medium (1 file, route through one-shot redactor) | secret leak in forensic record |
| P1 (correctness) | **G4** — explicit `ControlMaster=no` on dead-master fallthrough | trivial (1 line) | hang on user-customized ssh_config |
| P1 (UX) | **G5** — ControlPath length validation | small | cryptic kernel error |
| P2 (docs) | **G6, G7, G8** — three help-text additions | small | known limitations not surfaced |
| P3 (hardening) | **G9** — `set -C` in atomic-write script | trivial (1 token) | defense-in-depth |

**G2 is the only finding that I'd block a tag on.** Everything else is hardening or doc work. Want me to fix any subset of these now, or hold for triage?