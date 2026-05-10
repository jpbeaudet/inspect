# Inspect v0.1.3 — Real-World Pain Point Audit Checklist

**Source:** Deep research across dev blogs, GitHub issues, production postmortems, and security advisories. Every item maps to a documented real-world failure in a similar tool or technology.

**How to use:** For each feature in v0.1.3, run through the checklist items below. Each item has a test you can execute. Mark pass/fail. Fix all failures before tagging v0.1.3.

---

## 1. SSH ControlMaster & Auto-Reauth (F13, L4)

Real-world incidents: Lima VM #4913, Teleport #1650, zest.releaser #150, BOSH CLI #345, Ansible stale-socket deadlocks.

### 1.1 Stale socket after unclean disconnect
**Pain:** Lima VM discovered that `ssh -O exit` fails on a stale socket but the socket file persists. Every subsequent connection tries the dead socket, hangs for the SSH timeout, then fails. Users think the tool is broken.
- [ ] Kill the SSH master process with `kill -9` while a connection is active. Run `inspect run arte -- "echo alive"`. Verify: auto-detects stale socket, removes it, reconnects transparently, prints one-line notice. Does NOT hang for 30 seconds.
- [ ] Simulate network drop (disconnect WiFi mid-stream). Reconnect. Verify: next `inspect` command recovers without manual `disconnect`/`connect`.

### 1.2 ControlMaster becomes a new master unexpectedly
**Pain:** zest.releaser found that if a ControlMaster dies and a new SSH connection starts, that connection promotes itself to master and backgrounds itself — holding the caller's stderr pipe open indefinitely. The parent process hangs.
- [ ] With an active master, kill it. Immediately run `inspect run arte -- "echo test"`. Verify: the new connection does NOT background itself. Output returns to the caller.

### 1.3 ControlPath too long for Unix domain socket
**Pain:** Ansible hit this when usernames or hostnames are long. Unix domain socket paths are limited to 104-108 bytes on most systems.
- [ ] Create a namespace with a very long name (50+ characters). Verify: `inspect connect` works or fails with a clear error about path length, not a cryptic `Connection refused`.

### 1.4 MaxSessions exhaustion
**Pain:** OpenSSH's default `MaxSessions 10` silently refuses session 11+ with "channel open failed." Fleet operations against a server with 15+ containers hit this.
- [ ] Against a server with 15+ containers, run `inspect fleet status` or `inspect compose ps` that fans out to all containers. Verify: if sessions are refused, the error is clear ("MaxSessions limit reached; reduce concurrency or increase MaxSessions on the server"). No silent failures.

### 1.5 Password auth brute-force exposure (L4)
**Pain:** SSH password auth is inherently vulnerable to brute-force. Every security guide recommends disabling it.
- [ ] Verify: password auth has a max 3 attempt limit before abort.
- [ ] Verify: failed password attempts are NOT logged in plaintext (the password itself must never appear in audit, logs, or error messages).
- [ ] Verify: the tool emits a one-time warning that key-based auth is more secure.
- [ ] Verify: `inspect show <ns>` redacts the password env var value.

### 1.6 ControlPersist keeps sessions open for attack window
**Pain:** A 4-hour TTL means anyone with access to the socket file can open new SSH sessions without authenticating.
- [ ] Verify: socket file is mode 600.
- [ ] Verify: `inspect connections` shows TTL and expiry time for each active session.
- [ ] Verify: `--ttl` override works on `inspect connect` for sensitive environments.

---

## 2. Docker Exec & Stdin Forwarding (F9, F14, F16, L11)

Real-world incidents: moby/moby #45689 (stdout data loss), moby/moby #37870 (empty stdin hang), moby/moby #8679 (pipe failures), rancher-desktop #2094 (indefinite hang).

### 2.1 Docker exec -i hangs on empty stdin
**Pain:** `docker exec -i container bash -c "cat"` with empty stdin hangs indefinitely. This is a documented Docker bug that affects any tool piping to containers.
- [ ] Run `inspect run arte/service -- "cat"` with no stdin input. Verify: does NOT hang. Either returns empty or times out with a clear message.
- [ ] Run `echo "" | inspect run arte/service -- "cat"`. Verify: returns immediately, does not hang.

### 2.2 Docker stdout data loss when reader is slow
**Pain:** moby/moby #45689 documents that `docker run` loses stdout data when the downstream pipe reads slowly. The last chunk of data is silently dropped.
- [ ] Run `inspect run arte/service -- "dd if=/dev/zero bs=8192 count=1024 2>/dev/null | wc -c"`. Verify: the byte count is consistent across 10 runs. No silent data loss.

### 2.3 Script mode quoting preservation (F14)
**Pain:** Cross-layer quoting (local shell → SSH → bash → docker exec → psql) is the #1 source of "it works manually but not in the tool" bugs.
- [ ] Create a script with embedded double quotes, single quotes, dollar signs, and heredocs. Run via `inspect run --file script.sh arte/service`. Verify: the script reaches the remote interpreter byte-for-byte.
- [ ] Specifically test: `psql -c "SELECT 'embedded \"double\" quote';"` inside a script. Verify: no mangling.

### 2.4 Bidirectional stdin + stream (L11)
**Pain:** Half-duplex SSH channels make simultaneous stdin write + stdout stream unreliable. The two-phase approach (write script, then execute) avoids this.
- [ ] Run `inspect run --file script.sh --stream arte/service` on a script that produces output over 30 seconds. Verify: output streams in real-time AND the script body was received intact.
- [ ] Verify: temp file on remote is cleaned up after execution (no `/tmp/.inspect-l11-*` leftovers).

### 2.5 SIGINT forwarding (F16)
**Pain:** Teleport #1650: SSH exec requests without PTY don't forward SIGINT. The remote process keeps running after Ctrl-C, consuming resources.
- [ ] Run `inspect run --stream arte/service -- "tail -f /dev/null"`. Press Ctrl-C. Verify: the remote `tail` process is killed. Check with `inspect run arte -- "ps aux | grep tail"` — no orphan.
- [ ] Second Ctrl-C within 1 second should force-kill the local SSH process.

---

## 3. Secret Masking (F9 env masking, L7 extended masking)

Real-world incidents: GitHub Actions masking bypass via base64 encoding, Jenkins credentials-masking limitations, GitGuardian reports on URL-embedded credentials.

### 3.1 Base64 encoding bypasses masking
**Pain:** GitHub Actions auto-masks secrets in logs, but `echo "$SECRET" | base64` outputs the encoded value, which is trivially decodable and NOT masked. Any masking that only matches the raw value misses encoded forms.
- [ ] Run `inspect run arte/service -- "echo $ANTHROPIC_API_KEY | base64"`. Verify: the base64-encoded value is also masked (or the entire output is masked when the source is a known secret key).
- [ ] Document whether base64 bypass is accepted as a known limitation or actively prevented.

### 3.2 PEM blocks in output (L7)
**Pain:** Private key blocks in file content or error output leak the complete key in plaintext.
- [ ] Run `inspect run arte/service -- "cat /path/to/some/cert.pem"` (or any file containing a `-----BEGIN PRIVATE KEY-----` block). Verify: the PEM block is replaced with `[REDACTED PEM KEY]`.
- [ ] Verify: the masker handles multi-line PEM blocks correctly (the `BEGIN` and `END` markers span multiple lines).

### 3.3 URL-embedded credentials (L7)
**Pain:** Connection strings like `postgres://user:password@host/db` embed credentials in the URL. Line-oriented env-var masking doesn't catch them.
- [ ] Run `inspect run arte/service -- "echo 'DATABASE_URL=postgres://admin:s3cret@localhost/mydb'"`. Verify: the password portion is masked: `postgres://admin:****@localhost/mydb`.

### 3.4 Multi-line secrets where only first line is masked
**Pain:** GitHub Actions masks the first line of a multi-line secret but not subsequent lines.
- [ ] Store a multi-line secret (e.g., a JSON key file). Verify: all lines are masked, not just the first.

---

## 4. Docker Compose Verbs (F6, L8)

Real-world incidents: Docker Compose name resolution bugs (#7250, #8056, #9513), container name conflicts (#1488), project name ambiguity.

### 4.1 Service name vs container name confusion
**Pain:** Docker Compose creates containers named `project_service_1`. The service name in the compose file is `service`. Tools that use container names break when the project name changes.
- [ ] Verify: `inspect compose ps` shows service names (from compose file), not container names.
- [ ] Verify: selectors like `arte/api` resolve using the compose service name, not the Docker container name prefix.

### 4.2 Multiple compose projects on same host
**Pain:** Docker Compose V2 can run multiple projects. If inspect doesn't scope to a project, `inspect compose logs` might return logs from the wrong project.
- [ ] Deploy two compose projects with services named the same (e.g., both have `web`). Verify: `inspect compose ps arte/<project>` scopes correctly. No cross-project contamination.

### 4.3 Compose restart dependency ordering
**Pain:** `docker compose restart` does not restart dependencies. If service B depends on A, and both crash, restarting B without A leaves B in a failed state.
- [ ] `inspect compose restart arte/<project>/service-with-deps --apply`. Verify: the tool either restarts dependencies or warns that they are not running.

### 4.4 Network isolation with host network mode
**Pain:** Services using `network_mode: host` break Docker's built-in DNS service discovery. Container names don't resolve.
- [ ] Verify: if a service uses host network mode, `inspect` correctly identifies it and doesn't try to use Docker DNS for inter-service connectivity checks.

---

## 5. File Transfer (F15)

Real-world incidents: SCP broken pipe on large files, atomic write race conditions (TOCTOU), temp file symlink attacks.

### 5.1 Partial write on network interruption
**Pain:** If the network drops during a `cat | ssh` file transfer, the remote file may be partially written and left in a corrupted state.
- [ ] Verify: `inspect put` uses atomic write (write to temp, then rename). A failed transfer leaves the original file intact.
- [ ] Simulate network drop during transfer. Verify: no partial file at the target path. The temp file is cleaned up.

### 5.2 Temp file symlink attack (TOCTOU race)
**Pain:** If an attacker creates a symlink at the temp file path pointing to a sensitive file (e.g., `/etc/shadow`), the write goes to the symlink target instead.
- [ ] Verify: temp file is created with `O_EXCL` (exclusive create) or uses an unpredictable filename.
- [ ] Verify: temp file permissions are restrictive (mode 600) before the rename.

### 5.3 Large file transfer over cat|ssh
**Pain:** The `cat | ssh` approach buffers the entire file through the SSH channel. For files larger than available memory, this can OOM.
- [ ] Transfer a file larger than the `--stdin-max` cap. Verify: the tool rejects with a clear error pointing to `inspect cp` or `scp` for large files.

### 5.4 Revert capture before overwrite
**Pain:** If the snapshot is taken AFTER the overwrite (instead of before), the revert data is the new content, not the old.
- [ ] Run `inspect put local.conf arte/service:/etc/config.conf --apply`. Check the audit log. Verify: `previous_hash` matches the hash of the OLD file content, not the new. Verify the snapshot file contains the OLD content.

---

## 6. Audit Log & Revert (F11, F18, L5)

Real-world incidents: Hermes Agent #487 (hash-chain tampering), SOC2/ISO27001 audit log requirements, Jenkins credentials in logs.

### 6.1 Audit log secrets leakage
**Pain:** Audit entries that record command arguments may contain secrets if the command included credentials.
- [ ] Run `inspect exec arte -- "psql -U admin -p s3cret ..." --apply --reason "maintenance"`. Verify: the audit entry does NOT contain the password in the `args` field. Sensitive patterns in commands should be masked in the audit log too.

### 6.2 Hash chain integrity
**Pain:** Without hash chaining, an attacker who gains file access can silently delete or modify individual audit entries.
- [ ] Verify: `inspect audit verify` checks the hash chain and reports if any entry has been tampered with.
- [ ] Manually edit one line in the JSONL file. Run `inspect audit verify`. Verify: it detects the tamper.

### 6.3 Revert after someone else changed the file
**Pain:** If you revert to a snapshot, but someone else modified the file between your edit and your revert, the revert overwrites their changes.
- [ ] Apply an edit. Have another process modify the file. Run `inspect revert <id>`. Verify: the revert warns that the current file hash doesn't match `new_hash` and requires `--force`.

### 6.4 Log rotation during active session
**Pain:** If log rotation fires while the tool is appending to the JSONL file, entries can be lost or split across files.
- [ ] Verify: the audit writer opens the file in append mode and uses `fsync` after each write. File rotation (by external logrotate or by L5's `audit gc`) does not corrupt the active file.

### 6.5 Transcript size explosion (F18)
**Pain:** A 4-hour debug session with `--stream` on verbose services produces gigabytes of transcript data.
- [ ] Verify: transcript rotation is active (per-day files with configurable max size).
- [ ] Verify: `inspect history` handles large transcript files without OOM (streaming read, not full-file load).

---

## 7. Bundle Engine (B9, L6, L13)

Real-world incidents: Ansible partial-failure handling, Terraform apply failures with no rollback, deployment rollback postmortems.

### 7.1 Partial matrix failure rollback
**Pain:** If 4 of 6 parallel branches succeed and 2 fail, naive rollback undoes ALL 6 — including the successful ones that should be left alone.
- [ ] Run a bundle with a 4-branch parallel step where 2 branches fail. Verify: rollback only targets the 2 successful branches. The 2 failed branches are skipped (nothing to undo).
- [ ] Verify: `inspect bundle status <id>` shows per-branch pass/fail status.

### 7.2 Rollback script itself fails
**Pain:** If the rollback command fails (e.g., `docker compose start` fails because Docker is down), the system is in an indeterminate state with no further recovery.
- [ ] Simulate rollback failure. Verify: the tool reports the rollback failure clearly with the specific step that failed, does NOT silently exit 0.
- [ ] Verify: the audit log records both the original failure AND the rollback failure.

### 7.3 Ctrl-C during bundle execution
**Pain:** If the user presses Ctrl-C mid-bundle, what happens? If the bundle is between steps 3 and 4, steps 1-3 have been applied. No rollback, no cleanup.
- [ ] Start a multi-step bundle. Press Ctrl-C after step 2 completes. Verify: the tool prompts "Rollback completed steps 1-2? [y/N]" and executes rollback if confirmed.

### 7.4 Preflight check passes but condition changes before step executes
**Pain:** Preflight checks disk space. Between the check and the actual dump, another process fills the disk. The dump fails with "no space left."
- [ ] This is a known limitation (TOCTOU). Verify: the error message on disk-full is clear and actionable, not a raw OS error.

---

## 8. Discovery & Compose Project Detection (F2, F4, F5, F8)

Real-world incidents: Docker Compose V1 vs V2 naming changes, container name conflicts, DNS resolution failures.

### 8.1 Compose V1 vs V2 naming
**Pain:** Compose V1 uses `project_service_1`. Compose V2 uses `project-service-1`. Tools that hardcode the separator break.
- [ ] Verify: discovery handles both `_` and `-` separators in container names.
- [ ] Verify: the `com.docker.compose.service` label is used for service name extraction, not string parsing of the container name.

### 8.2 Stale discovery cache after container restart
**Pain:** A container that restarts gets a new container ID. The cached profile points to the old ID. Commands fail with "container not found."
- [ ] Restart a container. Without re-running `inspect setup`, run `inspect logs arte/<service>`. Verify: the command succeeds (resolves by name at command time, not by cached ID).

### 8.3 Docker daemon unresponsive during discovery
**Pain:** If the Docker daemon is overloaded, `docker inspect` can hang for minutes. The entire discovery blocks.
- [ ] Verify: per-container timeout (F2's three-bucket classifier) prevents one slow container from blocking the entire scan.

---

## 9. Output Formats & Templates (F7, F10)

### 9.1 Template injection
**Pain:** Go-template syntax (`{{.field}}`) can be exploited if user-controlled data flows into the template. A crafted log line containing `{{...}}` could inject template logic.
- [ ] Insert a log line containing `{{.service | printf "%s"}}` into a container's output. Run `inspect logs --format '{{.line}}'`. Verify: the template characters in the data are rendered as literal text, not executed as template code.

### 9.2 CSV injection
**Pain:** Values starting with `=`, `+`, `-`, or `@` in CSV output can be interpreted as formulas when opened in Excel. This is a known attack vector.
- [ ] Run `inspect ps --csv` where a container name starts with `=`. Verify: the value is quoted or escaped in the CSV output to prevent formula injection.

### 9.3 Unicode width in table rendering
**Pain:** CJK characters and emoji have double-width rendering. Tables that don't account for this misalign columns.
- [ ] Create a container with a CJK name. Run `inspect ps`. Verify: the table columns are correctly aligned despite variable-width characters.

---

## 10. Watch Verb (B10)

### 10.1 Polling interval drift
**Pain:** If the polled command takes longer than the interval, polls stack up and overwhelm the target.
- [ ] Set `--interval 2s` on a command that takes 5 seconds to execute. Verify: the next poll starts AFTER the previous one completes, not at the interval boundary. No stacking.

### 10.2 Watch with --until-http against a service that flaps
**Pain:** The service comes up, passes the health check, `inspect watch` exits 0. One second later, the service crashes again. The operator thinks it's healthy.
- [ ] This is a known limitation. Verify: documentation explicitly states that `watch` checks a point-in-time condition, not sustained health. Suggest using `--until-http` with `--min-consecutive 3` or similar if sustained health is needed (even if not implemented in v0.1.3).

### 10.3 Watch timeout with no output
**Pain:** A 10-minute `--timeout` with no output and no progress indicator leaves the operator staring at a blank terminal.
- [ ] Run `inspect watch arte/service --until-cmd "false" --timeout 30s`. Verify: progress line appears on stderr showing elapsed time and last result.

---

## 11. Parameterized Aliases (L3)

### 11.1 Missing parameter silent substitution
**Pain:** If `$svc` is not provided and the alias body contains `$svc`, it could silently substitute an empty string, producing a valid but wrong selector.
- [ ] Define alias with `$svc` parameter. Call without providing `svc`. Verify: clear error listing required parameters. Does NOT silently produce an empty-string selector.

### 11.2 Alias chain cycle
**Pain:** `@a` references `@b` which references `@a`. Infinite loop.
- [ ] Create a circular alias chain. Verify: error at definition time, not at use time. Max chain depth (5) enforced.

### 11.3 Shell metacharacters in parameter values
**Pain:** If a user passes `svc=pulse;rm -rf /` as a parameter value, and the value is interpolated into a shell command, it's command injection.
- [ ] Pass a parameter value containing `;`, `$()`, backticks. Verify: the value is treated as a literal string in the selector, never interpreted as shell.

---

## Summary: Priority Test Order

Run these tests first (highest production-risk):

1. **§5.4** — Revert captures BEFORE overwrite (data loss if wrong)
2. **§2.1** — Docker exec stdin hang (tool freezes on common pattern)
3. **§1.1** — Stale socket recovery (tool appears broken after network drop)
4. **§3.3** — URL credential masking (secret leak in agent context)
5. **§6.1** — Audit log secrets leakage (secrets in forensic record)
6. **§7.1** — Partial matrix rollback (wrong rollback destroys good work)
7. **§2.5** — SIGINT orphan processes (resource leak on remote)
8. **§4.1** — Service name vs container name (selector trust)
9. **§11.3** — Parameter injection (command execution via alias values)
10. **§9.1** — Template injection (code execution via log content)

Then cover the rest systematically by section.
