# Inspect — Known Pitfalls Audit Checklist

**Source:** Deep research across Stack Overflow, Grafana/Loki issue tracker, Rust forums, parser literature, SSH multiplexing docs, and CVE databases. Each item is a documented real-world failure mapped to inspect's codebase.

---

## 1. PARSER & LEXER (src/logql/)

### 1.1 The implicit grammar problem
**Source:** Laurence Tratt, "Why We Need to Know LR and Recursive Descent Parsing Techniques" (2023)
**Problem:** Hand-written recursive descent parsers always parse *something*, but it may not be the grammar you think. Bugs in hand-written parsers are common — they accept inputs they shouldn't, or reject inputs they shouldn't. Test suites focus on "good" inputs; the space of "bad" inputs is much larger.
**Check:**
- [ ] Do you have tests for every invalid query that should fail? (missing comma between label matchers, unclosed `{`, unclosed `"`, `|=` with no argument, nested `map` without closing `}`, metric query wrapping a metric query)
- [ ] Does every invalid query produce an actionable error with position info, not just "parse error"?
- [ ] Write a fuzzer: feed random/mutated strings and verify the parser never panics, only returns errors

### 1.2 Operator precedence in field filters
**Source:** Multiple parser tutorials cite arithmetic-expression-like precedence as a top source of bugs
**Problem:** `| status >= 500 and method =~ "POST" or path == "/health"` — is that `(status >= 500 and method =~ "POST") or path == "/health"` or `status >= 500 and (method =~ "POST" or path == "/health")`?
**Check:**
- [ ] Document and test the precedence: `and` binds tighter than `or` (LogQL convention)
- [ ] Test with explicit parentheses to verify grouping matches user intent
- [ ] Test deeply nested boolean expressions: `a and b or c and d or e`

### 1.3 String escaping in the lexer
**Source:** Multiple lexer/parser issue trackers; ANTLR discussion group; Absinthe GraphQL issue #165
**Problem:** LogQL strings can contain escaped quotes (`"foo\"bar"`), backslashes (`"path\\to"`), and potentially Unicode. Double-escaping is the #1 source of lexer bugs across languages. Users will paste log lines that contain quotes, backslashes, JSON, regex patterns.
**Check:**
- [ ] Test: `|= "foo\"bar"` — escaped quote inside string
- [ ] Test: `|= "C:\\Users\\path"` — escaped backslashes
- [ ] Test: `|~ "error|warn"` — pipe inside regex string (must not be parsed as pipeline operator)
- [ ] Test: `|= "{\"key\":\"value\"}"` — JSON inside a filter string
- [ ] Test: backtick quoting if supported (LogQL uses backticks as alternative to double quotes to avoid escaping)
- [ ] Test: empty string `|= ""`
- [ ] Test: string with only whitespace `|= " "`

### 1.4 Duration parsing edge cases
**Source:** Loki troubleshooting docs — "Use valid duration units — Use ns, us, ms, s, m, h, d, w, y, for example, 5m not 5minutes"
**Problem:** `[5m]` vs `[5min]` vs `[5]` vs `[0s]` vs `[-5m]` — every one of these is a real user mistake.
**Check:**
- [ ] Reject invalid duration units with a suggestion: "expected one of s, m, h, d, w; got 'min'"
- [ ] Reject zero and negative durations in range aggregations
- [ ] Test compound durations if supported: `1h30m`
- [ ] Test very large durations: `[90d]` — does this cause integer overflow anywhere?

### 1.5 Label name sanitization
**Source:** Loki docs — "Extracted label keys are automatically sanitized to follow Prometheus convention (ASCII letters, digits, underscores, colons; cannot start with a digit)"
**Problem:** `| json` on a line like `{"2xx-count": 5, "response.time": 0.3}` produces labels that violate Prometheus naming. Loki silently sanitizes; you need to decide what to do.
**Check:**
- [ ] Define and test sanitization rules for extracted labels
- [ ] Test label names starting with digits, containing hyphens, dots, spaces
- [ ] Test collision: original label `status` exists, JSON extraction also produces `status` — does `status_extracted` suffix work?

### 1.6 Metric query nesting
**Source:** LogQL BNF — metric queries can nest: `topk(5, sum by (service) (rate({...} [5m])))`
**Problem:** Nested aggregations are where recursive descent parsers get tricky. Left-paren counting, function-name lookahead, and nested `by`/`without` clauses all have to work together.
**Check:**
- [ ] Test: `topk(5, sum by (service) (count_over_time({...} |= "error" [5m])))`
- [ ] Test: `sum(rate({...} [5m])) / sum(rate({...} [5m]))` — binary operations between metric queries (if supported)
- [ ] Test: wrong nesting `rate(sum(...))` — should error clearly

### 1.7 Alias substitution before parsing
**Source:** Bible §6.8, §9.3
**Problem:** `@plogs or @atlas-conf |= "milvus"` — aliases substitute before parsing. If the substituted string has unbalanced braces or syntax errors, the error message will point to positions in the *expanded* string, not the alias name.
**Check:**
- [ ] Error messages for expanded aliases show both the alias name and the expansion
- [ ] Test: alias contains a syntax error — does the error message mention `@plogs`?
- [ ] Test: alias used in wrong context (LogQL alias in verb command) — clear error per §6.8
- [ ] Test: alias that expands to something containing `@` — must not chain (v1 limit)

### 1.8 The `map` stage `$field$` interpolation
**Source:** Bible §9.8
**Problem:** `$field$` with a value containing LogQL special characters (`"`, `{`, `}`, `|`) will inject those into the sub-query. This is essentially a LogQL injection vulnerability.
**Check:**
- [ ] `$field$` values are escaped/quoted before injection into the sub-query
- [ ] Test: field value containing `"` — does it break the sub-query string?
- [ ] Test: field value containing `}` — does it close the `map` block prematurely?
- [ ] Test: field value that is empty string
- [ ] Test: field name that doesn't exist in the parent record — clear error, not silent empty

---

## 2. STREAMING PIPELINE (src/exec/)

### 2.1 Unbounded channel memory growth
**Source:** "Tokio Backpressure: The One Mistake That Almost Killed Our Rust Data Pipeline" (Medium, 2025); Rust forum discussions on tokio::sync::mpsc
**Problem:** If a source reader produces records faster than the pipeline can consume them (e.g., reading 500MB of logs while a `| json` stage parses slowly), unbounded channels will eat all memory.
**Check:**
- [ ] All inter-stage channels are bounded (tokio::sync::mpsc::channel with explicit capacity)
- [ ] Backpressure propagates: if the consumer is slow, the producer slows down (bounded send blocks)
- [ ] Test with a large log file (100MB+) and a slow pipeline stage — memory stays bounded
- [ ] `--follow` mode doesn't accumulate records when the terminal can't scroll fast enough

### 2.2 Streaming cancellation
**Source:** Viacheslav Biriukov, "Async Rust with Tokio I/O Streams" (2025)
**Problem:** When a user hits Ctrl+C during a streaming query, all in-flight SSH commands must be cancelled. If any task is blocked on `write_all()` inside a `select!`, the cancellation token won't fire.
**Check:**
- [ ] Ctrl+C during `--follow` cleanly stops all remote commands and closes SSH sessions
- [ ] Ctrl+C during a `map` stage with parallel sub-queries cancels all sub-queries
- [ ] No zombie SSH processes left after Ctrl+C (check `ps aux | grep ssh` after a cancelled command)
- [ ] Graceful shutdown: partial results are printed before exit, not silently dropped

### 2.3 Record ordering in multi-source queries
**Source:** Bible §9.9 — "map output ordering is not stable across runs"
**Problem:** When merging streams from multiple sources (`or`-union), records arrive in arbitrary order. If the user expects time-sorted output, they'll be confused.
**Check:**
- [ ] Default human output for multi-source queries sorts by timestamp (if available)
- [ ] `--json` output does NOT sort (streaming, let the consumer sort) — document this
- [ ] Test: two sources with interleaved timestamps — verify merge order

### 2.4 The `| json` stage on non-JSON lines
**Source:** Grafana issue #50901 — "LogQL JSON parser shows JSONParserErr for ALL messages if one error present"
**Problem:** Real log streams are mixed: some lines are JSON, some aren't (startup messages, stack traces, plain text errors). `| json` must not poison the entire stream when one line fails to parse.
**Check:**
- [ ] `| json` on a non-JSON line adds an `__error__` label but does NOT drop the line
- [ ] `| json` on a partially-valid JSON line (truncated) — same behavior
- [ ] Mixed streams (10 JSON lines, 1 plain text line, 10 more JSON lines) — all 21 pass through
- [ ] The `__error__` label is filterable: `| json | __error__ = ""` keeps only clean parses

### 2.5 Filter pushdown correctness
**Source:** Loki issue #16653 — "performance drop due to incorrect LogQL type parsing"
**Problem:** When pushing `|= "error"` to remote `rg`, the remote command must handle the same escaping the local parser would. Special regex characters in a fixed-string filter (`|= "foo.bar"`) must not be treated as regex on the remote.
**Check:**
- [ ] `|= "foo.bar"` pushed to remote uses `rg --fixed-strings`, not regex
- [ ] `|~ "foo.bar"` pushed to remote uses `rg` regex mode
- [ ] `|= "foo\"bar"` — the escaped quote reaches the remote command correctly
- [ ] Remote `grep` fallback handles the same escaping (grep -F for fixed string)

---

## 3. SSH & CONTROLMASTER (src/ssh/)

### 3.1 Stale control sockets
**Source:** Ansible issue #17935; Red Hat Bugzilla #706396; OpenSSH multiplexing cookbook
**Problem:** If the master process dies (OOM, kill -9, network drop), the control socket file remains but is dead. New connections via the socket fail with "mux_client_read_packet: read header failed: Broken pipe."
**Check:**
- [ ] On connection failure via existing socket, detect stale socket, remove it, and retry with a fresh connection
- [ ] `inspect connections` shows status (alive/stale) per socket
- [ ] `inspect connect <ns>` when a stale socket exists: auto-cleans and reconnects
- [ ] Test: kill the SSH master process manually, then run `inspect status <ns>` — should recover

### 3.2 Socket permissions
**Source:** SSH documentation universally warns about this
**Problem:** SSH refuses to use a control socket that has wrong permissions. If the socket dir is world-writable or the socket is readable by others, SSH rejects it silently or with a cryptic error.
**Check:**
- [ ] `~/.inspect/sockets/` directory is created with mode 700
- [ ] Each socket file is mode 600
- [ ] If permissions are wrong (e.g., copied from another user), error message explains what to fix
- [ ] Test on a shared filesystem (NFS, CIFS) where permissions behave differently

### 3.3 MaxSessions server-side limit
**Source:** OpenSSH docs — "MaxSessions specifies the maximum number of open sessions permitted per network connection. Default is 10."
**Problem:** When fanning out to 12 containers on one server, you might hit the server's MaxSessions limit. The 11th and 12th sessions silently fail or error cryptically.
**Check:**
- [ ] Detect MaxSessions exhaustion and either queue sessions or warn clearly
- [ ] Fleet operations with many containers per server: does the tool respect the limit?
- [ ] Document that `MaxSessions 10` on the server limits concurrent per-namespace operations

### 3.4 SSH keepalive and server-side idle timeout
**Source:** Ansible docs; "ServerAliveInterval" and "ClientAliveInterval" documentation
**Problem:** Servers often have `ClientAliveInterval` and `ClientAliveCountMax` set. A persistent connection that goes idle (no commands for 10 minutes) may be killed by the server. The ControlMaster dies, the socket goes stale.
**Check:**
- [ ] `inspect connect` sets `ServerAliveInterval` (e.g., 30s) on the master connection
- [ ] Test: connect, wait 15 minutes, run a command — does it still work or does it auto-reconnect?
- [ ] `inspect connections` shows last-used time per connection

### 3.5 ProxyJump and bastion hosts
**Source:** openssh crate documentation; real-world deployments almost always have bastions
**Problem:** Many production servers are behind a bastion/jumpbox. `ProxyJump` in `~/.ssh/config` must be respected. ControlMaster with ProxyJump has known edge cases.
**Check:**
- [ ] Servers behind a bastion work with `inspect connect`
- [ ] The user's `~/.ssh/config` ProxyJump/ProxyCommand is respected
- [ ] ControlPath with ProxyJump doesn't collide (the socket path must include enough uniqueness)

---

## 4. WRITE VERBS & SAFETY (src/verbs/write/, src/safety/)

### 4.1 The `sed -i` symlink race (CVE-2026-5958)
**Source:** CERT Polska, published April 2026 — literally this month
**Problem:** `sed -i` with `--follow-symlinks` has a TOCTOU race: it resolves the symlink, then opens the original path. An attacker can swap the symlink target between those two operations. This is a real CVE from April 2026.
**Check:**
- [ ] `inspect edit` does NOT use `--follow-symlinks` on the remote `sed`
- [ ] Alternatively: `inspect edit` reads the file, applies the sed expression locally, writes back via a temp file + rename — avoiding `sed -i` entirely
- [ ] Test: file that is a symlink — does `edit` follow it, refuse it, or warn?
- [ ] Document behavior for symlinks in `edit`

### 4.2 Atomic write failure modes
**Source:** General Unix wisdom; TLDP Secure Programming HOWTO §7.10
**Problem:** "Write to temp file, rename to target" is atomic *on the same filesystem*. If the temp file is on a different filesystem (e.g., `/tmp` vs the target's filesystem), rename fails with EXDEV.
**Check:**
- [ ] Temp file is created in the same directory as the target, not in `/tmp`
- [ ] Test: target on a filesystem where temp file creation fails (read-only mount, no space) — clean error
- [ ] File permissions of the new file match the original (rename preserves the temp file's permissions, not the original's)
- [ ] File ownership: if running as a different user, the new file may have wrong ownership

### 4.3 Snapshot before mutation
**Source:** Bible §8.2
**Problem:** The snapshot must be saved *before* the mutation, not after. If the snapshot save fails (disk full, permissions), the mutation must not proceed.
**Check:**
- [ ] Order of operations: fetch original → save snapshot → hash original → apply mutation → hash result → write audit log
- [ ] If snapshot save fails, mutation is aborted with a clear error
- [ ] Test: fill the local disk, then run `inspect edit ... --apply` — should fail before mutating

### 4.4 Large-fanout interlock bypasses
**Source:** Bible §8.2 point 7
**Problem:** `inspect edit '*/\*:/etc/\*' 's/old/new/' --apply` — if the glob resolves to 50 files across 10 servers, the >10 interlock should fire. But does it count files or targets? If the selector matches 3 servers × 4 services = 12, that's >10, but 1 server × 15 files is also >10.
**Check:**
- [ ] Define: is the interlock count based on (server × service) pairs, or on individual files?
- [ ] Test: exactly 10 targets — no interlock. 11 targets — interlock fires.
- [ ] `--yes-all` bypasses the interlock — verify it's not just `--yes`

### 4.5 Audit log corruption on concurrent writes
**Source:** General JSONL append-only design
**Problem:** If two inspect processes run `--apply` simultaneously, both append to the same audit log. Without file locking, the JSONL entries can interleave mid-line and corrupt the file.
**Check:**
- [ ] Audit log writes are atomic: either use file locking (flock) or write-then-rename per entry
- [ ] Test: two concurrent `inspect edit ... --apply` — audit log is valid JSONL after both complete
- [ ] Alternative: accept that entries may interleave if each is a single `write()` call (POSIX guarantees atomicity for writes ≤ PIPE_BUF, typically 4096 bytes)

---

## 5. OUTPUT & FORMATS (src/format/)

### 5.1 CSV injection
**Source:** OWASP "CSV Injection" (also called Formula Injection)
**Problem:** If a log line contains `=cmd|'/C calc'!A0`, and you output it as CSV, Excel will execute it as a formula when opened. Real log lines can contain anything.
**Check:**
- [ ] CSV output escapes cells starting with `=`, `+`, `-`, `@`, `\t`, `\r` by prefixing with a single quote or wrapping in quotes
- [ ] Or: document that CSV output is data, not for direct Excel import without review

### 5.2 Template injection in `--format`
**Source:** Go template injection; tera/handlebars SSTI
**Problem:** If the `--format` template is user-controlled (it is — it's a flag), and the template engine supports function calls, a malicious template could access things it shouldn't. Less of a concern since the user is already trusted, but if templates end up in recipes...
**Check:**
- [ ] Template engine sandboxed: no filesystem access, no exec, no env var reading from within templates
- [ ] If using `tera`: ensure autoescaping is configured correctly
- [ ] Recipe-defined `--format` templates don't escalate trust

### 5.3 Unicode width in table rendering
**Source:** Widespread CLI issue; comfy-table handles this but edge cases remain
**Problem:** CJK characters are double-width, emoji are double-width, combining characters are zero-width. A table column header that's 10 chars ASCII might be 5 chars CJK but 10 columns wide. Log lines from non-English services will break table alignment.
**Check:**
- [ ] Table renderer uses unicode-width crate (or comfy-table's built-in) for column width calculation
- [ ] Test: log line containing CJK characters, emoji, RTL text — table alignment still correct
- [ ] `--raw` and `--json` are immune (no alignment needed)

### 5.4 Streaming output + SUMMARY/DATA/NEXT
**Source:** Bible §10.3 — "For streaming commands, each record is one JSON line; the envelope wraps the final summary after the stream ends"
**Problem:** If the stream is interrupted (Ctrl+C, SSH drop), the SUMMARY and NEXT are never emitted. A script consuming `--json` that expects the envelope will fail.
**Check:**
- [ ] Document: streaming `--json` emits records only; envelope is best-effort at end
- [ ] Scripts should handle missing envelope gracefully
- [ ] On clean Ctrl+C (SIGINT), attempt to emit a partial summary before exit

---

## 6. DISCOVERY & PROFILES (src/discovery/, src/profile/)

### 6.1 Docker socket permissions
**Source:** Universal Docker issue
**Problem:** `docker ps` requires the user to be in the `docker` group or have sudo. If neither, discovery fails. The error from `docker` is "permission denied" which doesn't tell the user how to fix it.
**Check:**
- [ ] Detect "permission denied" from docker and suggest: "add user to docker group or configure sudo"
- [ ] If discovery partially succeeds (some commands work, docker doesn't), show what was found + what failed

### 6.2 Non-standard Docker configurations
**Source:** Real-world deployments
**Problem:** Docker-in-Docker, rootless Docker, Podman pretending to be Docker, Docker with a custom socket path, Docker Compose v1 vs v2, Swarm mode. Each has different CLI behavior.
**Check:**
- [ ] Test: `docker` command not found — clear error, not a panic
- [ ] Test: Podman with docker CLI alias — does `docker ps --format json` still work?
- [ ] Test: Docker Compose v2 (`docker compose` vs `docker-compose`) — does it matter for discovery?
- [ ] Custom Docker socket: is `DOCKER_HOST` respected on the remote?

### 6.3 Profile cache staleness beyond drift
**Source:** Bible §5.2
**Problem:** Drift check compares container set. But services can restart with the same container names but different configs, different images, different port mappings. The drift check says "same" but the reality changed.
**Check:**
- [ ] Drift check compares container IDs (not just names)
- [ ] Test: `docker restart pulse` — drift check detects the new container ID
- [ ] Test: pull a new image, recreate the container — drift check catches the image change

---

## 7. SELECTORS (src/selector/)

### 7.1 Glob vs regex ambiguity
**Source:** General CLI design
**Problem:** `'milvus-*'` is a glob; `'/milvus-\d+/'` is a regex. What about `'milvus-[0-9]'`? That's valid as both a glob and a regex, with potentially different semantics.
**Check:**
- [ ] Document: slash-delimited = regex, everything else = glob
- [ ] Test: `'milvus-[0-9]'` as a glob — does `[0-9]` work as character class in globset?
- [ ] Test: a service name that contains regex metacharacters (e.g., `my.service`) — does the dot match everything as a glob?

### 7.2 The `_` host-level placeholder
**Source:** Bible §6.3
**Problem:** What if someone names a container `_`? The selector grammar would be ambiguous.
**Check:**
- [ ] Document: `_` is reserved; if a container is named `_`, it's unreachable via the short name
- [ ] Discovery warns if a container is named `_`

### 7.3 Comma in service names
**Source:** Docker allows almost any container name
**Problem:** If a container is named `pulse,atlas` (with a literal comma), the selector `arte/pulse,atlas` is ambiguous: is it two services or one?
**Check:**
- [ ] Document: comma is a separator in selectors; service names containing commas must be quoted or escaped
- [ ] Discovery warns about service names containing selector-special characters (`,`, `/`, `:`, `*`, `~`)

---

## 8. GENERAL RUST

### 8.1 Unwrap/expect in non-test code
**Problem:** `unwrap()` in production code is a panic waiting to happen. One unexpected `None` or `Err` and the whole binary crashes.
**Check:**
- [ ] `grep -rn 'unwrap()' src/ | grep -v test | grep -v '#\[test\]'` — review every instance
- [ ] `grep -rn 'expect(' src/ | grep -v test` — same
- [ ] Replace with proper error propagation (`?` operator) or explicit error handling

### 8.2 String allocation in hot paths
**Problem:** Lexer/parser that allocates a new `String` for every token will be slow on large inputs. The lexer should work with `&str` slices into the original input.
**Check:**
- [ ] Lexer tokens reference the original input via spans/ranges, not owned Strings
- [ ] Profile `inspect search` with a large result set (10k+ records) — check allocation count

### 8.3 Error type granularity
**Problem:** A single `InspectError` enum with 50 variants is unwieldy. Too-fine granularity is as bad as too-coarse.
**Check:**
- [ ] Error variants are grouped by module (SshError, ParseError, DiscoveryError, etc.)
- [ ] Each variant carries enough context for a helpful message
- [ ] No `Box<dyn Error>` in public interfaces (loses type info)

---

## Summary: Priority Order for Audit

**P0 — Will cause data loss or silent wrong results:**
- [ ] §4.1 — `sed -i` symlink race (CVE from this month!)
- [ ] §4.3 — Snapshot before mutation ordering
- [ ] §1.8 — `map` `$field$` injection
- [ ] §2.5 — Filter pushdown escaping correctness

**P1 — Will cause crashes or hangs in production use:**
- [ ] §3.1 — Stale SSH control sockets
- [ ] §2.1 — Unbounded channel memory growth
- [ ] §2.2 — Streaming cancellation on Ctrl+C
- [ ] §8.1 — Unwrap in non-test code

**P2 — Will confuse users or produce wrong output:**
- [ ] §1.3 — String escaping in lexer
- [ ] §1.7 — Alias substitution error messages
- [ ] §2.4 — `| json` on non-JSON lines
- [ ] §5.4 — Streaming output envelope

**P3 — Correctness and polish:**
- [ ] Everything else
