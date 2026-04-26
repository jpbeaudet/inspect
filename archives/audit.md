Read [](file:///workspaces/inspect/INSPECT_FIELD_PITFALLS.md)

audit field pitfalls vs code

Read [](file:///workspaces/inspect/INSPECT_FIELD_PITFALLS.md)

Searched for regex `MaxSessions|administratively prohibited|looks_like_max_sessions`, 20 results

Searched for regex `ssh -O check|MasterStatus|check_master`, 20 results

Searched for regex `ControlPersist|ttl|set_file_mode`, 20 results

Searched for regex `json-file|journald|log_driver|fluentd|awslogs|none`, 20 results

Searched for regex `16384|16385|_parse_error|truncat`, 20 results

Searched for regex `shquote|shell_escape|shell escape`, 20 results

Searched for regex `allow.exec|--apply|audit.log`, 20 results

Searched for regex `map_stage|map_injection|container_name|validate`, 20 results

Searched for regex `com\.docker\.compose|service|label`, 20 results

Searched for regex `canary|timed.out|timeout|stream`, 20 results

Searched for regex `ulimit|RLIMIT|file.descriptor`, 7 results

Searched for regex `stdbuf|ssh.tt|-tt|line.buffer`, 13 results

Searched for regex `from_utf8_lossy|encoding|UTF-8|non-utf|binary`, 20 results

Searched for regex `inode|symlink|visited|loop|depth`, 20 results

Searched for regex `shell|distroless|probe|fallback`, 20 results

Searched for regex `max_line|line.length|truncate.*line`, 4 results

Searched for regex `offset|clock|skew|since|until`, 20 results

Searched for regex `systemd|unit|journalctl`, 20 results

Read [](file:///workspaces/inspect/audit.md)

Read [](file:///workspaces/inspect/src/exec/map_stage.rs#1-1), lines 1 to 80

Read [](file:///workspaces/inspect/src/exec/parsers.rs#1-1), lines 1 to 100

Read [](file:///workspaces/inspect/src/verbs/write/edit.rs#1-1), lines 1 to 100

Read [](file:///workspaces/inspect/src/exec/reader/logs.rs#1-1), lines 1 to 80

Read [](file:///workspaces/inspect/src/verbs/write/edit.rs#100-100), lines 100 to 150

Read [](file:///workspaces/inspect/src/exec/map_stage.rs#50-50), lines 50 to 70

Read [](file:///workspaces/inspect/src/discovery/probes.rs#1-1), lines 1 to 30

Read [](file:///workspaces/inspect/src/exec/reader/dir.rs)

Searched for text `log_driver` (`**/src/profile/**`), 4 results

Searched for text `compose` (`**/src/discovery/**`), no results

Read [](file:///workspaces/inspect/src/exec/map_stage.rs#90-90), lines 90 to 120

Read [](file:///workspaces/inspect/src/ssh/concurrency.rs#50-50), lines 50 to 140

Searched for text `com.docker`, 1 result

Searched for text `label` (`**/src/discovery/**`), no results

Read [](file:///workspaces/inspect/src/profile/schema.rs#1-1), lines 1 to 100

Searched for regex `docker|container|service|label` (`**/src/discovery/probes.rs`), 20 results

Read [](file:///workspaces/inspect/src/discovery/probes.rs#1-1), lines 1 to 200

Searched for text `parse_docker_inspect` (`**/src/discovery/probes.rs`), 3 results

Read [](file:///workspaces/inspect/src/discovery/probes.rs#250-250), lines 250 to 350

Read [](file:///workspaces/inspect/src/discovery/probes.rs#349-349), lines 349 to 450

Read [](file:///workspaces/inspect/src/safety/audit.rs#1-1), lines 1 to 100

Read [](file:///workspaces/inspect/src/verbs/write/exec.rs#1-1), lines 1 to 100

Read [](file:///workspaces/inspect/src/verbs/quote.rs)

Comprehensive audit complete. Findings against INSPECT_FIELD_PITFALLS.md:

## Status table

| § | Item | Status | Evidence |
|---|---|---|---|
| **1.1** | MaxSessions silent wall | 🟡 | concurrency.rs detects "administratively prohibited"; exec.rs emits actionable error. Per-host semaphore (default 8) queues. **Gap:** queue is invisible at discovery. |
| **1.2** | Stale control sockets | ✅ | master.rs `check_socket()` via `ssh -O check`; reuse path validates. **Minor:** no "Reconnected" notice. |
| **1.3** | ControlPersist TTL exposure | ✅ | ttl.rs + `--ttl` flag; paths.rs enforces mode 600. |
| **1.4** | Bulk SCP starves interactive | ❌ | cp.rs reuses multiplexed channel; no size-based warning or split connection. |
| **2.1** | No log rotation by default | ❌ | No file-size discovery in probes.rs; operators only learn from slow searches. |
| **2.2** | `docker logs -f` stops on truncate | ❌ | logs.rs has no inode/heartbeat reconnect — silent hang. |
| **2.3** | Non-json-file log drivers | 🟡 | Discovery records `log_driver`; logs.rs dispatches journald → `journalctl`. **Gap:** `fluentd`/`awslogs`/`none` return empty instead of clear error. |
| **2.4** | 16KB line truncation | ✅ | Already addressed by P0 work — parsers.rs sets `__error__` on parse failure; truncated JSON now surfaces instead of being dropped. |
| **3.1** | `edit` sed shell injection | ✅ | edit.rs applies sed **locally**, never on remote; new content pushed via base64 + `shquote`. |
| **3.2** | `exec` is dangerous | 🟡 | Audit-logged + `--apply` gated. **Gap:** uses same `--apply` as predictable verbs; fanout threshold 10, not lower. |
| **3.3** | `map $field$` injection | ✅ | map_stage.rs substitutes only into LogQL AST quoted-string position with `"`/`\` escaping; never reaches shell unescaped. |
| **3.4** | Audit log tampering | ❌ | audit.rs is plain JSONL (mode 600 + flock); no hash chain or external forwarding. **Acknowledged limitation; honest doc would suffice.** |
| **4.1** | One slow server blocks batch | ❌ | Pipeline collects then renders; no per-server streaming or per-host timeout. Highest field-pain item. |
| **4.2** | Too many open files | ❌ | No `RLIMIT_NOFILE` check; `INSPECT_FLEET_CONCURRENCY × per-host fds` can blow ulimit silently. |
| **4.3** | Partial fleet failures | 🟡 | Per-target ok/bad summary already present; **gap:** no `--canary N` / `serial: 1` mode. |
| **5.1** | SSH channel buffering laggy | ❌ | No `ssh -tt` / `stdbuf -oL` for follow; users see bursty output. |
| **5.2** | Large search saturates SSH | 🟡 | `--tail` pushdown exists; **gap:** no auto-cap or backpressure on unbounded matches. |
| **5.3** | Clock skew across servers | ❌ | `--since`/`--until` pushed verbatim; no per-host offset recorded. |
| **6.1** | Compose service vs container name | ❌ | probes.rs uses `{{.Names}}`; `com.docker.compose.service` label ignored — UX regression vs stern. |
| **6.2** | Restart-loop ID changes | ✅ | drift.rs fingerprints `{{.ID}}\t{{.Image}}`; commands resolve by name at exec time. |
| **6.3** | Non-Docker services | 🟡 | systemd discovery exists; **gap:** no allowlist/filter for system units (dbus, cron, systemd-*) → noisy inventory. |
| **7.1** | Binary log data | ❌ | record.rs and renderers pass raw bytes; ANSI-injection / terminal corruption possible. |
| **7.2** | Non-UTF-8 encodings | ❌ | UTF-8 assumed; Latin-1/Shift-JIS → mojibake; no `--encoding` flag. |
| **7.3** | Containers with no shell | ❌ | All `docker exec` calls hard-code `sh -c`; distroless/scratch fail at runtime. |
| **7.4** | Symlink loops in `ls` | ❌ | dir.rs uses `ls -1Ap` without inode tracking or depth cap. |
| **7.5** | Very long log lines | ❌ | No human-output truncation; 100 KB lines blow up terminals and memory. |

## Summary
- **✅ Fully covered:** 5 (§1.2, §1.3, §2.4, §3.1, §3.3, §6.2)
- **🟡 Partial:** 6 (§1.1, §2.3, §3.2, §4.3, §5.2, §6.3)
- **❌ Missing:** 14

The audit you ran earlier (P0–P3 of INSPECT_AUDIT_CHECKLIST.md) covered the *plumbing* — parser correctness, signal handling, atomic writes, SSH semaphoring, encoding sanitization in extracted labels. The pitfalls doc surfaces a different layer: **what the operator experiences when a real fleet misbehaves.** Most of the green items here got greened by the previous P0–P1 work; most of the red items are field-experience gaps that the existing audit didn't target.

## Suggested fix order (Field-Pitfalls phases)

**FP-P0 — outage/safety bugs that silently break tools:**
1. §2.2 follow-mode reconnect (heartbeat + inode tracking) — silent hang on log rotation
2. §7.4 symlink-loop detection in `ls` — infinite recursion / OOM
3. §7.1 binary-data/ANSI sanitization on render — terminal injection
4. §7.5 long-line truncation in human output — terminal/memory blowup

**FP-P1 — field stability (this is where the doc says "test first"):**
5. §4.1 per-server streaming + per-host timeout
6. §4.2 ulimit check + auto-cap concurrency
7. §5.3 per-server clock-offset recording + `--since` adjustment
8. §6.1 `com.docker.compose.service` label as primary service name
9. §2.3 explicit error for unsupported log drivers (fluentd/awslogs/none)

**FP-P2 — UX polish operators will love:**
10. §5.1 `ssh -tt` / `stdbuf -oL` for follow mode
11. §2.1 log-file size discovery warning (>1 GB)
12. §6.3 system-unit filter on systemd discovery
13. §5.2 unbounded-match auto-cap with `--max` hint
14. §7.3 distroless probe + `docker cp` fallback
15. §7.2 lossy UTF-8 default + optional `--encoding`

**FP-P3 — depth-in-defence + nice-to-haves:**
16. §3.2 separate `--allow-exec` flag, lower fanout threshold for `exec`
17. §4.3 `--canary N` for fleet restart
18. §1.4 large-transfer warning + optional dedicated connection
19. §3.4 honest doc on audit-log limits + optional `inspect audit verify`
20. §1.1 proactive MaxSessions warning at discovery

Ready to execute **FP-P0** when you are.