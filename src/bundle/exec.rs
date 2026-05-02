//! Bundle planner + executor (B9, v0.1.2).
//!
//! Two entry points:
//!
//! * [`plan`] — validates structure, runs interpolation against the
//!   bundle's `vars:`, and prints the rendered step list. Never
//!   touches a remote.
//! * [`apply`] — runs preflight, executes steps in declaration order
//!   (parallel matrix entries fanned out with bounded concurrency),
//!   routes failures through `on_failure`, drives rollback when
//!   asked, and runs postflight on success.
//!
//! Audit:
//! Every `exec` and `watch` step appends one [`AuditEntry`] tagged
//! with the bundle's `bundle_id` (a fresh ULID per `inspect bundle
//! run` invocation) and the step's `id`. `run`-typed steps are
//! intentionally NOT audited (they're read-only by design — same
//! contract as `inspect run`).
//!
//! Failure semantics (clarified beyond what the YAML spec says):
//!
//! * `on_failure: abort` (default) — stop. No rollback. Bundle exits 2.
//! * `on_failure: continue` — log, proceed to next step.
//! * `on_failure: rollback` — run per-step `rollback:` for every
//!   completed reversible step in REVERSE declaration order, then run
//!   the bundle-level `rollback:` block. Bundle exits 2.
//! * `on_failure: rollback_to: <id>` — run per-step `rollback:` for
//!   completed reversible steps from the failed step back to BUT NOT
//!   INCLUDING `<id>`, in reverse order. Skip the bundle-level
//!   `rollback:` block. Bundle exits 2.
//!
//! If a rollback step itself fails the engine stops the rollback
//! loop and surfaces a loud `[inspect] rollback FAILED on step <id>:
//! ...` line on stderr — better to leave a half-rolled-back state
//! the operator can inspect than to silently keep going.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use chrono::Utc;

use crate::error::ExitKind;
use crate::safety::{AuditEntry, AuditStore};
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan as resolve_plan};
use crate::verbs::quote::shquote;
use crate::verbs::runtime::RemoteRunner;

use super::checks::{describe_check, run_check};
use super::schema::{Bundle, OnFailure, Step, StepBodyKind};
use super::vars::{interpolate, InterpError};

/// Default per-step timeout when the YAML doesn't override.
const DEFAULT_STEP_TIMEOUT_SECS: u64 = 300;

/// L6 (v0.1.3): per-branch execution outcome inside a `parallel:
/// true` + `matrix:` step. Recorded once per matrix value at the
/// time the branch finishes (whether ok, failed, or skipped under
/// stop-on-first-error). Used by [`do_rollback`] to invert ONLY
/// succeeded branches and by `inspect bundle status <id>` to render
/// the per-branch table.
#[derive(Debug, Clone)]
pub(crate) struct BranchResult {
    /// Display label for the branch — `<matrix-key>=<value>`. Stable
    /// across reruns of the same bundle so post-mortem queries can
    /// pivot on it.
    pub branch_id: String,
    /// Final status. `Ok` ⇒ inverse runs on rollback; `Failed` and
    /// `Skipped` ⇒ no inverse fires.
    pub status: BranchStatus,
    /// The matrix value as the operator wrote it. Threaded into
    /// rollback-block interpolation so `{{ matrix.<key> }}`
    /// resolves to this branch's value (not the full matrix).
    pub matrix_value: serde_yaml::Value,
    /// The matrix key (the YAML map's only key, given the v0.1.2
    /// validator caps `matrix:` at one entry). Same for every
    /// branch in the same step; carried per-branch so the rollback
    /// path doesn't need to thread the step pointer.
    pub matrix_key: String,
    // Note: the per-branch audit_id and duration_ms are recorded
    // directly on the audit entry (see `bundle_branch` /
    // `bundle_branch_status` in `AuditEntry`), so we don't need to
    // mirror them here. `inspect bundle status` reads them straight
    // from the audit log.
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BranchStatus {
    Ok,
    Failed,
    Skipped,
}

impl BranchStatus {
    fn as_audit_str(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

/// Knobs passed to [`apply`] from the CLI layer.
pub struct ApplyOpts {
    /// Operator passed `--apply`. Without it, every `exec`/`watch`/
    /// rollback step that doesn't carry its own `apply: false`
    /// is refused.
    pub apply: bool,
    /// CI mode: skip the Ctrl-C / failure-rollback prompt.
    pub no_prompt: bool,
}

/// Run plan-only mode: validate, interpolate, and print.
pub fn plan(bundle: &Bundle) -> Result<ExitKind> {
    validate_bundle(bundle)?;

    println!("# {}", bundle.name);
    if let Some(h) = &bundle.host {
        println!("host: {h}");
    }
    if let Some(r) = &bundle.reason {
        println!("reason: {r}");
    }

    if !bundle.preflight.is_empty() {
        println!("preflight:");
        for (i, c) in bundle.preflight.iter().enumerate() {
            println!("  [{}] {}", i + 1, describe_check(c));
        }
    }

    println!("steps:");
    let empty_matrix: BTreeMap<String, serde_yaml::Value> = BTreeMap::new();
    for (i, step) in bundle.steps.iter().enumerate() {
        let kind = step.body_kind().map_err(|e| anyhow!("plan: {e}"))?;
        let target = step
            .target
            .as_deref()
            .or(bundle.host.as_deref())
            .unwrap_or("(none)");

        if step.parallel && !step.matrix.is_empty() {
            let (mkey, mvals) = step
                .matrix
                .iter()
                .next()
                .ok_or_else(|| anyhow!("plan: step `{}` parallel without matrix", step.id))?;
            let cap = step
                .max_parallel
                .unwrap_or(mvals.len())
                .min(super::schema::MAX_PARALLEL_CAP);
            println!(
                "  [{n}] {id}  ({kind:?})  target={target}  matrix.{mkey}={vals}  max_parallel={cap}",
                n = i + 1,
                id = step.id,
                kind = kind,
                vals = mvals
                    .iter()
                    .map(|v| super::vars::yaml_to_str(v).unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join(",")
            );
            for v in mvals {
                let mut mtx = empty_matrix.clone();
                mtx.insert(mkey.clone(), v.clone());
                let rendered = render_step_body(step, kind, &bundle.vars, &mtx)?;
                println!(
                    "        - {mkey}={val}: {body}",
                    val = super::vars::yaml_to_str(v).unwrap_or_default(),
                    body = rendered
                );
            }
        } else {
            let rendered = render_step_body(step, kind, &bundle.vars, &empty_matrix)?;
            println!(
                "  [{n}] {id}  ({kind:?})  target={target}  on_failure={of}{apply}{rev}",
                n = i + 1,
                id = step.id,
                kind = kind,
                of = format_on_failure(&step.on_failure),
                apply = if step.apply { "" } else { "  apply=false" },
                rev = if step.reversible {
                    ""
                } else {
                    "  reversible=false"
                },
            );
            println!("        body: {rendered}");
            if let Some(rb) = &step.rollback {
                let rb_rendered = interpolate(rb, &bundle.vars, &empty_matrix)
                    .map_err(|e| anyhow!("step {} rollback: {e}", step.id))?;
                println!("        rollback: {rb_rendered}");
            }
        }
    }

    if !bundle.rollback.is_empty() {
        println!("rollback (reverse on `on_failure: rollback`):");
        for (i, step) in bundle.rollback.iter().enumerate() {
            let kind = step
                .body_kind()
                .map_err(|e| anyhow!("rollback step: {e}"))?;
            let rendered = render_step_body(step, kind, &bundle.vars, &empty_matrix)?;
            println!("  [{}] {id}: {rendered}", i + 1, id = step.id);
        }
    }

    if !bundle.postflight.is_empty() {
        println!("postflight:");
        for (i, c) in bundle.postflight.iter().enumerate() {
            println!("  [{}] {}", i + 1, describe_check(c));
        }
    }

    Ok(ExitKind::Success)
}

/// Run the bundle for real. Drives preflight → steps → postflight,
/// with rollback on failure.
pub fn apply(bundle: &Bundle, opts: ApplyOpts) -> Result<ExitKind> {
    validate_bundle(bundle)?;

    // --apply gate. Allow plan-shaped runs (no destructive steps) to
    // proceed without --apply.
    if !opts.apply && requires_apply(bundle) {
        crate::error::emit(
            "bundle has destructive steps and `--apply` was not passed. \
             Run `inspect bundle plan` to dry-run, or re-run with `--apply` to execute.",
        );
        return Ok(ExitKind::Error);
    }

    // The runner used for preflight/postflight checks. Per-step
    // execution re-resolves through `verbs::dispatch::plan` to pick up
    // container wrapping; that path also returns a runner instance
    // but we keep this one for preflight/postflight which run
    // bundle-level (no per-step container wrap).
    let runner = crate::verbs::runtime::current_runner();
    let bundle_id = mint_bundle_id();
    let started = Instant::now();
    eprintln!(
        "[inspect] bundle `{name}` id={id} starting ({n} step(s))",
        name = bundle.name,
        id = bundle_id,
        n = bundle.steps.len(),
    );

    // Preflight — abort before touching anything if any check fails.
    if !bundle.preflight.is_empty() {
        eprintln!("[inspect] preflight: {} check(s)", bundle.preflight.len());
        for (i, check) in bundle.preflight.iter().enumerate() {
            let res = run_check(&*runner, bundle.host.as_deref(), check)
                .with_context(|| format!("preflight check {}", i + 1))?;
            if res.passed {
                eprintln!("  ✓ {} — {}", res.label, res.detail);
            } else {
                eprintln!("  ✗ {} — {}", res.label, res.detail);
                crate::error::emit(format!(
                    "preflight failed: {} — {}. Bundle aborted before any step ran.",
                    res.label, res.detail
                ));
                return Ok(ExitKind::Error);
            }
        }
    }

    // Track which steps completed successfully, in declaration order,
    // so rollback can walk them in reverse.
    let mut completed: Vec<usize> = Vec::new();
    // L6 (v0.1.3): per-step matrix branch ledger. Indexed by step
    // declaration index. Populated only for `parallel: true` +
    // `matrix:` steps (whether they completed cleanly or failed
    // partway). [`do_rollback`] consults this map to invert ONLY the
    // succeeded branches with per-branch matrix interpolation.
    let mut step_branches: BTreeMap<usize, Vec<BranchResult>> = BTreeMap::new();
    let store = AuditStore::open().context("opening audit store")?;

    for (idx, step) in bundle.steps.iter().enumerate() {
        if crate::exec::cancel::is_cancelled() {
            eprintln!("[inspect] bundle cancelled by signal");
            do_rollback(
                &*runner,
                bundle,
                &bundle_id,
                &store,
                &completed,
                &step_branches,
                None,
                opts.no_prompt,
            );
            return Ok(ExitKind::Error);
        }

        // Validate `requires:` against completed set.
        for req in &step.requires {
            let req_ok = completed.iter().any(|&i| bundle.steps[i].id == *req);
            if !req_ok {
                crate::error::emit(format!(
                    "step `{}` requires `{}` which has not completed",
                    step.id, req
                ));
                return Ok(ExitKind::Error);
            }
        }

        let step_label = format!("{}/{}", idx + 1, bundle.steps.len());
        eprintln!("[inspect] step {step_label}: {id}", id = step.id);

        let outcome = run_step(&*runner, bundle, step, &bundle_id, &store, &opts);

        match outcome {
            Ok(StepOutcome::Single) => {
                completed.push(idx);
            }
            Ok(StepOutcome::Matrix(branches)) => {
                step_branches.insert(idx, branches);
                completed.push(idx);
            }
            Err(e) => {
                eprintln!(
                    "[inspect] step {step_label} `{id}` FAILED: {e}",
                    id = step.id
                );
                // L6 (v0.1.3): if the failure came from a parallel
                // matrix step, drain the per-branch ledger so
                // do_rollback can target succeeded branches only.
                if let Some(partial) = BranchFailureCarrier::drain() {
                    step_branches.insert(idx, partial);
                }
                match &step.on_failure {
                    OnFailure::Abort => {
                        eprintln!(
                            "[inspect] on_failure=abort — leaving completed steps in place. \
                             Inspect the audit log with `inspect audit ls --bundle {bundle_id}`."
                        );
                        return Ok(ExitKind::Error);
                    }
                    OnFailure::Continue => {
                        eprintln!("[inspect] on_failure=continue — proceeding");
                        continue;
                    }
                    OnFailure::Rollback => {
                        do_rollback(
                            &*runner,
                            bundle,
                            &bundle_id,
                            &store,
                            &completed,
                            &step_branches,
                            None,
                            opts.no_prompt,
                        );
                        return Ok(ExitKind::Error);
                    }
                    OnFailure::RollbackTo(target_id) => {
                        do_rollback(
                            &*runner,
                            bundle,
                            &bundle_id,
                            &store,
                            &completed,
                            &step_branches,
                            Some(target_id.as_str()),
                            opts.no_prompt,
                        );
                        return Ok(ExitKind::Error);
                    }
                }
            }
        }
    }

    // Postflight — failures here are loud but do NOT trigger rollback.
    let mut postflight_bad = 0usize;
    if !bundle.postflight.is_empty() {
        eprintln!("[inspect] postflight: {} check(s)", bundle.postflight.len());
        for (i, check) in bundle.postflight.iter().enumerate() {
            match run_check(&*runner, bundle.host.as_deref(), check) {
                Ok(res) => {
                    if res.passed {
                        eprintln!("  ✓ {} — {}", res.label, res.detail);
                    } else {
                        eprintln!("  ✗ {} — {}", res.label, res.detail);
                        postflight_bad += 1;
                    }
                }
                Err(e) => {
                    eprintln!("  ✗ postflight check {} errored: {e}", i + 1);
                    postflight_bad += 1;
                }
            }
        }
    }

    // F8: invalidate the runtime cache for every namespace any
    // completed step touched. Bundles can run arbitrary `exec` /
    // `run` bodies — we can't reliably classify them as mutating
    // vs read-only at parse time, so we conservatively invalidate.
    // Worst case: one extra `docker ps` on the next read verb.
    // Best case: no operator ever sees stale data after a bundle
    // run (the F8 invariant must hold for `bundle apply` too, not
    // just for the `restart` lifecycle verb).
    let mut bundle_touched: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for &i in &completed {
        let s = &bundle.steps[i];
        if let Some(t) = s.target.as_deref().or(bundle.host.as_deref()) {
            // Strip any `/service` suffix; cache is keyed by namespace.
            let ns = t.split('/').next().unwrap_or(t).to_string();
            if !ns.is_empty() && !ns.starts_with('@') && !ns.contains('*') {
                bundle_touched.insert(ns);
            }
        }
    }
    for ns in &bundle_touched {
        crate::verbs::cache::invalidate(ns);
    }

    let dur = started.elapsed().as_secs();
    eprintln!(
        "[inspect] bundle `{name}` id={id} complete in {dur}s — {n} step(s) ok{post}",
        name = bundle.name,
        id = bundle_id,
        n = completed.len(),
        post = if postflight_bad == 0 {
            String::new()
        } else {
            format!(", {postflight_bad} postflight failure(s)")
        },
    );

    Ok(if postflight_bad == 0 {
        ExitKind::Success
    } else {
        // Postflight failures don't roll back, but they do flag a
        // non-zero exit so CI catches them.
        ExitKind::Error
    })
}

// -----------------------------------------------------------------------------
// Validation
// -----------------------------------------------------------------------------

fn validate_bundle(bundle: &Bundle) -> Result<()> {
    if bundle.steps.is_empty() {
        return Err(anyhow!("bundle has no steps"));
    }
    let mut seen_ids: HashMap<&str, usize> = HashMap::new();
    for (idx, step) in bundle.steps.iter().enumerate() {
        if let Some(prev) = seen_ids.insert(step.id.as_str(), idx) {
            return Err(anyhow!(
                "duplicate step id `{id}` at indices {prev} and {idx}",
                id = step.id
            ));
        }
        let _ = step.body_kind().map_err(|e| anyhow!("{e}"))?;

        // `requires:` must reference earlier ids (no cycles, no fwd refs).
        for req in &step.requires {
            match seen_ids.get(req.as_str()) {
                Some(&prev) if prev < idx => {}
                Some(_) => {
                    return Err(anyhow!(
                        "step `{id}` requires `{req}` defined at the same index (self-loop?)",
                        id = step.id
                    ))
                }
                None => {
                    return Err(anyhow!(
                        "step `{id}` requires `{req}` which is not defined earlier",
                        id = step.id
                    ))
                }
            }
        }

        if step.parallel {
            if step.matrix.is_empty() {
                return Err(anyhow!(
                    "step `{id}` has parallel: true but no matrix:",
                    id = step.id
                ));
            }
            if step.matrix.len() > 1 {
                return Err(anyhow!(
                    "step `{id}`: matrix supports a single key in v0.1.2 (got {})",
                    step.matrix.len(),
                    id = step.id
                ));
            }
        }

        if let OnFailure::RollbackTo(target_id) = &step.on_failure {
            // Target must be defined earlier.
            if !seen_ids.contains_key(target_id.as_str()) {
                return Err(anyhow!(
                    "step `{id}`: on_failure rollback_to `{t}` refers to an unknown or later step",
                    id = step.id,
                    t = target_id
                ));
            }
        }
    }

    // Rollback steps need bodies and unique ids too.
    let mut rb_ids: BTreeSet<&str> = BTreeSet::new();
    for s in &bundle.rollback {
        if !rb_ids.insert(s.id.as_str()) {
            return Err(anyhow!("duplicate rollback step id `{id}`", id = s.id));
        }
        let _ = s.body_kind().map_err(|e| anyhow!("rollback: {e}"))?;
    }
    Ok(())
}

fn requires_apply(bundle: &Bundle) -> bool {
    bundle.steps.iter().any(|s| s.apply && s.exec.is_some())
        || bundle.rollback.iter().any(|s| s.apply && s.exec.is_some())
}

fn format_on_failure(of: &OnFailure) -> String {
    match of {
        OnFailure::Abort => "abort".to_string(),
        OnFailure::Continue => "continue".to_string(),
        OnFailure::Rollback => "rollback".to_string(),
        OnFailure::RollbackTo(id) => format!("rollback_to:{id}"),
    }
}

// -----------------------------------------------------------------------------
// Step rendering & execution
// -----------------------------------------------------------------------------

fn render_step_body(
    step: &Step,
    kind: StepBodyKind,
    vars: &BTreeMap<String, serde_yaml::Value>,
    matrix: &BTreeMap<String, serde_yaml::Value>,
) -> Result<String> {
    let raw = match kind {
        StepBodyKind::Exec => step.exec.as_deref().unwrap_or(""),
        StepBodyKind::Run => step.run.as_deref().unwrap_or(""),
        StepBodyKind::Watch => {
            let w = step
                .watch
                .as_ref()
                .ok_or_else(|| anyhow!("step `{}`: watch body missing", step.id))?;
            return Ok(format_watch_body(w));
        }
    };
    interpolate(raw, vars, matrix).map_err(|e: InterpError| anyhow!("step `{}`: {e}", step.id))
}

fn format_watch_body(w: &super::schema::WatchStep) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(c) = &w.until_cmd {
        parts.push(format!("until_cmd=`{c}`"));
    }
    if let Some(l) = &w.until_log {
        parts.push(format!("until_log=`{l}`"));
    }
    if let Some(s) = &w.until_sql {
        parts.push(format!("until_sql=`{s}`"));
    }
    if let Some(u) = &w.until_http {
        parts.push(format!("until_http=`{u}`"));
    }
    if let Some(e) = &w.equals {
        parts.push(format!("equals=`{e}`"));
    }
    if let Some(t) = &w.timeout {
        parts.push(format!("timeout={t}"));
    }
    parts.join(" ")
}

/// Run a single step (with fan-out for `parallel: true` matrix steps).
/// L6 (v0.1.3): outcome of one step. `Single` is a normal
/// (non-matrix) step — the apply loop tracks it in `completed` but
/// has no per-branch records to rollback against. `Matrix` carries
/// the per-branch results so [`do_rollback`] can iterate ONLY the
/// succeeded branches with per-branch matrix interpolation.
pub(crate) enum StepOutcome {
    Single,
    Matrix(Vec<BranchResult>),
}

fn run_step(
    runner: &dyn RemoteRunner,
    bundle: &Bundle,
    step: &Step,
    bundle_id: &str,
    store: &AuditStore,
    opts: &ApplyOpts,
) -> Result<StepOutcome> {
    let kind = step.body_kind().map_err(|e| anyhow!("{e}"))?;

    // `--apply` gate per step. Skipped if step opts out (`apply:
    // false`) or if it's a read-only `run`/`watch` step.
    let needs_apply = step.apply && matches!(kind, StepBodyKind::Exec);
    if needs_apply && !opts.apply {
        return Err(anyhow!(
            "step `{}` is destructive and `--apply` was not passed",
            step.id
        ));
    }

    if step.parallel && !step.matrix.is_empty() {
        let branches = run_parallel_matrix(runner, bundle, step, kind, bundle_id, store, opts)?;
        return Ok(StepOutcome::Matrix(branches));
    }

    let empty_matrix = BTreeMap::new();
    let _audit_id = run_single_branch(
        runner,
        bundle,
        step,
        kind,
        &empty_matrix,
        bundle_id,
        store,
        opts,
    )?;
    Ok(StepOutcome::Single)
}

/// Run one (possibly matrix-branch) step body. Returns the audit
/// entry id minted for this branch when the body wrote one (every
/// `exec` and `watch` step does; `run` is intentionally unaudited).
/// L6 (v0.1.3): `matrix` is non-empty only for branches dispatched
/// from `run_parallel_matrix`; when non-empty, the audit entry is
/// stamped with `bundle_branch` + `bundle_branch_status` so
/// `inspect bundle status <id>` can render per-branch outcomes
/// without re-deriving them from `args` text.
#[allow(clippy::too_many_arguments)]
fn run_single_branch(
    runner: &dyn RemoteRunner,
    bundle: &Bundle,
    step: &Step,
    kind: StepBodyKind,
    matrix: &BTreeMap<String, serde_yaml::Value>,
    bundle_id: &str,
    store: &AuditStore,
    _opts: &ApplyOpts,
) -> Result<Option<String>> {
    let target_ns = step
        .target
        .as_deref()
        .or(bundle.host.as_deref())
        .ok_or_else(|| {
            anyhow!(
                "step `{}`: no target (set bundle `host:` or step `target:`)",
                step.id
            )
        })?;

    let timeout_secs = step.timeout_secs.unwrap_or(DEFAULT_STEP_TIMEOUT_SECS);
    let reason = step.reason.clone().or_else(|| bundle.reason.clone());

    let started = Instant::now();
    match kind {
        StepBodyKind::Exec | StepBodyKind::Run => {
            let body = render_step_body(step, kind, &bundle.vars, matrix)?;
            // Resolve through the same selector pipeline `inspect run`
            // and `inspect watch` use so container-scoped selectors
            // (`ns/svc`) get the docker-exec wrap automatically.
            let (_plan_runner, nses, targets) = resolve_plan(target_ns)
                .map_err(|e| anyhow!("resolving target `{target_ns}`: {e}"))?;
            let resolved: Vec<_> = iter_steps(&nses, &targets).collect();
            if resolved.is_empty() {
                return Err(anyhow!(
                    "step `{}`: target `{}` matched no resources",
                    step.id,
                    target_ns
                ));
            }
            if resolved.len() > 1 {
                return Err(anyhow!(
                    "step `{}`: target `{}` matched {} resources; bundle steps must address a single target",
                    step.id,
                    target_ns,
                    resolved.len()
                ));
            }
            let rstep = &resolved[0];
            let cmd = match rstep.container() {
                Some(container) => format!(
                    "docker exec {} sh -c {}",
                    shquote(container),
                    shquote(&body)
                ),
                None => body.clone(),
            };

            // Stream stdout to operator's stderr (so stdout stays clean
            // for any data the step might emit). Use the streaming-
            // capturing variant so the audit args reflect what was shown.
            let opts = RunOpts::with_timeout(timeout_secs);
            let mut on_line = |line: &str| {
                eprintln!("    {line}");
            };
            let out = runner.run_streaming_capturing(
                &rstep.ns.namespace,
                &rstep.ns.target,
                &cmd,
                opts,
                &mut on_line,
            )?;
            let dur_ms = started.elapsed().as_millis() as u64;

            // Audit only `exec` (per Run-vs-Exec contract). Record the
            // user-authored body, not the docker-exec wrapper, so the
            // audit log matches what the operator wrote in YAML.
            let mut audit_id: Option<String> = None;
            if matches!(kind, StepBodyKind::Exec) {
                let mut e = AuditEntry::new("exec", target_ns);
                e.args = body.clone();
                e.exit = out.exit_code;
                e.duration_ms = dur_ms;
                e.reason = crate::safety::validate_reason(reason.as_deref())?;
                e.bundle_id = Some(bundle_id.to_string());
                e.bundle_step = Some(step.id.clone());
                // L6 (v0.1.3): stamp matrix-branch metadata when this
                // single-branch run is the per-branch leg of a
                // `parallel: true` + `matrix:` step.
                if let Some((mkey, mval)) = matrix.iter().next() {
                    let label = super::vars::yaml_to_str(mval).unwrap_or_default();
                    e.bundle_branch = Some(format!("{mkey}={label}"));
                    e.bundle_branch_status =
                        Some(if out.ok() { "ok" } else { "failed" }.to_string());
                }
                audit_id = Some(e.id.clone());
                let _ = store.append(&e);
            }

            if !out.ok() {
                return Err(anyhow!(
                    "exit {}: {}",
                    out.exit_code,
                    out.stderr.trim().lines().next().unwrap_or("")
                ));
            }
            Ok(audit_id)
        }
        StepBodyKind::Watch => {
            let watch_step = step
                .watch
                .as_ref()
                .ok_or_else(|| anyhow!("step `{}`: watch body missing", step.id))?;
            let args =
                build_watch_args(watch_step, target_ns, reason.clone(), &bundle.vars, matrix)?;
            // Delegate to the B10 engine. It maintains its own audit
            // entry; we patch in bundle_id/step via env hand-off:
            // simplest path is to call ::run() and let watch write its
            // own audit (verb=watch). To attach bundle_id, append a
            // second short audit entry from here marking the bundle
            // membership.
            let exit = crate::verbs::watch::run(args)?;
            let dur_ms = started.elapsed().as_millis() as u64;
            let code = exit.code() as i32;
            let mut e = AuditEntry::new("bundle.watch", target_ns);
            e.args = format!("step={}", step.id);
            e.exit = code;
            e.duration_ms = dur_ms;
            e.reason = crate::safety::validate_reason(reason.as_deref())?;
            e.bundle_id = Some(bundle_id.to_string());
            e.bundle_step = Some(step.id.clone());
            // L6 (v0.1.3): mirror the matrix-branch metadata for
            // watch-step branches so `inspect bundle status` can
            // group them alongside the exec-step branches.
            if let Some((mkey, mval)) = matrix.iter().next() {
                let label = super::vars::yaml_to_str(mval).unwrap_or_default();
                e.bundle_branch = Some(format!("{mkey}={label}"));
                e.bundle_branch_status = Some(if code == 0 { "ok" } else { "failed" }.to_string());
            }
            let audit_id = Some(e.id.clone());
            let _ = store.append(&e);
            if code != 0 {
                return Err(anyhow!("watch exit {code}"));
            }
            Ok(audit_id)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_parallel_matrix(
    runner: &dyn RemoteRunner,
    bundle: &Bundle,
    step: &Step,
    kind: StepBodyKind,
    bundle_id: &str,
    store: &AuditStore,
    opts: &ApplyOpts,
) -> Result<Vec<BranchResult>> {
    // Single-key matrix only (validated earlier). Build the per-branch
    // matrix maps and run them with bounded concurrency.
    let (mkey, mvals) = step
        .matrix
        .iter()
        .next()
        .ok_or_else(|| anyhow!("step `{}`: parallel without matrix", step.id))?;
    let cap = step
        .max_parallel
        .unwrap_or(mvals.len())
        .clamp(1, super::schema::MAX_PARALLEL_CAP);

    eprintln!(
        "    parallel matrix={mkey} entries={} max_parallel={cap}",
        mvals.len()
    );

    // Channel of pending entries; workers pull until empty. We use a
    // shared Mutex over a Vec instead of std::sync::mpsc to keep the
    // dependency surface small and the worker loop simple.
    let queue: Arc<Mutex<Vec<serde_yaml::Value>>> = Arc::new(Mutex::new(mvals.clone()));
    let first_err: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let stop_flag = Arc::new(AtomicBool::new(false));
    // L6 (v0.1.3): per-branch outcome ledger. Workers append to it as
    // they complete; the parent reads it back after the scope joins
    // and forwards it to the apply loop for rollback consumption.
    let branches: Arc<Mutex<Vec<BranchResult>>> = Arc::new(Mutex::new(Vec::new()));

    std::thread::scope(|scope| {
        for _ in 0..cap {
            let queue = Arc::clone(&queue);
            let first_err = Arc::clone(&first_err);
            let stop_flag = Arc::clone(&stop_flag);
            let branches = Arc::clone(&branches);
            let mkey = mkey.clone();
            // Captures of &-references are fine inside scoped threads.
            scope.spawn(move || loop {
                if stop_flag.load(Ordering::SeqCst) {
                    return;
                }
                let v = {
                    let mut q = queue.lock().unwrap();
                    if q.is_empty() {
                        return;
                    }
                    q.remove(0)
                };
                let mut mtx = BTreeMap::new();
                mtx.insert(mkey.clone(), v.clone());
                let label = super::vars::yaml_to_str(&v).unwrap_or_default();
                eprintln!("    ├─ {mkey}={label} starting");
                // Isolate worker panics: a panic in run_single_branch
                // would otherwise tear down the entire scope and
                // bypass first_err recording. Convert to a normal
                // failure so rollback semantics still apply.
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_single_branch(runner, bundle, step, kind, &mtx, bundle_id, store, opts)
                }));
                let res = match res {
                    Ok(r) => r,
                    Err(payload) => {
                        let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                            (*s).to_string()
                        } else if let Some(s) = payload.downcast_ref::<String>() {
                            s.clone()
                        } else {
                            "worker panicked".to_string()
                        };
                        Err(anyhow!("panic: {msg}"))
                    }
                };
                let branch_id = format!("{mkey}={label}");
                match res {
                    Ok(_audit_id) => {
                        eprintln!("    ├─ {branch_id} ok");
                        branches.lock().unwrap().push(BranchResult {
                            branch_id,
                            status: BranchStatus::Ok,
                            matrix_value: v.clone(),
                            matrix_key: mkey.clone(),
                        });
                    }
                    Err(e) => {
                        eprintln!("    ├─ {branch_id} FAILED: {e}");
                        branches.lock().unwrap().push(BranchResult {
                            branch_id: branch_id.clone(),
                            status: BranchStatus::Failed,
                            matrix_value: v.clone(),
                            matrix_key: mkey.clone(),
                        });
                        let mut slot = first_err.lock().unwrap();
                        if slot.is_none() {
                            *slot = Some(format!("{branch_id}: {e}"));
                            // First failure aborts the rest of the
                            // matrix — avoids burning compute on a
                            // run we're going to roll back anyway.
                            stop_flag.store(true, Ordering::SeqCst);
                        }
                    }
                }
            });
        }
    });

    // L6 (v0.1.3): branches that the stop_flag short-circuited never
    // actually started. Synthesize Skipped records so
    // `inspect bundle status` can render the full matrix table and
    // the rollback path can prove it didn't try to invert them.
    let mut branches_vec = std::mem::take(&mut *branches.lock().unwrap());
    let executed: BTreeSet<String> = branches_vec.iter().map(|b| b.branch_id.clone()).collect();
    let leftover: Vec<serde_yaml::Value> = std::mem::take(&mut *queue.lock().unwrap());
    for v in leftover {
        let label = super::vars::yaml_to_str(&v).unwrap_or_default();
        let branch_id = format!("{mkey}={label}");
        if !executed.contains(&branch_id) {
            branches_vec.push(BranchResult {
                branch_id,
                status: BranchStatus::Skipped,
                matrix_value: v,
                matrix_key: mkey.clone(),
            });
        }
    }
    // Sort by branch_id so the rollback path and post-mortem queries
    // see the same deterministic order regardless of worker
    // scheduling.
    branches_vec.sort_by(|a, b| a.branch_id.cmp(&b.branch_id));

    if let Some(msg) = first_err.lock().unwrap().clone() {
        // L6: even on failure we return the per-branch ledger via Err
        // so the apply loop can stash it for rollback. Encode the
        // branches alongside the error message via a side channel —
        // simplest is to thread it through a thread-local-like
        // sidecar. We use a `BranchFailure` shape that the apply
        // loop unpacks.
        return Err(BranchFailureCarrier::wrap(branches_vec, msg));
    }
    Ok(branches_vec)
}

/// L6 (v0.1.3): when a `parallel` matrix step fails partway, we need
/// to thread the per-branch ledger back to the apply loop alongside
/// the error message so [`do_rollback`] can invert only the
/// succeeded branches. anyhow's [`anyhow::Error`] is a single-value
/// container; we attach the ledger via a thread-local sidecar
/// keyed by the error's pointer identity. This avoids touching
/// `Result<T, E>`'s shape.
struct BranchFailureCarrier;

impl BranchFailureCarrier {
    fn wrap(branches: Vec<BranchResult>, msg: String) -> anyhow::Error {
        BRANCH_LEDGER_SIDECAR.with(|cell| {
            *cell.borrow_mut() = Some(branches);
        });
        anyhow!(msg)
    }
    /// Drain the most recently stashed ledger. Returns `None` if the
    /// caller's failure didn't come from a matrix step.
    fn drain() -> Option<Vec<BranchResult>> {
        BRANCH_LEDGER_SIDECAR.with(|cell| cell.borrow_mut().take())
    }
}

thread_local! {
    static BRANCH_LEDGER_SIDECAR: std::cell::RefCell<Option<Vec<BranchResult>>> =
        const { std::cell::RefCell::new(None) };
}

fn build_watch_args(
    w: &super::schema::WatchStep,
    target: &str,
    reason: Option<String>,
    vars: &BTreeMap<String, serde_yaml::Value>,
    matrix: &BTreeMap<String, serde_yaml::Value>,
) -> Result<crate::cli::WatchArgs> {
    // Interpolate every string field that an operator might template.
    let interp = |s: Option<&str>| -> Result<Option<String>> {
        match s {
            Some(v) => Ok(Some(
                interpolate(v, vars, matrix).map_err(|e| anyhow!("watch field: {e}"))?,
            )),
            None => Ok(None),
        }
    };
    Ok(crate::cli::WatchArgs {
        selector: target.to_string(),
        until_cmd: interp(w.until_cmd.as_deref())?,
        until_log: interp(w.until_log.as_deref())?,
        until_sql: interp(w.until_sql.as_deref())?,
        until_http: interp(w.until_http.as_deref())?,
        equals: interp(w.equals.as_deref())?,
        matches: interp(w.matches.as_deref())?,
        gt: w.gt,
        lt: w.lt,
        changes: w.changes,
        stable_for: w.stable_for.clone(),
        regex: w.regex,
        psql_opts: w.psql_opts.clone(),
        r#match: w.match_expr.clone(),
        insecure: w.insecure,
        interval: w.interval.clone(),
        timeout: w.timeout.clone(),
        reason,
        verbose: false,
    })
}

// -----------------------------------------------------------------------------
// Rollback
// -----------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn do_rollback(
    runner: &dyn RemoteRunner,
    bundle: &Bundle,
    bundle_id: &str,
    store: &AuditStore,
    completed: &[usize],
    step_branches: &BTreeMap<usize, Vec<BranchResult>>,
    rollback_to: Option<&str>,
    no_prompt: bool,
) {
    // L6 (v0.1.3): even when no top-level step `completed`, a
    // parallel-matrix step that failed mid-way may have produced
    // succeeded branches that DO need inverting. Walk both the
    // `completed` set and any partially-failed matrix steps.
    let mut to_visit: Vec<usize> = completed.to_vec();
    for &idx in step_branches.keys() {
        if !to_visit.contains(&idx) {
            to_visit.push(idx);
        }
    }
    to_visit.sort();

    let any_branch_to_invert = step_branches
        .values()
        .any(|brs| brs.iter().any(|b| b.status == BranchStatus::Ok));

    if to_visit.is_empty() && bundle.rollback.is_empty() && !any_branch_to_invert {
        eprintln!("[inspect] nothing to rollback");
        return;
    }

    if !no_prompt && !confirm_rollback() {
        eprintln!(
            "[inspect] rollback declined by operator. Bundle left in partially-applied state."
        );
        return;
    }

    eprintln!("[inspect] rolling back...");

    // Per-step rollbacks: walk visited steps in reverse, run each
    // step's `rollback:` field if set and the step is reversible.
    // Stop at `rollback_to` if specified.
    let stop_idx = match rollback_to {
        Some(t) => bundle.steps.iter().position(|s| s.id == t),
        None => None,
    };

    let empty = BTreeMap::new();
    for &idx in to_visit.iter().rev() {
        if let Some(stop) = stop_idx {
            if idx <= stop {
                eprintln!(
                    "[inspect] reached checkpoint `{}`, stopping per-step rollback",
                    bundle.steps[stop].id
                );
                break;
            }
        }
        let step = &bundle.steps[idx];
        if !step.reversible {
            eprintln!(
                "[inspect] skip rollback of `{}` (reversible=false)",
                step.id
            );
            continue;
        }
        let Some(rb_cmd) = &step.rollback else {
            continue;
        };

        // L6 (v0.1.3): branch-aware rollback. For matrix steps we
        // run the rollback block once per SUCCEEDED branch, with
        // `{{ matrix.<key> }}` resolving to that branch's value.
        // Failed/skipped branches log an audit note explaining why
        // no inverse fired (the step never produced an effect on
        // them).
        if let Some(branches) = step_branches.get(&idx) {
            for br in branches {
                match br.status {
                    BranchStatus::Ok => {
                        let mut mtx = BTreeMap::new();
                        mtx.insert(br.matrix_key.clone(), br.matrix_value.clone());
                        let rb_rendered = match interpolate(rb_cmd, &bundle.vars, &mtx) {
                            Ok(r) => r,
                            Err(e) => {
                                eprintln!(
                                    "[inspect] rollback FAILED on `{}` branch {}: render error: {e}",
                                    step.id, br.branch_id
                                );
                                eprintln!(
                                    "[inspect] STOPPING rollback — bundle is in mixed state."
                                );
                                return;
                            }
                        };
                        if let Err(e) = run_rollback_action(
                            runner,
                            bundle,
                            &step.id,
                            &rb_rendered,
                            bundle_id,
                            store,
                            Some(&br.branch_id),
                        ) {
                            eprintln!(
                                "[inspect] rollback FAILED on `{}` branch {}: {e}",
                                step.id, br.branch_id
                            );
                            eprintln!("[inspect] STOPPING rollback — bundle is in mixed state.");
                            return;
                        }
                    }
                    BranchStatus::Failed | BranchStatus::Skipped => {
                        let why = if br.status == BranchStatus::Failed {
                            "branch failed mid-execution"
                        } else {
                            "branch skipped (peer branch failed first)"
                        };
                        eprintln!(
                            "[inspect] skip rollback of `{}` branch {} — {why}",
                            step.id, br.branch_id
                        );
                        // Audit note so the bundle status report can
                        // show "no inverse fired, branch never
                        // applied an effect" for forensic clarity.
                        let mut e = AuditEntry::new("bundle.rollback.skip", &step.id);
                        e.is_revert = true;
                        e.bundle_id = Some(bundle_id.to_string());
                        e.bundle_step = Some(step.id.clone());
                        e.bundle_branch = Some(br.branch_id.clone());
                        e.bundle_branch_status = Some(br.status.as_audit_str().to_string());
                        e.diff_summary = format!("rollback skipped: {why}");
                        let _ = store.append(&e);
                    }
                }
            }
            continue;
        }

        // Non-matrix step: legacy single-rollback path.
        let rb_rendered = match interpolate(rb_cmd, &bundle.vars, &empty) {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "[inspect] rollback FAILED on `{}`: render error: {e}",
                    step.id
                );
                eprintln!("[inspect] STOPPING rollback — bundle is in mixed state.");
                return;
            }
        };
        if let Err(e) = run_rollback_action(
            runner,
            bundle,
            &step.id,
            &rb_rendered,
            bundle_id,
            store,
            None,
        ) {
            eprintln!("[inspect] rollback FAILED on `{}`: {e}", step.id);
            eprintln!("[inspect] STOPPING rollback — bundle is in mixed state.");
            return;
        }
    }

    // Bundle-level rollback block — ONLY when not running a partial
    // rollback_to. Run in reverse declaration order.
    if rollback_to.is_none() {
        for step in bundle.rollback.iter().rev() {
            let kind = match step.body_kind() {
                Ok(k) => k,
                Err(e) => {
                    eprintln!("[inspect] rollback step `{}`: {e}", step.id);
                    return;
                }
            };
            if !matches!(kind, StepBodyKind::Exec | StepBodyKind::Run) {
                eprintln!(
                    "[inspect] rollback `{}` body kind {:?} not supported (use exec/run)",
                    step.id, kind
                );
                return;
            }
            let body = match render_step_body(step, kind, &bundle.vars, &empty) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("[inspect] rollback `{}` render: {e}", step.id);
                    return;
                }
            };
            if let Err(e) =
                run_rollback_action(runner, bundle, &step.id, &body, bundle_id, store, None)
            {
                eprintln!("[inspect] rollback step `{}` FAILED: {e}", step.id);
                eprintln!("[inspect] STOPPING rollback — bundle is in mixed state.");
                return;
            }
        }
    }

    eprintln!("[inspect] rollback complete");
}

fn run_rollback_action(
    runner: &dyn RemoteRunner,
    bundle: &Bundle,
    step_id: &str,
    body: &str,
    bundle_id: &str,
    store: &AuditStore,
    branch_label: Option<&str>,
) -> Result<()> {
    let prefix = match branch_label {
        Some(b) => format!("[inspect] rollback `{step_id}` branch {b}: "),
        None => format!("[inspect] rollback `{step_id}`: "),
    };
    eprintln!("{prefix}{}", short(body));
    let target_ns = bundle
        .host
        .as_deref()
        .ok_or_else(|| anyhow!("bundle has no `host:` and rollback step has no target"))?;
    let (_plan_runner, nses, targets) = resolve_plan(target_ns)
        .map_err(|e| anyhow!("resolving rollback target `{target_ns}`: {e}"))?;
    let resolved: Vec<_> = iter_steps(&nses, &targets).collect();
    let rstep = resolved
        .first()
        .ok_or_else(|| anyhow!("rollback target `{target_ns}` matched no resources"))?;
    let cmd = match rstep.container() {
        Some(container) => format!("docker exec {} sh -c {}", shquote(container), shquote(body)),
        None => body.to_string(),
    };
    let started = Instant::now();
    let opts = RunOpts::with_timeout(DEFAULT_STEP_TIMEOUT_SECS);
    let mut on_line = |line: &str| {
        eprintln!("      {line}");
    };
    let out = runner.run_streaming_capturing(
        &rstep.ns.namespace,
        &rstep.ns.target,
        &cmd,
        opts,
        &mut on_line,
    )?;
    let dur_ms = started.elapsed().as_millis() as u64;
    let mut e = AuditEntry::new("bundle.rollback", target_ns);
    e.args = body.to_string();
    e.exit = out.exit_code;
    e.duration_ms = dur_ms;
    e.is_revert = true;
    e.bundle_id = Some(bundle_id.to_string());
    e.bundle_step = Some(step_id.to_string());
    // L6 (v0.1.3): when this rollback is for a specific matrix
    // branch (per-branch invert), stamp the branch label so audit
    // queries can group by it.
    if let Some(b) = branch_label {
        e.bundle_branch = Some(b.to_string());
        e.bundle_branch_status = Some(if out.ok() { "ok" } else { "failed" }.to_string());
    }
    let _ = store.append(&e);
    if !out.ok() {
        return Err(anyhow!(
            "exit {}: {}",
            out.exit_code,
            out.stderr.trim().lines().next().unwrap_or("")
        ));
    }
    Ok(())
}

fn confirm_rollback() -> bool {
    use std::io::{stdin, stdout, BufRead, IsTerminal, Write};
    if !stdin().is_terminal() {
        // Non-TTY default: roll back. CI passes --no-prompt to avoid
        // hitting this branch at all. Prompting would hang forever.
        return true;
    }
    eprint!("[inspect] rollback completed steps? [y/N] ");
    let _ = stdout().flush();
    let mut line = String::new();
    let _ = stdin().lock().read_line(&mut line);
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

// -----------------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------------

/// Mint a fresh bundle correlation id. Format mirrors `AuditEntry.id`
/// for log-grep symmetry: `<ts-millis>-<rand4>`.
fn mint_bundle_id() -> String {
    use std::sync::atomic::AtomicU32;
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let ts = Utc::now().timestamp_millis();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    // Non-cryptographic; we just want low collision risk across
    // simultaneous bundle invocations on the same machine.
    let salt = (ts as u32).wrapping_mul(2654435761).wrapping_add(n);
    format!("b{ts}-{salt:04x}")
}

fn short(s: &str) -> String {
    let one = s.replace('\n', " ");
    if one.chars().count() > 80 {
        let mut out: String = one.chars().take(77).collect();
        out.push('…');
        out
    } else {
        one
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::schema::{Bundle, OnFailure, Step, WatchStep};

    fn step(id: &str) -> Step {
        Step {
            id: id.to_string(),
            target: None,
            exec: Some(format!("echo {id}")),
            run: None,
            watch: None,
            requires: vec![],
            parallel: false,
            matrix: BTreeMap::new(),
            max_parallel: None,
            on_failure: OnFailure::Abort,
            reversible: true,
            apply: true,
            rollback: None,
            timeout_secs: None,
            reason: None,
        }
    }

    fn bundle(steps: Vec<Step>) -> Bundle {
        Bundle {
            name: "t".into(),
            host: Some("h".into()),
            reason: None,
            vars: BTreeMap::new(),
            preflight: vec![],
            steps,
            rollback: vec![],
            postflight: vec![],
        }
    }

    #[test]
    fn validate_rejects_empty_steps() {
        let b = bundle(vec![]);
        assert!(validate_bundle(&b).is_err());
    }

    #[test]
    fn validate_rejects_duplicate_ids() {
        let b = bundle(vec![step("a"), step("a")]);
        assert!(validate_bundle(&b).is_err());
    }

    #[test]
    fn validate_rejects_forward_requires() {
        let mut s_a = step("a");
        s_a.requires = vec!["b".into()];
        let b = bundle(vec![s_a, step("b")]);
        assert!(validate_bundle(&b).is_err());
    }

    #[test]
    fn validate_rejects_unknown_rollback_to() {
        let mut s = step("b");
        s.on_failure = OnFailure::RollbackTo("zzz".into());
        let bd = bundle(vec![step("a"), s]);
        assert!(validate_bundle(&bd).is_err());
    }

    #[test]
    fn validate_rejects_parallel_without_matrix() {
        let mut s = step("a");
        s.parallel = true;
        let b = bundle(vec![s]);
        assert!(validate_bundle(&b).is_err());
    }

    #[test]
    fn validate_rejects_multikey_matrix() {
        let mut s = step("a");
        s.parallel = true;
        s.matrix.insert("x".into(), vec![serde_yaml::Value::Null]);
        s.matrix.insert("y".into(), vec![serde_yaml::Value::Null]);
        let b = bundle(vec![s]);
        assert!(validate_bundle(&b).is_err());
    }

    #[test]
    fn requires_apply_true_when_destructive_step_present() {
        let b = bundle(vec![step("a")]);
        assert!(requires_apply(&b));
    }

    #[test]
    fn requires_apply_false_for_run_only_bundle() {
        let mut s = step("a");
        s.exec = None;
        s.run = Some("ls".into());
        let b = bundle(vec![s]);
        assert!(!requires_apply(&b));
    }

    #[test]
    fn requires_apply_false_when_step_opts_out() {
        let mut s = step("a");
        s.apply = false;
        let b = bundle(vec![s]);
        assert!(!requires_apply(&b));
    }

    #[test]
    fn mint_bundle_id_format() {
        let id = mint_bundle_id();
        assert!(id.starts_with('b'));
        assert!(id.contains('-'));
    }

    #[test]
    fn watch_args_inherits_target_and_interpolates() {
        let w = WatchStep {
            until_cmd: Some("echo {{ vars.x }}".into()),
            until_log: None,
            until_sql: None,
            until_http: None,
            equals: Some("{{ matrix.k }}".into()),
            matches: None,
            gt: None,
            lt: None,
            changes: false,
            stable_for: None,
            regex: false,
            psql_opts: None,
            match_expr: None,
            insecure: false,
            interval: None,
            timeout: Some("5s".into()),
        };
        let mut vars = BTreeMap::new();
        vars.insert("x".into(), serde_yaml::Value::String("hello".into()));
        let mut mtx = BTreeMap::new();
        mtx.insert("k".into(), serde_yaml::Value::String("world".into()));
        let args = build_watch_args(&w, "ns/svc", None, &vars, &mtx).unwrap();
        assert_eq!(args.selector, "ns/svc");
        assert_eq!(args.until_cmd.as_deref(), Some("echo hello"));
        assert_eq!(args.equals.as_deref(), Some("world"));
        assert_eq!(args.timeout.as_deref(), Some("5s"));
    }

    #[test]
    fn short_truncates() {
        let long = "a".repeat(120);
        let s = short(&long);
        assert!(s.chars().count() <= 80);
        assert!(s.ends_with('…'));
    }
}
