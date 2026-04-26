# Inspect Audit Response

Legend: ✅ Covered · 🟡 Partial / harden · ❌ Missing

## §1 — Parser & Lexer

| # | Item | Status | Evidence / Action |
|---|------|--------|-------------------|
| 1.1 | Invalid-query negative tests + fuzzing | 🟡 | tests.rs, phase6_logql.rs cover good inputs + a few error envelopes. **Action:** add a negative-test matrix (unclosed `{`, dangling `\|=`, bad `map`, metric-of-metric) and a `cargo-fuzz` target asserting "errors only, no panic". |
| 1.2 | `and` > `or` precedence | ✅ | `continue_field_or` calls `continue_field_and` in parser.rs; test `field_filter_boolean` confirms grouping. |
| 1.3 | String escapes (`\"`, `\\`, `\n`, `\t`, `\r`) | ✅ | lexer.rs. **Harden:** no backtick string form yet (LogQL allows `` `…` `` for raw strings) — add if we want full LogQL parity; also add explicit tests for `""`, all-whitespace, and JSON-inside-filter strings. |
| 1.4 | Duration units / zero / negative | 🟡 | Two parsers: duration.rs (s/m/h/d) and lexer.rs (s/m/h/d/w). Negatives rejected; **`0s` is silently accepted**, no `y`, no compound (`1h30m`). **Action:** reject zero in range aggregations with a clear error; document accepted units. |
| 1.5 | Label name sanitization on `\| json` | ❌ | parsers.rs inserts raw JSON keys verbatim — `2xx-count`, `response.time` flow through. **Action:** add `sanitize_label_name()` (Prometheus rules) + collision policy (`name_extracted` suffix). |
| 1.6 | Metric query nesting | ✅ | Recursive `parse_metric_query()` (parser.rs); `topk_with_grouping` test exists. **Nice to have:** test for `rate(...) / rate(...)` and bad nesting `rate(sum(...))`. |
| 1.7 | Alias substitution error messages | 🟡 | alias_subst.rs names unknown alias and detects chaining, **but** when expansion is parsed and fails, the span points into the expanded text and the alias name is not surfaced. **Action:** wrap expansion errors with "in expansion of `@plogs`: …" and keep both spans. |
| 1.8 | `map $field$` injection | ✅ | map_stage.rs escapes `"` and `\` before substituting into the quoted sub-query. **Add tests:** value containing `}`, empty value, missing field. |

## §2 — Streaming Pipeline

| # | Item | Status | Evidence / Action |
|---|------|--------|-------------------|
| 2.1 | Bounded channels | ✅ | All sites use `tokio::sync::mpsc::channel(N)`; no `unbounded_channel` anywhere in src. |
| 2.2 | Ctrl+C cancellation / no zombie ssh | ❌ | No `tokio::signal`, no `CancellationToken`, no SIGINT handler anywhere in src. **Action (P1):** install `tokio::signal::ctrl_c()` in `main.rs`, propagate a `CancellationToken` through `exec::pipeline` and `ssh::exec`, send `ssh -O cancel`/SIGTERM to children on shutdown, and add a `ps aux \| grep ssh` post-test. |
| 2.3 | Multi-source merge ordering | ❌ | pipeline.rs does not k-way merge by timestamp. **Action:** for human/table output, k-way-merge on `__timestamp__`; for `--json` document interleaved order. |
| 2.4 | `\| json` on non-JSON lines | ❌ | parsers.rs silently returns — the line **survives** but no `__error__` label is added, so users can't filter or even see that parsing failed. **Action (P2):** set `rec.fields["__error__"] = "JSONParserErr"` (and similar for `logfmt`/`pattern`/`regexp`); add `phase7` tests for mixed streams. |
| 2.5 | Filter pushdown `-F` vs `-E` | ✅ | mod.rs emits `-F`/`-E` correctly and `shquote()`s the pattern. |

## §3 — SSH / ControlMaster

| # | Item | Status | Evidence / Action |
|---|------|--------|-------------------|
| 3.1 | Stale control sockets | ✅ | `MasterStatus::{Alive,Stale,Missing}` via `ssh -O check` in master.rs; `exit_master` removes the socket. **Verify:** confirm callers always retry on `Stale` (worth one explicit test killing the master with `kill -9`). |
| 3.2 | Socket dir/file perms 700/600 | ✅ | paths.rs `set_dir_mode_0700` + `set_file_mode_0600`; sockets dir is created via `ensure_sockets_dir`. **Harden:** assert mode 600 on the socket file itself after `ssh` creates it (OpenSSH already does this, but a defensive check + clear error helps NFS users). |
| 3.3 | `MaxSessions` exhaustion | ❌ | No detection / queueing. **Action:** add per-host semaphore (default 8) configurable via profile, plus stderr-pattern detection of "open failed: administratively prohibited". |
| 3.4 | `ServerAliveInterval` keepalive | ✅ | **Correction to subagent:** master.rs sets `ServerAliveInterval=30` and `ServerAliveCountMax=3` on the master. **Add:** `last_used` timestamp in `inspect connections`. |
| 3.5 | ProxyJump / bastion | ✅ | We delegate entirely to OpenSSH + `~/.ssh/config` — no reimplementation, so ProxyJump works automatically. **Verify:** `ControlPath` includes namespace hash so bastion+target collisions are impossible (already true in `paths.rs`). |

## §4 — Write verbs & Safety

| # | Item | Status | Evidence / Action |
|---|------|--------|-------------------|
| 4.1 | `sed -i` symlink race (CVE-2026-5958) | ✅ | edit.rs reads remote → applies `sed` **locally** → writes via temp+rename. We never invoke `sed -i --follow-symlinks`. **Add:** explicit symlink behaviour doc + a `realpath` check before writing (refuse to clobber if the symlink target changed since snapshot). |
| 4.2 | Atomic write (same FS) | ✅ | Temp lives in target dir: `format!("{}.inspect.{}.tmp", w.path, …)` (edit.rs, cp.rs). **Harden:** preserve mode/uid/gid of original (`stat` original → `chmod`/`chown` temp before rename); currently the new file inherits the temp's mode. |
| 4.3 | Snapshot before mutation | ✅ | Order verified in edit.rs: read → diff → snapshot → write → audit. |
| 4.4 | >10 fanout interlock | ✅ | gate.rs `fanout_threshold = 10`, `--yes-all` bypass. **Clarify in docs:** count = number of `(target, path)` work items, not server×service pairs. |
| 4.5 | Concurrent audit log writes | 🟡 | audit.rs opens with `O_APPEND` and a single `writeln!`. POSIX guarantees atomicity only ≤ `PIPE_BUF` (4096B). Long JSON entries (large diffs, many fields) can exceed this. **Action:** add `fs2::FileExt::lock_exclusive()` around the write, or buffer the entry and write with a single `write_all` call after `fcntl(F_SETLKW)`. |

## §5 — Output & Formats

| # | Item | Status | Evidence / Action |
|---|------|--------|-------------------|
| 5.1 | CSV formula injection | 🟡 | render.rs handles `,`, `"`, `\n`, `\r` but **not** leading `=`, `+`, `-`, `@`, `\t`. **Action:** prefix `'` (or wrap and prefix) when first char is one of those. |
| 5.2 | Template sandboxing | ✅ | template.rs is a small custom engine — no fs / exec / env access. |
| 5.3 | Unicode width in tables | 🟡 | render.rs uses `chars().count()` — CJK/emoji misaligned. **Action:** add `unicode-width = "0.1"`, replace with `UnicodeWidthStr::width`. |
| 5.4 | Streaming envelope on Ctrl+C | ❌ | No best-effort summary on SIGINT. **Action (ties to 2.2):** in the SIGINT handler, flush a `{"status":"cancelled","summary":{…}}` envelope before exit; document that streaming `--json` may end without an envelope. |

## §6 — Discovery & Profiles

| # | Item | Status | Evidence / Action |
|---|------|--------|-------------------|
| 6.1 | `docker` permission denied | ❌ | probes.rs propagates the raw stderr. **Action:** match `permission denied while trying to connect to the Docker daemon socket` and emit "add user to `docker` group, or run with `sudo`". |
| 6.2 | Podman / rootless / `DOCKER_HOST` / compose v2 | ❌ | Hard-coded `docker` binary. **Action:** probe `which docker \|\| which podman`, honour `DOCKER_HOST`, fall back to `docker compose` for v2 (only matters if we ever shell to compose — currently we don't, so this is mostly a discovery niceness). |
| 6.3 | Drift compares container IDs | ✅ | drift.rs fingerprints `{{.ID}}\t{{.Image}}` — restart and image-pull both invalidate the profile. |

## §7 — Selectors

| # | Item | Status | Evidence / Action |
|---|------|--------|-------------------|
| 7.1 | Glob vs regex (`/…/`) | ✅ | parser.rs `split_outside_regex` enforces slash delimiters. |
| 7.2 | `_` reserved as host placeholder | 🟡 | Parser treats `_` as host-level; no warning if a real container is named `_`. **Action:** add discovery warning + doc note. (Low risk in practice.) |
| 7.3 | Comma in service names | 🟡 | Parser splits unconditionally on `,`. **Action:** in discovery, warn when a container name contains `,`, `/`, `:`, `*`, or `~`. |

## §8 — General Rust

| # | Item | Status | Evidence / Action |
|---|------|--------|-------------------|
| 8.1 | `unwrap()`/`expect()` in non-test code | 🟡 | A handful (~5–6) in parser.rs and parser.rs; most are precondition-safe (after `bump()` checks), but worth a sweep. **Action:** `rg -n 'unwrap\(\)\|expect\(' src/ \| rg -v '#\[test\]'` and convert any non-trivial ones to `?`. |
| 8.2 | Lexer string allocation | 🟡 | lexer.rs builds owned `String` per token even when no escape happened. Fine for queries (small), bad for large JSON-pushed bodies. **Action:** switch to `Cow<'a, str>` returning a borrowed slice when no escape was applied. Defer until profiling shows it. |
| 8.3 | Error type granularity | ✅ | Per-module enums (`ConfigError`, `ParseError`, `SelectorParseError`); no `Box<dyn Error>` on the public surface. |

---

## Suggested fix order

**P0 (correctness / data integrity):**
1. §2.4 `\| json` `__error__` label (silent data loss today).
2. §4.5 audit-log `flock` (corruption under concurrency).
3. §1.5 label sanitization for `\| json` extraction.

**P1 (production stability):**
4. §2.2 + §5.4 SIGINT handling, cancellation token, partial envelope.
5. §3.3 per-host `MaxSessions` semaphore.
6. §8.1 unwrap sweep on parser/selector hot paths.

**P2 (UX / polish):**
7. §1.7 alias-expansion error wrapping.
8. §5.1 CSV formula-prefix escaping.
9. §5.3 `unicode-width` adoption.
10. §6.1/§6.2 Docker permission-denied / Podman fallback messages.
11. §1.4 reject `0s` in range aggregations.
12. §1.1 negative-test matrix + `cargo-fuzz` target.

**P3 (defence in depth):**
13. §4.2 preserve original mode/uid/gid on atomic rename.
14. §7.2/§7.3 discovery warnings for reserved/special names.
15. §8.2 `Cow`-based lexer tokens (only if profiling demands).

