# inspect-cli — Field Notes & Feedback

Running log of what works, what hurts, and bugs found while using inspect-cli on the `arte` namespace. Append new entries with date.

---

## 2026-04-27 — first real-world session (knowledge-tab debug)

### Verdict
**Net positive.** It saved me from juggling SSH commands and the persistent ControlMaster is genuinely valuable (one passphrase, then I made ~15 calls without re-auth). Found the SSE 400 bug in ws-bridge → luminary-api → atlas chain in maybe 5 minutes; would have been comparable with raw ssh+docker but noisier.

### What helped
- **`inspect connect arte` once, then forget about auth.** Big quality-of-life win vs `eval $(ssh-agent) && ssh-add` dance.
- **Mux socket is reusable by stock `ssh`/`rsync`.** I confirmed `ssh -o ControlPath=/home/codespace/.inspect/sockets/arte.sock -o ControlMaster=no ubuntu@arte ...` works with no passphrase prompt. This is excellent — means inspect doesn't lock you into its own verbs.
- **`inspect ps arte`** clean tabular container list; faster to scan than `docker ps`.
- **`inspect setup arte`** discovery is fast (~5s) and produces a usable inventory.
- **`inspect test arte`** caught config issues before connect (key perms, TCP reachability).
- **Selectors** (`arte/<svc>`) are nice once you know which short names exist.

### What hurt / friction
1. **Phantom service names in profile.** `inspect resolve arte/api` says it resolved to `service=api`, but `inspect logs arte/api` then errors "No such container: api". The discovered profile has selectors that don't map to real container names. Workaround: use `inspect ps arte` to find the real names (`luminary-api`, not `api`).
2. **`inspect logs <selector>` silently hangs / is slow.** First `inspect logs arte/ws-bridge --since 5m` timed out at 30s with no output streamed; had to retry with `--tail 80` which worked. Need progress feedback when fetching a long backlog. Possibly because `--since` does a full container-time-range scan server-side?
3. **`inspect help <topic>` does not work for command names.** `inspect help add` errors "unknown help topic 'add'". You have to use `inspect add --help`. The `inspect help` topics only cover `quickstart`, `selectors`, etc. Misleading discoverability.
4. **`inspect exec` env dump leaked unrelated secrets.** Running `docker exec luminary-api env | grep -iE 'PORT|API_KEY'` faithfully showed `ANTHROPIC_API_KEY`, `TOOL_TAVILY_API_KEY`, `PULSE_API_KEY` plaintext in my terminal output, which then ended up in agent context. Not strictly inspect's fault — it's running my command — but a "secret-aware exec" mode that masks values matching an obvious `*_API_KEY=` pattern in stdout would be a great safety feature, since `inspect exec` is the natural fit for agents that may not know what envs are sensitive.
5. **`inspect exec` always dry-runs first.** Forced two-call pattern (`exec` → re-run with `--apply --allow-exec`) for *every* command, even read-only ones like `docker logs`. Understandable for write verbs, but `docker logs`/`cat`/`env` are read-only — a `--allow-read-only-exec` shortcut or auto-apply when the inner command pattern is read-only would cut friction. Right now I doubled my call count on every diagnostic.
6. **`inspect logs` filtering has no `--grep` / pattern flag.** I had to pipe `inspect exec arte --apply --allow-exec -- "docker logs ... | grep ..."` because `inspect logs --tail 200 | grep` outputs a structured record stream that grep-filters poorly across multiple lines per record. A `--match <regex>` or `--filter level=warn,err` flag would beat shelling out to docker directly.
7. **`docker inspect` warning during discovery.** `inspect setup` warned about a 30s timeout on `docker inspect` for a batch of containers. It didn't fail, but I can't tell what info I lost. Probably needs per-container fallback instead of batch.

### Bugs / oddities to file upstream
- Profile contains short-name selectors (`api`, `worker`, `pulse`, `backend`) that resolve but match no real container. These look like derived nicknames but are non-functional. Either: (a) actually map them to the right container in the resolver, or (b) drop them from the profile if no mapping exists.
- `inspect help <command>` should be a synonym for `<command> --help`, or list of help topics should include all commands.

### Was it net helpful vs ssh+docker?
**Yes, clearly.** Reasons:
- One passphrase per session is a big agent-friendliness win.
- Selector + tabular output is more grep-able than raw docker.
- The mux socket being reusable from stock ssh/rsync means I get the auth benefit without losing access to existing tooling (deploy.sh, etc.).

What I'd want next:
- Read-only exec mode that doesn't need `--apply --allow-exec`.
- Secret-masking on `exec` stdout.
- `--match` / `--grep` on `inspect logs`.
- Fix the phantom service selectors.

---

## 2026-04-27 — second session (production-grade embed pipeline + 413 retry diagnosis)

### Context
Iterating on a 413-loop bug in the knowledge embed pipeline. Doc kept failing on a single chunk. Used inspect-cli to watch worker logs across multiple deploy cycles and tail-with-filter for `embed-doc | 413 | split | contextualize | status_changed`. ~30 calls in this session.

### New wins
- **`inspect exec arte --apply --allow-exec -- "docker logs --since 3m luminary-worker 2>&1 | grep -iE '...' | tail -60"`** is now my muscle-memory pattern. Combined with the persistent mux it's <600ms per check. Faster than scrolling Temporal UI for one specific activity failure.
- **Output redirection inside the exec command works cleanly.** `2>&1` and pipes pass through fine.
- **Persistent mux survives across `./scripts/deploy.sh sync && build && restart`** even though deploy.sh runs its own ssh/rsync calls — the mux just gets reused, no extra auth prompts. This is the single biggest win.

### New friction (this session)
8. **No way to *follow* logs.** `inspect logs --follow` doesn't exist. For a "watch this activity in real time" workflow I'd want `inspect logs arte/worker --follow --match 'embed-doc|413'`. Right now I poll `docker logs --since 3m | tail` every 30-60s, which leaves a window where short-lived events disappear off the front.
9. **No multi-container log merge.** When debugging `ws-bridge → luminary-api → luminary-worker` chain, I want one merged time-sorted view across all three containers. Currently three separate calls and visual interleave by timestamp. `inspect logs arte/ws-bridge,api,worker --since 2m --merged` would be killer.
10. **Big stdout from `inspect exec` overflows agent context.** A 300-line `docker logs` returns ~16 KB of JSON-line records (most being `Registered tool: ...` boot noise). Some way to pre-filter server-side — `inspect exec --filter-line-pattern '<re>'` — would save round-tripping the noise. Today I work around with `| grep -iE` inside the exec command, but that requires me to know what to grep for *up front*.
11. **No structured exit code / status surfaced.** `inspect exec` returns the SUMMARY line "1 ok, 0 failed" but I can't tell what exit code the inner command returned. If `docker logs` succeeds and the pipe to `grep` returns 1 (no match), the exec is "ok" — fine — but for some workflows (e.g. "alert me if no matches") I'd want the inner exit code surfaced.
12. **No first-class "tail since last command" cursor.** Each call is `--since 3m` with the risk that something happened between calls but outside the window, or with the pain of expanding the window and re-pulling overlap. A `inspect logs arte/worker --since-last` that remembers per-namespace per-container last-fetch timestamp would let me poll efficiently without dupes or gaps.
13. **`--apply --allow-exec` is two flags for the same intent.** Why both? Either is sufficient as a "yes, really run it" gate. If `--allow-exec` is the unlock and `--apply` is the commit, that's an N+1 confusion vs `terraform apply -auto-approve` (one flag).
14. **No way to attach a "reason" / audit comment to exec.** I'd love `inspect exec arte --reason 'debugging 413 loop in embed pipeline' --apply --allow-exec -- ...` and have that appear in `inspect audit ls`. Right now audit shows the verb and command but not why I ran it. For agent traces this is gold.

### What I'd want next (cumulative)
1. Read-only exec auto-apply (#5 from session 1) — or auto-detect read-only inner cmd.
2. Secret-masking on `exec` stdout (#4).
3. `--match` / `--grep` on `inspect logs` (#6).
4. Fix phantom service selectors (#1).
5. **`inspect logs --follow`** for real-time tailing (#8). [HIGH VALUE]
6. **`inspect logs --merged` across multiple selectors** (#9). [HIGH VALUE]
7. Server-side line filter on `inspect exec` (#10).
8. Inner-command exit code surfacing (#11).
9. **`--since-last` cursor** for incremental polling (#12). [HIGH VALUE FOR AGENTS]
10. Collapse `--apply --allow-exec` into one flag (#13).
11. `--reason` / audit comments on exec (#14).

### Verdict (cumulative)
Now ~60+ calls into using this in anger. Still a clear net positive. The persistent mux + namespace abstraction + clean tabular `ps` is the core value prop and it holds up. The friction list is mostly missing power-user features, not broken core. If items 5/6/9 above shipped, this would be 10x more useful for agent-driven debugging — they're the difference between "wrapper around ssh+docker" and "actual SRE tool."

