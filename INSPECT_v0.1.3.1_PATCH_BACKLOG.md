# inspect v0.1.3.1 — patch backlog (accumulating)

Live patch backlog for items surfaced **after** the v0.1.3 tag was cut
(2026-05-10). Items here are bug-class fixes, security-adjacent gaps, and
small ergonomic regressions that don't fit the "no new features" v0.1.5
stabilization slot but also shouldn't wait for the v0.1.4 Kubernetes cycle.

Ship cadence: when ~3-5 items are ready and validated against a real host,
cut a `v0.1.3.1` tag off `main`. No feature growth — only fixes,
redactions, hardening, and small ergonomics that close trap classes.

Status legend: `🟦 Open` · `🟧 In progress` · `✅ Done` · `🟥 Bumped` (to a
later release).

---

## P1 — env-var-with-key-suffix redaction matcher

| Field | Value |
|---|---|
| **ID** | P1 |
| **Status** | 🟦 Open |
| **Priority** | HIGH (security-adjacent) |
| **Source** | First v0.1.3 agentic field user, 2026-05-10 audit session |
| **Surfaced via** | `docker inspect <ctr> --format '{{json .Config.Env}}'` returned `TEMPORAL_MCP_API_KEY=lum_...` **unredacted** in `inspect run` stdout. |

### Problem

The L7 four-masker family (header / PEM / URL credential / env-prefix-via-`-e KEY=`) does not match env-var assignments emitted by `docker inspect ... .Config.Env`, which arrive as bare `KEY=value` tokens inside a JSON array. The exact shape that slipped through:

```text
["TEMPORAL_MCP_API_KEY=lum_xxxxxxxx", "OTHER=plain", ...]
```

A less attentive operator would have pasted this into a doc / chat / ticket. The user (an LLM agent) flagged it, but the masker should have caught it.

### Proposed fix

Add a fifth masker to `src/redact/` matching env-assignment shapes where the **key suffix** signals secret material, case-insensitive:

- `_KEY` / `_API_KEY`
- `_TOKEN`
- `_SECRET`
- `_PASSWORD` / `_PASSPHRASE`
- `_CREDENTIAL` / `_CREDENTIALS`

Match contexts:

1. Bare `KEY=value` inside JSON arrays (the `docker inspect .Config.Env` shape).
2. `export KEY=value` lines in shell output.
3. `KEY=value` on its own line (env-file dump shape).

Mask the value with the existing redaction sentinel; key stays visible (it's the secret-class signal). Fires across:

- `run` / `exec` / `compose exec` stdout streams (the chokepoint that surfaced the bug).
- Audit log `rendered_cmd` and any captured-output field that touches stdout.
- `cat` / `grep` / `find` output where the file content matches the shape.

### Acceptance

- New tests in `tests/phase_p_v0131.rs` (new file): `p1_env_suffix_key_masked_in_docker_inspect_env`, `p1_env_suffix_token_in_export_line_masked`, `p1_env_suffix_case_insensitive`, `p1_env_suffix_no_match_on_plain_key`, `p1_env_suffix_masking_audited_in_rendered_cmd`.
- Help-search index byte cap auto-bumps if needed (precedent in `src/help/search.rs`).
- `inspect help safety` topic gets a one-paragraph addendum listing the new suffix family.
- CHANGELOG entry under a new `[0.1.3.1]` section above `[0.1.3]`.

### Notes

The existing `--env KEY=value` flag (F12) is *separately* redacted at the audit-args bracket layer; that path is fine. P1 is specifically about secrets that arrive in **stdout** of remote commands, not secrets the operator hands to inspect.

---

## P2 — `compose ls --refresh` should write-through to the cache

| Field | Value |
|---|---|
| **ID** | P2 |
| **Status** | 🟦 Open |
| **Priority** | HIGH (bug class) |
| **Source** | First v0.1.3 agentic field user, 2026-05-10 audit session |
| **Surfaced via** | `inspect compose ls arte --refresh` returned 7 projects; immediate follow-up `inspect compose ps arte/luminary-atlas` errored with "no such service — run `inspect setup arte`". |

### Problem

`--refresh` on `compose ls` performs live discovery (paying the round-trip cost) but does not persist results into the namespace cache. The very next verb that needs the project list re-discovers from the cold cache and fails. The error message redirects to `setup`, which is correct but ignores that the operator just paid the discovery cost.

### Proposed fix

Make `--refresh` write-through:

1. After live discovery succeeds, write the result to the namespace's cache file (same path `setup` uses).
2. No new flag needed — `--refresh` was always supposed to mean "refresh the cache", not "list once and discard".
3. Document the write-through in the `LONG_COMPOSE_LS` clap doc + `docs/MANUAL.md` compose section.

### Acceptance

- Test: `p2_compose_ls_refresh_persists_so_next_verb_finds_services` — mock-driven, asserts cache file mtime updated after `--refresh` and a follow-up `compose ps <ns>/<svc>` resolves without re-discovery.
- Test: `p2_compose_ls_no_refresh_does_not_touch_cache` — `--refresh`-less call must remain a pure read.
- CHANGELOG entry.

### Open question

Should the write-through extend to `setup --discover --json` and any other live-probe verb that ignores the cache today? Likely yes — same trap class, same fix shape — but scope it to `compose ls` first to avoid a bigger sweep mid-patch.

---

## P3 — verify `inspect run --json` per-step envelope

| Field | Value |
|---|---|
| **ID** | P3 |
| **Status** | 🟦 Open (verification first) |
| **Priority** | MEDIUM (ergonomics; may already exist) |
| **Source** | First v0.1.3 agentic field user, 2026-05-10 audit session |
| **Surfaced via** | Cross-fleet `inspect run` of a `for d in dir1 dir2 …; do …; done` glob produced human-readable line-prefixed output (`arte | …`) but no per-target structured JSON. User wanted `{server, exit_code, stdout, stderr}` per dispatch to feed the next pipeline stage. |

### Investigation step (do this first)

Before deciding scope, audit what `inspect run --json` actually emits today:

- Does the F13 summary envelope include per-step results, or only aggregate `ok` / `failed` / `failure_class`?
- Does `--stream --json` emit per-line NDJSON with target attribution? (The line-prefixed `arte |` in human output suggests target attribution exists; whether it's in the JSON path needs checking.)

If the per-step shape already exists, this becomes a **doc / discoverability fix** (point users at it from `LONG_RUN`'s SELECTING block) rather than a behavior change.

### If the shape doesn't exist

Add a per-step envelope phase to `--json` mode:

```json
{"phase": "step", "server": "arte", "selector": "arte/api", "exit_code": 0, "stdout": "...", "stderr": ""}
{"phase": "step", "server": "arte", "selector": "arte/web", "exit_code": 1, "stdout": "", "stderr": "..."}
{"phase": "summary", "ok": 1, "failed": 1, "failure_class": null}
```

Backward-compatible because the existing summary envelope keeps its shape; per-step phases are *added* before it.

### Acceptance

- If existing: a doc commit pointing at the existing path; no test churn.
- If new: tests `p3_run_json_emits_per_step_envelope`, `p3_run_json_per_step_carries_exit_code_and_streams`, `p3_run_json_summary_still_terminal`. CHANGELOG entry.

---

## P4 — defer to v0.2.0+: `inspect deps <ns> --include-listeners`

| Field | Value |
|---|---|
| **ID** | P4 |
| **Status** | 🟥 Bumped (v0.2.0+ feature, not a patch item) |
| **Source** | First v0.1.3 agentic field user, 2026-05-10 audit session |

User's own framing: "obviously not something to add" — listed here only so it doesn't get lost. A `deps` verb that joins `compose config` declared ports + actual `ss -tnlp` listeners + container env into structured topology output would be a real unlock for k8s migration audits, but it is feature growth and belongs in the v0.2.0 contract-design conversation, not the v0.1.3.x patch lane.

Move to `archives/v0.1.3/` as a forward-looking note when this file gets archived after the patch ships.

---

## Workflow

When a new field-feedback item arrives:

1. Add it as `P<N+1>` with the table header above (Status / Priority / Source / Surfaced via).
2. Write the **Problem** section in the operator's voice. Quote them where useful.
3. Sketch a **Proposed fix** with concrete module names + acceptance tests.
4. Tag a **Priority** (HIGH security-adjacent / HIGH bug class / MEDIUM ergonomics / LOW polish).
5. If the item is feature growth, mark it `🟥 Bumped` and explain *which* release it belongs in, the same way P4 does.

When ~3-5 items reach `✅ Done`:

1. Run the standard pre-commit gate per `CLAUDE.md`.
2. Bump `Cargo.toml` to `0.1.3.1`.
3. Add a `[0.1.3.1] — <date>` section to `CHANGELOG.md` above `[0.1.3]`.
4. Tag and push, no PR-to-`main` ceremony if `main` is the development branch.
5. Move this file to `archives/v0.1.3/INSPECT_v0.1.3.1_PATCH_BACKLOG.md` and start a fresh `INSPECT_v0.1.3.2_PATCH_BACKLOG.md` if more items accumulate before v0.1.4 ships.

---

## Closed items

(none yet)
