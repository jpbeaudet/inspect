# inspect v0.1.4 — Kubernetes-medium deep audit (2026-07-06)

**Status:** COMPLETE — fan-out returned, findings verified against code, synthesis below.
**Surface:** the v0.1.4 Kubernetes runtime medium (git range `main..feat/v0.1.4-program`, 63 commits).
**Methodology:** [`/luminary/docs/audits/CROSS_REPO_AUDIT_TEMPLATE.md`](../../../docs/audits/CROSS_REPO_AUDIT_TEMPLATE.md) — the Luminary substrate bar, adapted to a single-operator OSS CLI (Dimension **S** maps to operator secret/credential/kubeconfig blindness at the output boundary rather than multi-tenant IP).
**Exit-gate rule (JP-2026-07-06):** v0.1.4 does not tag/release until this audit reaches **0 Critical / 0 High** (fix → re-audit until only pedantic Med/Low remain).

**Verdict: maturity bar NOT met — 0 Critical, 5 High. This is a fix-then-re-audit surface (expected for a new medium's first deep pass).** Every High is grounded in a file:line and independently verified by the coordinator against code.

---

## §4 — Closing synthesis

### Summary table (most-severe first)

| ID | Dim/Surface | Sev | One-line | Disposition |
|----|-------------|-----|----------|-------------|
| **H1** (S1) | S — sovereign secret law | **High** | `describe_k8s` embeds raw `kubectl get pod -o json` into the envelope with no redaction → inline `env[].value` + `last-applied-configuration` leak in plaintext on stdout | ✅ **FIXED** — `scrub_pod_secrets` in `describe.rs`, tests `k11_scrub_*` |
| **H2** (G1) | Surface 1 — gaps | **High** | k8s **write** verbs K15–K19 (scale/rollout/delete/exec) ship with ~zero automated tests; plan §6 names 21 that don't exist | ✅ **FIXED** — 11 `k15_*`–`k19_*` acceptance tests in `phase_k_v014.rs` (docker-refusal, missing-target, exec `--no-revert` interlock, resolved-target dry-run echo); live apply+revert stays in the smoke (WD-1). G2 read-verb tests tracked as the remaining Medium |
| **H3** (O1/R3) | O + Surface 1 | **High** | All 5 write verbs echo/audit `k8s_namespace` via `unwrap_or("default")`; when unset, kubectl acts in the **context's** namespace → dry-run preview, confirm prompt, and AuditEntry can name a *different* namespace than the mutation lands in | ✅ **FIXED** — `effective_namespace()` resolves the real target (config→context-default→"default", offline); write verbs echo/audit + pin it via `kubectl_base_in`/`revert_kubectl_prefix_in`; tests `h3_effective_*` |
| **H4** (R1) | R — redundancy | **High** | Read verbs split into two JSON conventions in one release: `ps/status/describe/network/images/volumes/ports` emit the standard envelope; `top/events` (tabular) emit bare `json!` with no envelope and **silently drop `--select`** — the bare-NDJSON trap already extinguished in `34ae25d` | ✅ **FIXED** — `top`→`.data.pods[]`, `events`→`.data.events[]` via `render_doc(..select_spec)`. Per-verb adjudication: `cat/ls/grep/logs` were a false positive — they honor `--select` via the *streaming* `select_filter` (correct line-output design); `top`/`events` were the only two that dropped it |
| **H5** (C1) | C — capability-first | **High** | The load-bearing "orthogonal runtime axis behind a `Runtime` trait, mechanical kube-rs swap" invariant is **aspirational, not realized**: dispatch is a hand-copied `if runtime_kind()==K8s` fork in ~29 verbs; `K8sRuntime`/`runtime_for(K8s)` are used only in tests | ✅ **RESOLVED — Option B** (JP 2026-07-06): guard verified over-claim (k8s path works, nothing depends on the trait); corrected CLAUDE.md §Kubernetes-medium + plan Q2 + `runtime.rs` module/`K8sRuntime` docs; trait-as-seam refactor deferred v0.1.5+ |
| C2/C3/C4 | C | Med | Trait methods return command `String`s (kube-rs can't implement); failure taxonomy is kubectl-named not `Runtime::classify_failure`; config `type` is a hardcoded docker\|k8s fork | ✅ **RESOLVED via H5/Option B** — these are all symptoms of the trait-not-being-the-seam reality; JP ruled the per-verb fork is a sound design for two runtimes, so the fix was to make the docs honest (done in H5), not to re-shape the code. A future 3rd runtime / kube-rs swap revisits them as one refactor (deferred v0.1.5+) |
| G2 | Surface 1 | Med | Read verbs K9–K14 (`cat/ls/grep/why/describe/events/top/…`) have no `k<n>_*` acceptance tests | ✅ **FIXED** — k8s-only read verbs' contract edges pinned (`k11_describe_*`/`k12_events_*`/`k13_top_*` refusal + missing-target); `describe` scrub unit-tested (`k11_scrub_*`); happy-path output is smoke-covered (reads are non-destructive) |
| G3 | Surface 5 | Med | CHANGELOG v0.1.4 stops at K6 — ~17 shipped items undocumented | ✅ **FIXED** — grouped verbose entries for K7–K14 (read), K15–K20 (write), K21 (fleet), K24 (help), K25 (smoke) + the K22/K23 v0.1.5 deferral, under `[0.1.4] Added` |
| G4 | Surface 5 | Med | Plan marks Waves C/D/E ✅ and asserts the §6 "every K-item has a passing `k<n>_*` test" gate; the tree contradicts it (Class-1 doc-ahead-of-code) | ✅ **RESOLVED** — H2 (write) + G2 (read) landed the `k15_*`–`k19_*` + `k11_*`–`k13_*` acceptance tests; the §6 gate claim now holds for contract edges (happy-path via smoke) |
| R2 | R + Surface 1 | Med | `exec_k8s` drops `reason` + `duration_ms` from its AuditEntry — scaffold drift vs the other 4 write verbs | ✅ **FIXED** — exec_k8s now captures both (commit `fe5a763`) |
| R4 | R | Med | Dual-verb k8s branch-detection copy-pasted ~15× (near byte-identical, double-resolves); 7 k8s-only verbs use the opposite swallow-vs-propagate convention → one `k8s_route()` helper | Path A — fix-now |
| R6 | O | Med | Write verbs hardcode `deploy/`; StatefulSet/DaemonSet fail opaquely as `not_found` instead of the K20 REFUSE-with-hint idiom | ✅ **FIXED** — `deploy_write_hint()` turns a wrong-kind `not_found` into a chained hint naming the deploy-only conservative write set + the kubectl escape hatch (scale/rollout/restart); scale help + tests `r6_deploy_write_hint_*`. Actually *supporting* other kinds stays a v0.1.5+ scope decision |
| O2 | O | Med | k8s read `--request-timeout=5s` is a magic literal with no named-const home | ✅ **FIXED** — `READ_REQUEST_TIMEOUT` const in kubectl.rs; 7 read verbs + discovery use it (the `test` preflight keeps its distinct probe timeout) |
| W1/W2/W3 | Surface 2/3 + W | Low | builder/expander duplication; RBAC sequential spawns; scattered timeout literals — all cold-path hygiene | Path B — track |
| O3/O4 | O | Low | backlinked to K14/K7 | Path B — tracked |
| S2 | I | Low | `Unknown` failure hint says "see raw stderr below" but stderr is swallowed — dead-end pointer, no leak | Path A — trivial |

### Counts
- **Critical: 0**
- **High: 5** (H1 S1, H2 G1, H3 O1/R3, H4 R1, H5 C1)
- **Medium: ~10** (C2/C3/C4, G2/G3/G4, R2/R4/R6, O2)
- **Low: ~7** (W1/W2/W3, O3/O4, S2)
- **Maturity bar met?** **NO.** 5 Highs. Fix Path-A Highs (H1–H4) + Mediums, obtain JP decision on H5, re-run this audit; target 0-Crit/0-High before tag.

### Per-dimension roll-up
- **S (sovereign):** BROKEN by H1 (describe_k8s leak). Everything else clean — kubeconfig paths are config identifiers not credentials (never hit argv/audit/cache); `exec_k8s` redacts args+stdout and is fail-closed; write verbs dry-run-default with no secret in revert payloads; discovery caches only name/image/health; no `unwrap`/`expect`/`panic` on fallible k8s boundaries.
- **C (capability-first):** H5 — invariant aspirational. The *code shape* (per-verb fork) is a defensible design for two runtimes with heterogeneous per-verb behavior; the defect is that CLAUDE.md + plan Q2 **claim** the trait is the seam and promise a mechanical swap it does not deliver.
- **O (no-overfit):** H3 (namespace default overfit) + R6 (`deploy/`-only). WA-4 exit bands verified single-source (`KubectlFailure::exit_code()`), not scattered. Selector 2-segment grammar is plan-committed (exception 3, not a finding).
- **Perf/Weight:** PASS — no N+1, no pod-count-scaling hot path, no unbounded retention, no blocking-in-async. Three cold-path Lows.
- **Redundancy:** H4 + R2/R4 — real divergence across the ~19 parallel verb branches; definitive forms already exist in-tree to converge onto.

### Fix-coupling sequence (Path-A landing order)
1. **H1 (S1) describe redaction** — independent, highest-severity (active secret leak). Land first.
2. **H3 (O1/R3) namespace resolution** — fixing the `{context,ns,workload}` resolution into one struct (R4's `k8s_route`/resolved-target helper) *subsumes* the R3 mislabel and de-risks H2's write tests (they assert the audited namespace). Land the helper, then H3 rides it.
3. **R4 helper + R2 exec AuditEntry parity** — land with/after H3 (shared write-scaffold seam).
4. **H4 (R1) envelope convergence** — independent; converge `top`/`events` onto the envelope + restore `--select`. Adjudicate per-verb (line-streaming `cat`/`logs`/`grep` bare-NDJSON may be legitimate — verify against the CLAUDE.md NDJSON-vs-envelope note before converting).
5. **H2 (G1) + G2 write/read acceptance tests** — land LAST among Highs so the tests assert the *fixed* behavior (post-H1/H3), not the buggy one. This is the largest effort (~21 write tests + read coverage).
6. **Mediums/Lows:** R6 refuse-hint, O2 const, S2 hint, G3 CHANGELOG sweep — fold in.
7. **H5 (C1):** blocked on JP decision (below); the CLAUDE.md invariant text is corrected in the same change whichever way JP rules.

### Sovereignty statement
**BROKEN on this surface by H1**: `inspect describe <ns>/<pod> --json` emits inline pod-env secret literals + the `last-applied-configuration` annotation in plaintext across the stdout boundary. Once H1 lands, the full statement holds: *"no operator secret/credential/kubeconfig content crosses the audit/stdout/stderr/cache boundary in plaintext on the k8s surface."*

### The one escalation — H5 (C1), JP decision required
Both fix options are consequential and neither is the coordinator's to self-authorize:
- **Option A — make the trait the real seam:** add the methods verbs need to `Runtime` (`classify_failure`, a `Command`-returning pod/exec builder, inventory), route each verb's branch through `runtime_for(kind)`, delete the dead `K8sRuntime` string-impl. ~29-verb refactor.
- **Option B — correct the invariant:** amend CLAUDE.md "Kubernetes medium" + plan Q2 to describe what shipped (a per-verb `runtime_kind()` fork + a `kubectl` helper module; the `Runtime` trait as the docker-extraction byte-parity harness it actually is), and resolve dead `K8sRuntime` (remove, or keep as an explicitly JP-authorized v0.1.5 swap-seed deferral with a named unblock — a bare "later" is a Rule-5 finding).

Coordinator's read: the per-verb fork is a *fine design* for two runtimes; forcing a premature trait generalization for a hypothetical third backend is itself anti-overfit (Rule 8) risk. **Recommend Option B** (make the docs honest + resolve the dead code), with the trait-as-seam refactor deferred to v0.1.5 only if a third runtime actually appears. But rewriting a load-bearing CLAUDE.md architectural invariant is a JP call — surfaced, not decided.

---

## §5 — Methodology carry-forward (for the next surface's audit)
- **Echo-vs-command divergence trace** (caught H3): for any verb that both *displays/audits* a resolved target and *builds a command* from the same config, diff the fill-in defaults on each side — `unwrap_or(X)` on the display side vs `if let Some { … }` omission on the command side. A mismatch means the record lies.
- **Trait-seam reality check** (caught H5): a "behind a trait" claim is only real if `Box<dyn Trait>`/`factory(kind)` is called on the *dispatch* path, not just in tests. Grep the factory's real call sites; if they're all tests + one hardcoded default, the trait is a harness, not a seam.
- **Two-conventions-in-one-release check** (caught H4): when a feature adds N verbs of one kind, grep their output sites together — a subset emitting a *different* shape than the established one is drift, even if each compiles and "works."
- **Contract→(code,test) matrix** (caught H2/G2): build the full item→(file,test) table; "code landed" ≠ "acceptance tested." A one-commit-per-item history can still leave the highest-risk items (write verbs) untested.

---

## Fan-out provenance (read-only, one agent per slice)
Detailed per-slice findings in `docs/audits/_wip/` (gitignored scratch): `S-integrity.md`, `C-capability.md`, `O-overfit.md`, `gaps-coherence.md`, `perf-weight.md`, `R-redundancy.md`. This synthesis is the committed record; each High was re-verified against code by the coordinator (dispatch grep for H5/C1; describe.rs:91 for H1/S1; `grep 'fn k1[5-9]_'`→empty for H2/G1; `kubectl_base` `-n` conditional vs `scale.rs:40` echo for H3/O1; `cat.rs:282`/`top.rs` bare `json!` for H4/R1).
