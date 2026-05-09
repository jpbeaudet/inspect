//! F17 (v0.1.3): multi-step runner for `inspect run --steps <manifest.json>`.
//!
//! Migration-operator field feedback: *"When I run a 5-step heredoc, all
//! 5 steps are one 'run' with one exit code. If step 3 fails I see it in
//! the output but the SUMMARY still says `1 ok` because the **outer
//! ssh** succeeded. … A `--steps` mode that took an array of commands
//! and returned per-step exit codes would be amazing for migration
//! scripts."*
//!
//! F17 promotes the defensive `set +e; … || echo MARKER` pattern from a
//! widespread workaround to a first-class verb mode with **structured
//! per-step output that an LLM-driven wrapper can reason about**. Without
//! F17, agentic callers cannot reliably build "run these N steps, stop
//! on first failure, give me the per-step result table" workflows on top
//! of `inspect run`.
//!
//! ## Manifest shape
//!
//! ```json
//! {
//!   "steps": [
//!     {"name": "snap",     "cmd": "tar czf - /data > /tmp/snap.tgz", "on_failure": "stop"},
//!     {"name": "stop-app", "cmd": "docker compose stop app",
//!      "on_failure": "stop", "revert_cmd": "docker compose start app"},
//!     {"name": "migrate",  "cmd_file": "./migrate.sh",
//!      "on_failure": "stop", "timeout_s": 600},
//!     {"name": "verify",   "cmd": "curl -fsS http://localhost/health",
//!      "on_failure": "continue"}
//!   ]
//! }
//! ```
//!
//! YAML manifests via `--steps-yaml <path>` are accepted with the same
//! field shape — convenient for operators who maintain migration
//! manifests alongside CI/CD pipelines.
//!
//! ## Multi-target dispatch
//!
//! When the selector resolves to N targets (N >= 1), each manifest step
//! fans out across all N targets **sequentially within the step**. The
//! step's aggregate status is `ok` only if every target's exit was 0;
//! `failed` if any target had a non-zero exit; `timeout` if any target
//! overran its `timeout_s`. `on_failure: "stop"` applies globally — any
//! target's failure aborts the next manifest step on every target. Each
//! (step, target) pair writes its own `run.step` audit entry, all
//! sharing the parent's `steps_run_id`. Parallel fan-out within a
//! single step is L13 in this release.

use std::sync::Mutex;
use std::time::Instant;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::cli::RunArgs;
use crate::error::ExitKind;
use crate::exec::dispatch::{dispatch_with_reauth, ReauthPolicy};
use crate::safety::audit::{AuditEntry, AuditStore, Revert};
use crate::ssh::exec::{RemoteOutput, RunOpts};
use crate::verbs::dispatch::{iter_steps, plan, Step};
use crate::verbs::output::{Envelope, JsonOut, Renderer};
use crate::verbs::quote::shquote;
use crate::verbs::runtime::RemoteRunner;

/// Default per-step wall-clock cap in seconds. Mirrors the F16
/// `--stream` default (8 hours) because the migration-operator's use
/// case is hours-long pipelines whose individual steps may run for
/// many minutes.
const DEFAULT_STEP_TIMEOUT_SECS: u64 = 60 * 60 * 8;

/// Per-(step, target) captured-stdout cap. Live printing is unaffected
/// (the operator always sees every line); the captured copy stops
/// growing past this size and stamps `output_truncated: true` on the
/// per-target result so downstream JSON consumers know the captured
/// blob is partial. 10 MiB matches the F9 `--stdin-max` default —
/// "this is what counts as 'a lot of bytes' in this tool."
const MAX_STEP_CAPTURE_BYTES: usize = 10 * 1024 * 1024;

/// L13 (v0.1.3): default per-step parallel-fanout cap when
/// `parallel: true` is set without an explicit `parallel_max`.
/// Matches `inspect fleet`'s concurrency cap so operators get the
/// same scale-axis behavior across both surfaces.
const PARALLEL_MAX_DEFAULT: usize = 8;

/// L13 (v0.1.3): hard ceiling on `parallel_max`. Above this an
/// operator should reach for `inspect fleet`, which has its own
/// large-fanout safety gate (the L13 path stays focused on
/// migration-bundle scale: 8–32 simultaneous targets).
const PARALLEL_MAX_CEILING: usize = 64;

/// On-disk manifest shape for `inspect run --steps <file.json>` and
/// `--steps-yaml <file.yaml>`.
#[derive(Debug, Clone, Deserialize)]
struct Manifest {
    steps: Vec<StepSpec>,
}

/// A single step's declaration inside the manifest. See module-level
/// docs for the field semantics.
#[derive(Debug, Clone, Deserialize)]
struct StepSpec {
    name: String,
    #[serde(default)]
    cmd: Option<String>,
    /// F14 composition: read the body from a local script file.
    #[serde(default)]
    cmd_file: Option<String>,
    #[serde(default = "default_on_failure")]
    on_failure: OnFailure,
    /// Per-step wall-clock cap (seconds). Default 8 hours.
    #[serde(default)]
    timeout_s: Option<u64>,
    /// Declared inverse for F11 composite revert. Absent ⇒ this step's
    /// per-step audit entry records `revert.kind = "unsupported"`.
    #[serde(default)]
    revert_cmd: Option<String>,
    /// L13 (v0.1.3): when `true` AND the selector resolves to >1
    /// target, dispatch the step's per-target work in parallel
    /// instead of sequentially. Default `false` so existing
    /// manifests behave identically. Capped by [`StepSpec::parallel_max`]
    /// (default 8 — matches `inspect fleet`'s concurrency cap).
    /// Per-target output gets a `[<target>]` prefix and a
    /// per-line writer mutex so two targets can't interleave a
    /// single line; the trade-off is that operators see lines
    /// interleaved by completion order rather than per-target
    /// contiguous (documented contract).
    #[serde(default)]
    parallel: bool,
    /// L13 (v0.1.3): per-step concurrency cap for the parallel
    /// fan-out. `None` ⇒ default 8 (matches the existing
    /// `inspect fleet` cap and the F18 transcript's spec). Above
    /// this many in-flight targets, additional targets queue and
    /// run as slots free up. Hard ceiling at 64 — operators
    /// running against 64+ simultaneous targets should reach for
    /// `inspect fleet` instead, which has its own large-fanout
    /// safety gate.
    #[serde(default)]
    parallel_max: Option<usize>,
}

fn default_on_failure() -> OnFailure {
    OnFailure::Stop
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum OnFailure {
    Stop,
    Continue,
}

/// Per-step aggregate result. `targets` carries one entry per resolved
/// target; the step-level `status` is the aggregate (worst across
/// targets).
#[derive(Debug, Clone, Serialize)]
struct StepResult {
    name: String,
    cmd: String,
    /// Aggregate status across all targets.
    status: StepStatus,
    targets: Vec<TargetStepResult>,
}

/// One target's outcome for one step.
#[derive(Debug, Clone, Serialize)]
struct TargetStepResult {
    /// `<ns>` or `<ns>/<svc>` — what the operator sees in line
    /// prefixes and STEP markers.
    label: String,
    /// `0` for ok, the remote exit code on command failure, `-1` on
    /// transport error, `-2` on a per-step timeout overrun.
    exit: i32,
    duration_ms: u64,
    /// Captured stdout (capped at MAX_STEP_CAPTURE_BYTES; see
    /// `output_truncated`).
    #[serde(skip_serializing_if = "String::is_empty")]
    stdout: String,
    /// Captured stderr (uncapped — typically much smaller than stdout).
    #[serde(skip_serializing_if = "String::is_empty")]
    stderr: String,
    /// `true` when the captured stdout was truncated at the per-step
    /// cap. Live printing was not affected; only the captured copy
    /// (which feeds the audit + JSON output) is partial.
    #[serde(skip_serializing_if = "is_false")]
    output_truncated: bool,
    /// Per-target step status (`ok` / `failed` / `timeout`).
    status: StepStatus,
    /// Audit entry id for this (step, target) pair, so JSON consumers
    /// can point operators at `inspect audit show <id>` for the
    /// captured output + revert details.
    #[serde(skip_serializing_if = "Option::is_none")]
    audit_id: Option<String>,
    /// F13 (v0.1.3): `true` when the dispatch wrapper retried this
    /// (step, target) after a stale-session reauth.
    #[serde(skip_serializing_if = "is_false")]
    retried: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum StepStatus {
    Ok,
    Failed,
    Skipped,
    Timeout,
}

impl StepStatus {
    fn marker(self) -> &'static str {
        match self {
            Self::Ok => "✓",
            Self::Failed => "✗",
            Self::Timeout => "⏱",
            Self::Skipped => "·",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
            Self::Timeout => "timeout",
        }
    }
}

/// Aggregate summary for the structured `--json` output.
#[derive(Debug, Clone, Serialize)]
struct StepsSummary {
    total: usize,
    ok: usize,
    failed: usize,
    skipped: usize,
    /// Name of the step that triggered the stop-on-failure abort, or
    /// `None` if every step ran (whether ok or continue-on-failure).
    stopped_at: Option<String>,
    /// Number of auto-revert entries written by `--revert-on-failure`.
    /// Zero unless the operator opted in AND a step actually failed.
    auto_revert_count: usize,
    /// Number of distinct targets the selector resolved to. `1` for
    /// single-host pipelines (the migration-operator's primary case);
    /// >1 for fleet-wide step dispatch.
    target_count: usize,
}

/// Read + parse a manifest from disk (or stdin when `path == "-"`).
/// `is_yaml` selects the parser. Returns the parsed manifest plus the
/// canonical sha256 (hex) of the raw file body, which the parent audit
/// entry stamps as `manifest_sha256`.
fn load_manifest(path: &str, is_yaml: bool) -> Result<(Manifest, String)> {
    let body = if path == "-" {
        let mut buf = String::new();
        use std::io::Read;
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("reading manifest from stdin")?;
        buf
    } else {
        std::fs::read_to_string(path).with_context(|| format!("reading manifest from '{path}'"))?
    };
    let manifest: Manifest = if is_yaml {
        serde_yaml::from_str(&body)
            .with_context(|| format!("parsing manifest at '{path}' as YAML"))?
    } else {
        serde_json::from_str(&body)
            .with_context(|| format!("parsing manifest at '{path}' as JSON"))?
    };
    let sha = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(body.as_bytes());
        crate::safety::snapshot::hex_encode(&h.finalize())
    };
    Ok((manifest, sha))
}

/// Validate the manifest before dispatch. Returns an error string
/// describing the first problem; clean manifests return `Ok(())`.
fn validate_manifest(manifest: &Manifest) -> Result<()> {
    if manifest.steps.is_empty() {
        return Err(anyhow::anyhow!(
            "manifest has zero steps — at least one step is required"
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for (i, s) in manifest.steps.iter().enumerate() {
        if s.name.trim().is_empty() {
            return Err(anyhow::anyhow!(
                "step #{i}: 'name' is required and must be non-empty"
            ));
        }
        if !seen.insert(s.name.clone()) {
            return Err(anyhow::anyhow!(
                "step '{}' appears twice — names must be unique within a manifest",
                s.name
            ));
        }
        match (&s.cmd, &s.cmd_file) {
            (Some(c), None) if !c.trim().is_empty() => {}
            (None, Some(f)) if !f.trim().is_empty() => {}
            (Some(_), Some(_)) => {
                return Err(anyhow::anyhow!(
                    "step '{}': set exactly one of 'cmd' or 'cmd_file' (got both)",
                    s.name
                ));
            }
            _ => {
                return Err(anyhow::anyhow!(
                    "step '{}': set exactly one of 'cmd' or 'cmd_file' (got neither)",
                    s.name
                ));
            }
        }
    }
    Ok(())
}

/// F17 dispatch entrypoint. Called from `verbs::run::run` when
/// `args.steps` or `args.steps_yaml` is set. Returns the verb-level
/// exit kind.
pub fn run(args: &RunArgs) -> Result<ExitKind> {
    let fmt = args.format.resolve()?;
    let json = matches!(fmt, crate::format::OutputFormat::Json);

    // F17 (v0.1.3): exactly one of --steps or --steps-yaml must be
    // set — the clap layer already enforces this, but resolving here
    // keeps the rest of this function unaware of which spelling fired.
    let (manifest_path, is_yaml) = match (&args.steps, &args.steps_yaml) {
        (Some(p), None) => (p.clone(), false),
        (None, Some(p)) => (p.clone(), true),
        (None, None) => {
            crate::error::emit("steps runner invoked without --steps or --steps-yaml");
            return Ok(ExitKind::Error);
        }
        (Some(_), Some(_)) => {
            crate::error::emit("--steps and --steps-yaml are mutually exclusive");
            return Ok(ExitKind::Error);
        }
    };

    // Reason is informational on bare `inspect run`; for `--steps`,
    // additionally stamp it onto the parent audit entry so a
    // post-mortem of a 4-hour migration can recover the operator's
    // intent without trawling the terminal scrollback.
    let reason = crate::safety::validate_reason(args.reason.as_deref())?;
    if let Some(r) = &reason {
        crate::tee_eprintln!("# reason: {r}");
    }

    let (manifest, manifest_sha) = match load_manifest(&manifest_path, is_yaml) {
        Ok(p) => p,
        Err(e) => {
            crate::error::emit(format!("{e:#}"));
            return Ok(ExitKind::Error);
        }
    };
    if let Err(e) = validate_manifest(&manifest) {
        crate::error::emit(format!("{e}"));
        return Ok(ExitKind::Error);
    }

    let (runner, nses, targets) = plan(&args.selector)?;
    let resolved: Vec<Step<'_>> = iter_steps(&nses, &targets).collect();
    if resolved.is_empty() {
        crate::error::emit(format!("'{}' matched no targets", args.selector));
        return Ok(ExitKind::Error);
    }
    let target_labels: Vec<String> = resolved
        .iter()
        .map(|s| {
            format!(
                "{}{}",
                s.ns.namespace,
                s.service().map(|n| format!("/{n}")).unwrap_or_default()
            )
        })
        .collect();
    let target_count = resolved.len();

    // F12 (v0.1.3): per-invocation env overrides. Validated once
    // before the per-step loop so a typo short-circuits.
    let user_env: Vec<(String, String)> = {
        let mut out = Vec::with_capacity(args.env.len());
        for raw in &args.env {
            out.push(crate::exec::env_overlay::parse_kv(raw)?);
        }
        out
    };

    // F17 (v0.1.3): one fresh `steps_run_id` per --steps invocation,
    // stamped on every per-(step, target) entry, every auto-revert
    // entry, AND on the parent. Reuse the standard AuditEntry::new
    // id-shape so the linkage matches the existing `<ms>-<4hex>`
    // format the rest of the audit log uses.
    let parent_label = if target_count == 1 {
        target_labels[0].clone()
    } else {
        args.selector.clone()
    };
    let steps_run_id = AuditEntry::new("steps", &parent_label).id;

    let audit_store = match AuditStore::open() {
        Ok(s) => Some(s),
        Err(e) => {
            // Match the F9/F16 fallback shape: warn, proceed.
            crate::tee_eprintln!("warning: audit log unavailable ({e}); proceeding without audit");
            None
        }
    };

    let manifest_step_names: Vec<String> = manifest.steps.iter().map(|s| s.name.clone()).collect();

    // F19 (v0.1.3): construct the streaming `--select` filter ONCE at
    // function entry so a parse error fails fast before any frame is
    // emitted. Wrapped in a Mutex (zero-cost in sequential mode) so
    // the parallel execution path can share the single filter
    // instance across worker threads.
    let select: Mutex<Option<crate::query::ndjson::Filter>> =
        Mutex::new(args.format.select_filter()?);

    let mut results: Vec<StepResult> = Vec::with_capacity(manifest.steps.len());
    let mut composite_payload_items: Vec<serde_json::Value> = Vec::new();
    let mut stopped_at: Option<String> = None;
    // L12 (v0.1.3): pre-compute total step count for the F18-style
    // step-boundary headers (`── step 3 of 5: <name> ──`). The
    // dispatch path can still abort early via stop-on-failure, but
    // the headers always render against the manifest's full size so
    // an operator skimming the live tail can tell at a glance how
    // far into the manifest they are.
    let manifest_step_count = manifest.steps.len();

    // SMOKE 2026-05-09 fix: pre-quote operator-supplied positional
    // args once at run() scope so both the forward-pass step
    // dispatch and the reverse-pass revert dispatch can splice
    // them into the rendered cmd. Manifest entries reference these
    // via `$1` / `$2` (the contract documented in
    // `tests/smoke/v013/migration.json::_comment_arg_passing`).
    // Pre-fix, `args.cmd` was silently dropped and step bodies
    // ran with empty positionals.
    let positional_args: String = args
        .cmd
        .iter()
        .map(|a| crate::verbs::quote::shquote(a))
        .collect::<Vec<_>>()
        .join(" ");

    for (step_idx, spec) in manifest.steps.iter().enumerate() {
        // Pipeline already aborted by a stop-on-failure step: every
        // remaining step is marked skipped (per target) without
        // dispatching.
        if stopped_at.is_some() {
            let cmd_preview = spec.cmd.clone().unwrap_or_default();
            results.push(StepResult {
                name: spec.name.clone(),
                cmd: cmd_preview,
                status: StepStatus::Skipped,
                targets: target_labels
                    .iter()
                    .map(|label| TargetStepResult {
                        label: label.clone(),
                        exit: 0,
                        duration_ms: 0,
                        stdout: String::new(),
                        stderr: String::new(),
                        output_truncated: false,
                        status: StepStatus::Skipped,
                        audit_id: None,
                        retried: false,
                    })
                    .collect(),
            });
            // Composite payload still records the inverse so a
            // post-hoc revert (which doesn't run skipped steps'
            // forward action either) keeps the manifest order
            // intact for the reverse walk.
            composite_payload_items.push(composite_item_for_spec(spec));
            continue;
        }

        // F14 composition: read cmd_file once, hold the body for
        // both the dispatch closure and the per-target metadata
        // stamp. (Same body is shipped to every target — the
        // manifest is the source of truth, not the host.)
        let mut script_sha: Option<String> = None;
        let mut script_bytes: Option<u64> = None;
        let mut script_path: Option<String> = None;
        let mut script_body: Option<Vec<u8>> = None;
        // SMOKE 2026-05-09 fix: thread operator-supplied trailing
        // positional args (`inspect run arte --steps manifest.json
        // -- arg1 arg2`) into every step's dispatched command so
        // `$1` / `$2` references inside step `cmd` / `cmd_file`
        // bodies resolve. Pre-fix, `args.cmd` was silently dropped
        // by the steps runner — manifest entries that referenced
        // `"$1"` (e.g. `docker exec "$1" sh -c '…'`) saw an empty
        // `$1` and exited with `invalid container name or ID: value
        // is empty`, including the auto-revert path. Surfaced live
        // during P4.F17 (`migration.json` step 1 failed because
        // `$1` was unbound). Both flavours are now handled:
        //   - cmd path: prepend `set -- <quoted-args>;` so the
        //     remote `sh -c <cmd>` evaluates `cmd` with the
        //     positionals already set.
        //   - cmd_file path: append `-- <quoted-args>` to
        //     `bash -s` (matches F14's `render_script_invocation`).
        let dispatched_cmd: String = if let Some(file) = &spec.cmd_file {
            let body = match std::fs::read(file)
                .with_context(|| format!("reading step '{}' cmd_file '{}'", spec.name, file))
            {
                Ok(b) => b,
                Err(e) => {
                    crate::error::emit(format!("{e:#}"));
                    return Ok(ExitKind::Error);
                }
            };
            let sha = {
                use sha2::{Digest, Sha256};
                let mut h = Sha256::new();
                h.update(&body);
                crate::safety::snapshot::hex_encode(&h.finalize())
            };
            script_bytes = Some(body.len() as u64);
            script_sha = Some(sha);
            script_path = Some(file.clone());
            script_body = Some(body);
            if positional_args.is_empty() {
                "bash -s".to_string()
            } else {
                format!("bash -s -- {positional_args}")
            }
        } else {
            let raw = spec.cmd.clone().unwrap_or_default();
            if positional_args.is_empty() || raw.is_empty() {
                raw
            } else {
                format!("set -- {positional_args}; {raw}")
            }
        };

        let timeout_secs = spec.timeout_s.unwrap_or(DEFAULT_STEP_TIMEOUT_SECS);

        // L12 (v0.1.3): print the step-boundary opener in the F18
        // transcript header format (`── step N of M: <name> ──`)
        // when streaming, so an operator skimming the live tail of
        // a multi-step migration sees the same fence shape they'd
        // see when extracting blocks from the per-day transcript
        // file. Without `--stream`, the legacy `STEP <name> ▶` form
        // stays — non-streaming runs are typically short and the
        // F18 fence's extra horizontal real estate isn't worth it.
        if !json {
            let step_n = step_idx + 1;
            if args.stream {
                if target_count == 1 {
                    crate::tee_println!(
                        "── step {step_n} of {manifest_step_count}: {} ──",
                        spec.name
                    );
                } else {
                    crate::tee_println!(
                        "── step {step_n} of {manifest_step_count}: {} (across {} target(s)) ──",
                        spec.name,
                        target_count
                    );
                }
            } else if target_count == 1 {
                crate::tee_println!("STEP {} ▶", spec.name);
            } else {
                crate::tee_println!("STEP {} ▶ (across {} target(s))", spec.name, target_count);
            }
        } else {
            // F19 (v0.1.3): step-begin envelopes flow through the same
            // `--select` filter as per-target stream lines and the
            // step-end envelope.
            let mut sel = select.lock().unwrap_or_else(|p| p.into_inner());
            JsonOut::write(
                &Envelope::new(&parent_label, "run", "step")
                    .put("steps_run_id", steps_run_id.as_str())
                    .put("step_name", spec.name.as_str())
                    .put("phase", "begin")
                    .put("target_count", target_count as i64),
                sel.as_mut(),
            )?;
        }

        // L13 (v0.1.3): branch on `parallel: true` per spec. The
        // sequential path stays the default so existing manifests
        // behave identically. Parallel branches use std::thread::scope
        // batched in chunks of `parallel_max` (default 8, ceiling 64);
        // wall-clock for a step against N targets becomes
        // ~`ceil(N / parallel_max) × max(target_durations)` instead
        // of `sum(target_durations)`. on_failure="stop" coordination
        // happens via the global cancel flag — when any parallel
        // target fails, in-flight peers see is_cancelled() and the
        // dispatch wrapper aborts.
        let writer_lock_for_parallel = if spec.parallel && resolved.len() > 1 {
            Some(Mutex::new(()))
        } else {
            None
        };
        let ctx = PerTargetCtx {
            spec,
            args,
            runner: runner.as_ref(),
            audit_store: audit_store.as_ref(),
            steps_run_id: &steps_run_id,
            json,
            dispatched_cmd: &dispatched_cmd,
            script_body: script_body.as_deref(),
            script_sha: script_sha.as_deref(),
            script_bytes,
            script_path: script_path.as_deref(),
            timeout_secs,
            user_env: &user_env,
            select: &select,
        };
        let per_target_results: Vec<TargetStepResult> = if spec.parallel && resolved.len() > 1 {
            run_targets_parallel(
                &ctx,
                &resolved,
                &target_labels,
                resolve_parallel_max(spec.parallel_max),
                writer_lock_for_parallel.as_ref().unwrap(),
                spec.on_failure,
            )
        } else {
            resolved
                .iter()
                .zip(&target_labels)
                .enumerate()
                .map(|(target_idx, (target_step, target_label))| {
                    run_one_target(
                        &ctx,
                        target_step,
                        target_label,
                        target_idx,
                        /*is_parallel=*/ false,
                        None,
                    )
                })
                .collect()
        };

        // Aggregate the step's status across targets. Any
        // failure/timeout demotes the aggregate; otherwise ok.
        let aggregate = if per_target_results
            .iter()
            .any(|t| matches!(t.status, StepStatus::Timeout))
        {
            StepStatus::Timeout
        } else if per_target_results
            .iter()
            .any(|t| matches!(t.status, StepStatus::Failed))
        {
            StepStatus::Failed
        } else {
            StepStatus::Ok
        };

        // L12 (v0.1.3): step-closer in the F18 transcript fence
        // format when streaming (`── step N ◀ exit=… duration=…ms
        // audit_id=… ──`), so the live tail's per-step boundaries
        // match the per-day transcript fence shape exactly. The
        // closer carries the audit_id so an operator copy-pasting
        // a fence pair from the live tail into `inspect audit
        // show <audit_id>` works without further translation.
        let step_n = step_idx + 1;
        if !json {
            for tr in &per_target_results {
                if target_count == 1 {
                    if args.stream {
                        let aid = tr
                            .audit_id
                            .as_deref()
                            .map(|s| format!(" audit_id={s}"))
                            .unwrap_or_default();
                        crate::tee_println!(
                            "── step {step_n} ◀ exit={} duration={}ms{aid} ──",
                            tr.exit,
                            tr.duration_ms,
                        );
                    } else {
                        crate::tee_println!(
                            "STEP {} ◀ exit={} duration={}ms",
                            spec.name,
                            tr.exit,
                            tr.duration_ms
                        );
                    }
                } else {
                    // Multi-target: per-target sub-line under one
                    // shared step block. Same fence shape under
                    // --stream; legacy `◀` form otherwise.
                    if args.stream {
                        let aid = tr
                            .audit_id
                            .as_deref()
                            .map(|s| format!(" audit_id={s}"))
                            .unwrap_or_default();
                        crate::tee_println!(
                            "  ◀ {}: exit={} duration={}ms{aid}{}",
                            tr.label,
                            tr.exit,
                            tr.duration_ms,
                            if tr.retried {
                                " (retried after reauth)"
                            } else {
                                ""
                            }
                        );
                    } else {
                        crate::tee_println!(
                            "  ◀ {}: exit={} duration={}ms{}",
                            tr.label,
                            tr.exit,
                            tr.duration_ms,
                            if tr.retried {
                                " (retried after reauth)"
                            } else {
                                ""
                            }
                        );
                    }
                }
                if tr.output_truncated {
                    crate::tee_println!("  ⚠ {}: stdout capture truncated at 10 MiB", tr.label);
                }
            }
            if target_count > 1 {
                if args.stream {
                    crate::tee_println!(
                        "── step {step_n} ◀ status={} ({}/{} targets ok) ──",
                        aggregate.label(),
                        per_target_results
                            .iter()
                            .filter(|t| matches!(t.status, StepStatus::Ok))
                            .count(),
                        target_count
                    );
                } else {
                    crate::tee_println!(
                        "STEP {} ◀ status={} ({}/{} targets ok)",
                        spec.name,
                        aggregate.label(),
                        per_target_results
                            .iter()
                            .filter(|t| matches!(t.status, StepStatus::Ok))
                            .count(),
                        target_count
                    );
                }
            }
        } else {
            // F19 (v0.1.3): step-end envelopes flow through the same
            // `--select` filter (see step-begin emission above).
            let mut sel = select.lock().unwrap_or_else(|p| p.into_inner());
            JsonOut::write(
                &Envelope::new(&parent_label, "run", "step")
                    .put("steps_run_id", steps_run_id.as_str())
                    .put("step_name", spec.name.as_str())
                    .put("phase", "end")
                    .put("status", aggregate.label())
                    .put(
                        "targets_ok",
                        per_target_results
                            .iter()
                            .filter(|t| matches!(t.status, StepStatus::Ok))
                            .count() as i64,
                    )
                    .put("target_count", target_count as i64),
                sel.as_mut(),
            )?;
        }

        composite_payload_items.push(composite_item_for_spec(spec));

        let stop_now = !matches!(aggregate, StepStatus::Ok) && spec.on_failure == OnFailure::Stop;
        results.push(StepResult {
            name: spec.name.clone(),
            cmd: dispatched_cmd.clone(),
            status: aggregate,
            targets: per_target_results,
        });
        if stop_now {
            stopped_at = Some(spec.name.clone());
        }
    }

    // ---- Compute summary counts ----------------------------------
    let total = results.len();
    let ok_count = results
        .iter()
        .filter(|r| matches!(r.status, StepStatus::Ok))
        .count();
    let failed_count = results
        .iter()
        .filter(|r| matches!(r.status, StepStatus::Failed | StepStatus::Timeout))
        .count();
    let skipped_count = results
        .iter()
        .filter(|r| matches!(r.status, StepStatus::Skipped))
        .count();

    // ---- F17 + F11: --revert-on-failure auto-unwind --------------
    //
    // Walk the per-(step, target) entries that already ran (Ok or
    // Failed) in REVERSE manifest order. For each prior step, fan
    // out the inverse across every target that had a per-step entry.
    // Skipped steps' inverses are intentionally not run (they were
    // never applied in the first place).
    let mut auto_revert_count = 0usize;
    if args.revert_on_failure && stopped_at.is_some() {
        if !json {
            crate::tee_println!();
            let applied_steps = results
                .iter()
                .filter(|r| matches!(r.status, StepStatus::Ok | StepStatus::Failed))
                .count();
            crate::tee_println!(
                "─── REVERT: unwinding {} prior step(s) in reverse manifest order ───",
                applied_steps
            );
        }
        for idx in (0..results.len()).rev() {
            let result = &results[idx];
            if !matches!(result.status, StepStatus::Ok | StepStatus::Failed) {
                continue;
            }
            let spec = &manifest.steps[idx];
            let revert_cmd = match &spec.revert_cmd {
                Some(c) if !c.trim().is_empty() => c.clone(),
                _ => {
                    if !json {
                        crate::tee_eprintln!(
                            "  · skipped revert for '{}' (no declared revert_cmd)",
                            spec.name
                        );
                    }
                    continue;
                }
            };
            // SMOKE 2026-05-09 fix: thread the operator's positional
            // args into the revert_cmd too — manifests routinely
            // reference the same `$1` in revert_cmd as in cmd
            // (e.g. `docker exec "$1" sh -c 'rm /tmp/marker'`), so
            // a forward-path arg bug would surface again on the
            // unwind path. Same shape as the forward path:
            // prepend `set -- <quoted-args>;` so the remote sh
            // evaluates revert_cmd with positionals already set.
            let revert_cmd = if positional_args.is_empty() {
                revert_cmd
            } else {
                format!("set -- {positional_args}; {revert_cmd}")
            };
            // Fan out the inverse across each target the step ran
            // against. We iterate per-target results so we have the
            // original audit_id (for auto_revert_of) and the
            // matching target_step (for container resolution).
            for (target_idx, target_result) in result.targets.iter().enumerate() {
                let target_step = &resolved[target_idx];
                let wrapped = match target_step.container() {
                    Some(container) => format!(
                        "docker exec {} sh -c {}",
                        shquote(container),
                        shquote(&revert_cmd)
                    ),
                    None => revert_cmd.clone(),
                };
                let effective_overlay = crate::exec::env_overlay::merge(
                    Some(&target_step.ns.env_overlay),
                    &user_env,
                    args.env_clear,
                );
                let cmd_with_env =
                    crate::exec::env_overlay::apply_to_cmd(&wrapped, &effective_overlay)
                        .into_owned();
                let started = Instant::now();
                let out = runner.run(
                    &target_step.ns.namespace,
                    &target_step.ns.target,
                    &cmd_with_env,
                    RunOpts::with_timeout(120),
                );
                let dur = started.elapsed().as_millis() as u64;
                let (revert_exit, revert_ok, revert_stderr) = match &out {
                    Ok(o) => (o.exit_code, o.ok(), o.stderr.clone()),
                    Err(e) => (-1, false, e.to_string()),
                };
                if !json {
                    if revert_ok {
                        crate::tee_println!(
                            "  ✓ reverted '{}' on {} (exit=0, duration={}ms)",
                            spec.name,
                            target_result.label,
                            dur
                        );
                    } else {
                        crate::tee_eprintln!(
                            "  ✗ revert FAILED for '{}' on {} (exit={}, {})",
                            spec.name,
                            target_result.label,
                            revert_exit,
                            revert_stderr.trim()
                        );
                    }
                }
                if let Some(store) = &audit_store {
                    let mut entry = AuditEntry::new("run.step.revert", &target_result.label);
                    entry.is_revert = true;
                    entry.steps_run_id = Some(steps_run_id.clone());
                    entry.step_name = Some(spec.name.clone());
                    if let Some(orig_id) = &target_result.audit_id {
                        entry.auto_revert_of = Some(orig_id.clone());
                        entry.reverts = Some(orig_id.clone());
                    }
                    entry.args = crate::redact::redact_for_audit(&revert_cmd).into_owned();
                    entry.exit = revert_exit;
                    entry.duration_ms = dur;
                    entry.applied = Some(revert_ok);
                    // G2: revert command body may carry secrets; redact.
                    entry.rendered_cmd =
                        Some(crate::redact::redact_for_audit(&cmd_with_env).into_owned());
                    if !effective_overlay.is_empty() {
                        entry.env_overlay = Some(effective_overlay.clone());
                    }
                    let _ = store.append(&entry);
                }
                auto_revert_count += 1;
            }
        }
    }

    // ---- Parent audit entry --------------------------------------
    //
    // One entry per --steps invocation, written AFTER the per-step
    // entries (and any auto-reverts) so its `duration_ms` reflects
    // the full wall-clock span. The composite payload is the
    // ordered list of per-step inverses; the parent's `selector`
    // is the operator-typed selector (multi-target case) so
    // `inspect revert <parent-id>` re-resolves the same fan-out
    // shape.
    let parent_exit = if failed_count > 0 { 1 } else { 0 };
    if let Some(store) = &audit_store {
        let mut parent = AuditEntry::new("run.steps", &parent_label);
        parent.id = steps_run_id.clone();
        parent.steps_run_id = Some(steps_run_id.clone());
        parent.manifest_sha256 = Some(manifest_sha.clone());
        parent.manifest_steps = Some(manifest_step_names.clone());
        parent.args = format!(
            "manifest={} steps={} targets={} (sha256:{})",
            manifest_path,
            manifest_step_names.len(),
            target_count,
            &manifest_sha[..16.min(manifest_sha.len())]
        );
        parent.exit = parent_exit;
        parent.duration_ms = results
            .iter()
            .flat_map(|r| r.targets.iter())
            .map(|t| t.duration_ms)
            .sum();
        parent.failure_class = Some(
            if failed_count == 0 {
                "ok"
            } else if stopped_at.is_some() {
                "stopped_on_failure"
            } else {
                "command_failed"
            }
            .to_string(),
        );
        if let Some(r) = &reason {
            parent.reason = Some(r.clone());
        }
        if args.stream {
            parent.streamed = true;
        }
        let composite_payload =
            serde_json::to_string(&composite_payload_items).unwrap_or_else(|_| "[]".to_string());
        parent.revert = Some(Revert::composite(
            composite_payload,
            format!(
                "composite revert across {} step(s) × {} target(s); \
                 inspect revert {} walks inverses in reverse manifest order",
                manifest_step_names.len(),
                target_count,
                steps_run_id
            ),
        ));
        parent.applied = Some(true);
        let _ = store.append(&parent);
    }

    // ---- Render the human STEPS table or the JSON object --------
    if json {
        let summary = StepsSummary {
            total,
            ok: ok_count,
            failed: failed_count,
            skipped: skipped_count,
            stopped_at: stopped_at.clone(),
            auto_revert_count,
            target_count,
        };
        let payload = serde_json::json!({
            "v": 1,
            "ns": parent_label,
            "verb": "run.steps",
            "steps_run_id": steps_run_id,
            "manifest_sha256": manifest_sha,
            "target_labels": target_labels,
            "steps": results,
            "summary": summary,
        });
        crate::tee_println!("{}", payload);
    } else {
        let mut r = Renderer::new();
        let across = if target_count == 1 {
            String::new()
        } else {
            format!(" (across {} target(s))", target_count)
        };
        r.summary(format!(
            "STEPS: {total} total, {ok_count} ok, {failed_count} failed, \
             {skipped_count} skipped{across}"
        ));
        for res in &results {
            let extra = match res.status {
                StepStatus::Failed | StepStatus::Timeout
                    if stopped_at.as_deref() == Some(&res.name) =>
                {
                    " (stopped pipeline)"
                }
                _ => "",
            };
            if target_count == 1 {
                let t = &res.targets[0];
                r.data_line(format!(
                    "{marker} {name:24} exit={exit:<4} duration={dur}ms{extra}",
                    marker = res.status.marker(),
                    name = res.name,
                    exit = t.exit,
                    dur = t.duration_ms,
                ));
            } else {
                let ok = res
                    .targets
                    .iter()
                    .filter(|t| matches!(t.status, StepStatus::Ok))
                    .count();
                let max_dur = res.targets.iter().map(|t| t.duration_ms).max().unwrap_or(0);
                r.data_line(format!(
                    "{marker} {name:24} {ok}/{n} ok    (max={max_dur}ms){extra}",
                    marker = res.status.marker(),
                    name = res.name,
                    n = target_count,
                ));
            }
        }
        if let Some(ref n) = stopped_at {
            let skipped: Vec<&str> = results
                .iter()
                .filter(|r| matches!(r.status, StepStatus::Skipped))
                .map(|r| r.name.as_str())
                .collect();
            if !skipped.is_empty() {
                r.data_line(format!("  skipped: {}", skipped.join(", ")));
            }
            r.next(format!(
                "step '{n}' aborted the pipeline; inspect audit show {steps_run_id} for the full table"
            ));
            if args.revert_on_failure {
                r.next(format!(
                    "auto-reverted {auto_revert_count} (step,target) pair(s); \
                     inspect revert {steps_run_id} to walk the composite inverse again"
                ));
            } else {
                r.next(format!(
                    "inspect revert {steps_run_id} to walk the composite inverse \
                     (or re-run with --revert-on-failure next time)"
                ));
            }
        } else if failed_count == 0 {
            r.next(format!(
                "inspect revert {steps_run_id} to walk the composite inverse"
            ));
        }
        r.print();
    }

    if json {
        // F19 (v0.1.3): end-of-stream slurp flush so a `--select-slurp`
        // filter sees its accumulated buffer rendered before the verb
        // exits. No-op for per-frame mode.
        let mut sel = select.lock().unwrap_or_else(|p| p.into_inner());
        crate::verbs::output::flush_filter(sel.as_mut())?;
    }

    Ok(if parent_exit == 0 {
        ExitKind::Success
    } else {
        ExitKind::Error
    })
}

/// L13 (v0.1.3): per-target dispatch context. References-only so the
/// struct is `Send + Sync` for thread::scope dispatch in the parallel
/// path. The runner is passed as `&(dyn RemoteRunner + Send + Sync)`
/// (from `runner.as_ref()`), which inherits the trait's Send/Sync
/// bounds declared at `verbs::runtime::RemoteRunner`.
struct PerTargetCtx<'a> {
    spec: &'a StepSpec,
    args: &'a RunArgs,
    runner: &'a (dyn RemoteRunner + 'a),
    audit_store: Option<&'a AuditStore>,
    steps_run_id: &'a str,
    json: bool,
    dispatched_cmd: &'a str,
    script_body: Option<&'a [u8]>,
    script_sha: Option<&'a str>,
    script_bytes: Option<u64>,
    script_path: Option<&'a str>,
    timeout_secs: u64,
    user_env: &'a [(String, String)],
    /// F19 (v0.1.3): shared `--select` streaming filter handle. Behind
    /// a `Mutex` because steps can run in parallel mode (one filter
    /// instance shared across all per-target threads — same writer-
    /// lock pattern as the stdout `tee_println!` serialization). In
    /// sequential mode the lock is uncontended and zero-cost.
    select: &'a Mutex<Option<crate::query::ndjson::Filter>>,
}

/// L13 (v0.1.3): clamp `parallel_max` to its default-or-ceiling.
/// `None` ⇒ default 8; `Some(n)` capped at the hard ceiling
/// [`PARALLEL_MAX_CEILING`]; `Some(0)` is treated as the default
/// (operators expect 0 to mean "no limit", but our actual contract
/// is "default 8" — surface the ambiguity by using the default
/// rather than spawning unbounded threads).
fn resolve_parallel_max(declared: Option<usize>) -> usize {
    match declared {
        None | Some(0) => PARALLEL_MAX_DEFAULT,
        Some(n) => n.min(PARALLEL_MAX_CEILING),
    }
}

/// L13 (v0.1.3): run all targets for one step in parallel, batched
/// at most `parallel_max` at a time. Per-line emit uses
/// `writer_lock` so two threads can't interleave bytes mid-line.
/// Returns per-target results in **manifest order**, regardless of
/// completion order — agents reading the JSON envelope see the
/// same shape they would for a sequential run.
///
/// `on_failure: stop` coordination: when any thread's target
/// completes with non-zero exit, we set the global cancel flag.
/// Subsequent in-flight ssh dispatches see `is_cancelled()` and
/// abort. Already-completed targets keep their results.
fn run_targets_parallel(
    ctx: &PerTargetCtx<'_>,
    resolved: &[Step<'_>],
    target_labels: &[String],
    parallel_max: usize,
    writer_lock: &Mutex<()>,
    on_failure: OnFailure,
) -> Vec<TargetStepResult> {
    let n = resolved.len();
    let mut out: Vec<Option<TargetStepResult>> = (0..n).map(|_| None).collect();

    // Chunked batches of `parallel_max` targets. After each batch
    // we check `on_failure: stop` and short-circuit if a failure
    // occurred (in-flight peers in the failing batch already saw
    // the cancel flag; we don't dispatch further batches).
    let mut start = 0;
    while start < n {
        let end = (start + parallel_max).min(n);
        let batch: Vec<usize> = (start..end).collect();

        // Borrow fences: scoped threads can borrow `ctx`,
        // `resolved`, `target_labels`, `writer_lock` for the
        // scope's lifetime, no Arc / clone needed.
        std::thread::scope(|s| {
            let mut handles = Vec::with_capacity(batch.len());
            for target_idx in &batch {
                let target_idx = *target_idx;
                let target_step = &resolved[target_idx];
                let target_label = target_labels[target_idx].as_str();
                let writer_lock_ref = writer_lock;
                let ctx_ref = ctx;
                handles.push(s.spawn(move || {
                    run_one_target(
                        ctx_ref,
                        target_step,
                        target_label,
                        target_idx,
                        true,
                        Some(writer_lock_ref),
                    )
                }));
            }
            for (offset, h) in handles.into_iter().enumerate() {
                let target_idx = batch[offset];
                match h.join() {
                    Ok(r) => out[target_idx] = Some(r),
                    Err(_) => {
                        // Worker panicked — synthesize a failed
                        // result so the aggregate can detect it
                        // and the operator gets a clear status.
                        out[target_idx] = Some(TargetStepResult {
                            label: target_labels[target_idx].clone(),
                            exit: -1,
                            duration_ms: 0,
                            stdout: String::new(),
                            stderr: "worker thread panicked".into(),
                            output_truncated: false,
                            status: StepStatus::Failed,
                            audit_id: None,
                            retried: false,
                        });
                    }
                }
            }
        });

        // L13 stop-on-failure: any failure in this batch trips
        // the global cancel flag so peers in subsequent batches
        // see it and skip. Subsequent steps' loop level also
        // sets `stopped_at` based on the aggregate.
        if matches!(on_failure, OnFailure::Stop)
            && out[start..end].iter().any(|r| {
                matches!(
                    r.as_ref().map(|t| t.status),
                    Some(StepStatus::Failed | StepStatus::Timeout)
                )
            })
        {
            crate::exec::cancel::cancel();
            break;
        }

        start = end;
    }

    // Fill any unprocessed slots (when stop-on-failure aborted
    // mid-batches) with skipped placeholders so the per-target
    // count matches the manifest's resolved-target count.
    for (i, slot) in out.iter_mut().enumerate() {
        if slot.is_none() {
            *slot = Some(TargetStepResult {
                label: target_labels[i].clone(),
                exit: 0,
                duration_ms: 0,
                stdout: String::new(),
                stderr: String::new(),
                output_truncated: false,
                status: StepStatus::Skipped,
                audit_id: None,
                retried: false,
            });
        }
    }

    out.into_iter().map(|s| s.unwrap()).collect()
}

/// L13 (v0.1.3): per-target dispatch body. Extracted from F17's
/// inline loop so the same code can drive both the sequential
/// path and the `parallel: true` parallel path. When
/// `writer_lock` is `Some`, every per-line emit acquires it for
/// the duration of the print so two parallel targets cannot
/// interleave bytes mid-line.
#[allow(clippy::too_many_lines)]
fn run_one_target(
    ctx: &PerTargetCtx<'_>,
    target_step: &Step<'_>,
    target_label: &str,
    target_idx: usize,
    is_parallel: bool,
    writer_lock: Option<&Mutex<()>>,
) -> TargetStepResult {
    let spec = ctx.spec;
    let args = ctx.args;
    let json = ctx.json;
    let steps_run_id = ctx.steps_run_id;
    let dispatched_cmd = ctx.dispatched_cmd;

    // Wrap in `docker exec` when the resolved target is a
    // service-level selector. cmd_file uses bash -s with -i so
    // docker exec keeps stdin attached for the body to flow in
    // (same shape F14 uses).
    let inner = match (target_step.container(), spec.cmd_file.is_some()) {
        (Some(container), false) => format!(
            "docker exec {} sh -c {}",
            shquote(container),
            shquote(dispatched_cmd)
        ),
        (Some(container), true) => format!("docker exec -i {} bash -s", shquote(container)),
        (None, _) => dispatched_cmd.to_string(),
    };
    let effective_overlay = crate::exec::env_overlay::merge(
        Some(&target_step.ns.env_overlay),
        ctx.user_env,
        args.env_clear,
    );
    let cmd = crate::exec::env_overlay::apply_to_cmd(&inner, &effective_overlay).into_owned();

    // Per-(step, target) live capture. Capped at MAX_STEP_CAPTURE_BYTES;
    // live printing is unaffected.
    let mut step_stdout = String::new();
    let mut output_truncated = false;
    let started = Instant::now();

    // L12 (v0.1.3): per-(step, target) L7 redactor. One per
    // (step, target) so PEM-block gate state can't leak across
    // pairs. `--show-secrets` bypasses every masker.
    let redactor = crate::redact::OutputRedactor::new(args.show_secrets, args.redact_all);

    let policy = ReauthPolicy {
        allow_reauth: !args.no_reauth && target_step.ns.auto_reauth,
    };
    let cmd_ref = &cmd;
    let runner_ref = ctx.runner;
    let body_for_step: Option<Vec<u8>> = ctx.script_body.map(|s| s.to_vec());
    let label_for_step = target_label.to_string();
    let outcome = dispatch_with_reauth(
        &target_step.ns.namespace,
        &target_step.ns.target,
        runner_ref,
        ctx.audit_store,
        "run.step",
        &label_for_step,
        policy,
        || -> anyhow::Result<RemoteOutput> {
            let mut opts = RunOpts::with_timeout(ctx.timeout_secs);
            if let Some(b) = body_for_step.clone() {
                opts = opts.with_stdin(b);
            }
            if args.stream {
                opts = opts.with_tty(true);
            }
            runner_ref.run_streaming_capturing(
                &target_step.ns.namespace,
                &target_step.ns.target,
                cmd_ref,
                opts,
                &mut |line| {
                    let masked = match redactor.mask_line(line) {
                        Some(m) => m,
                        None => return,
                    };
                    // L13 (v0.1.3): hold the writer lock across
                    // the entire emit (not just one byte) so the
                    // line + its trailing newline land atomically.
                    // No-op closure when sequential.
                    let _g = writer_lock.map(|m| m.lock().unwrap_or_else(|p| p.into_inner()));
                    if !json {
                        crate::tee_println!("{label_for_step} | {masked}");
                    } else {
                        // F19 (v0.1.3): grab the shared filter mutex
                        // for the duration of this emit. Per-frame
                        // filter errors go to stderr and the stream
                        // continues.
                        let mut sel = ctx.select.lock().unwrap_or_else(|p| p.into_inner());
                        if let Err(e) = JsonOut::write(
                            &Envelope::new(&label_for_step, "run", "step")
                                .put("steps_run_id", steps_run_id)
                                .put("step_name", spec.name.as_str())
                                .put("target", label_for_step.as_str())
                                .put("line", masked.as_ref()),
                            sel.as_mut(),
                        ) {
                            crate::error::emit(format!("steps stream emit: {e}"));
                        }
                    }
                    drop(_g);
                    let line_with_nl_bytes = masked.len() + 1;
                    if !output_truncated
                        && step_stdout.len() + line_with_nl_bytes <= MAX_STEP_CAPTURE_BYTES
                    {
                        step_stdout.push_str(masked.as_ref());
                        step_stdout.push('\n');
                    } else if !output_truncated {
                        output_truncated = true;
                        step_stdout.push_str(
                            "\n[OUTPUT CAPTURE TRUNCATED AT 10 MIB — \
                             full output streamed live above]\n",
                        );
                    }
                },
            )
        },
    );
    let dur = started.elapsed().as_millis() as u64;

    let (status, exit_code, stderr_text) = match (&outcome.result, outcome.failure_class) {
        (Ok(out), _) => {
            let code = out.exit_code;
            let st = if code == 0 {
                StepStatus::Ok
            } else {
                StepStatus::Failed
            };
            (st, code, out.stderr.clone())
        }
        (Err(e), _) => {
            let msg = e.to_string();
            if msg.contains("timed out") {
                (StepStatus::Timeout, -2, msg)
            } else {
                (StepStatus::Failed, -1, msg)
            }
        }
    };

    // Per-(step, target) audit entry.
    let mut audit_id_for_target: Option<String> = None;
    if let Some(store) = ctx.audit_store {
        let mut e = AuditEntry::new("run.step", target_label);
        e.steps_run_id = Some(steps_run_id.to_string());
        e.step_name = Some(spec.name.clone());
        e.args = format!(
            "step={} cmd={}",
            spec.name,
            crate::redact::redact_for_audit(&truncate(dispatched_cmd, 200))
        );
        e.exit = exit_code;
        e.duration_ms = dur;
        // G2: redact rendered_cmd to mask any secrets the operator
        // wrote into the steps manifest. `--show-secrets` is not a
        // steps-runner concept (steps are non-interactive), so the
        // safe default applies unconditionally.
        e.rendered_cmd = Some(crate::redact::redact_for_audit(&cmd).into_owned());
        if !effective_overlay.is_empty() {
            e.env_overlay = Some(effective_overlay.clone());
        }
        if outcome.retried {
            e.retry_of = Some(format!("transport_stale@{}", target_label));
        }
        if let Some(rid) = &outcome.reauth_id {
            e.reauth_id = Some(rid.clone());
        }
        let class = match status {
            StepStatus::Ok => "ok",
            StepStatus::Timeout => "timeout",
            StepStatus::Failed => {
                if outcome.result.is_err() && outcome.failure_class.is_some() {
                    outcome
                        .failure_class
                        .as_ref()
                        .map(|c| c.as_str())
                        .unwrap_or("command_failed")
                } else if outcome.result.is_err() {
                    "transport_error"
                } else {
                    "command_failed"
                }
            }
            StepStatus::Skipped => "skipped",
        };
        e.failure_class = Some(class.to_string());
        if matches!(status, StepStatus::Failed | StepStatus::Timeout) && outcome.result.is_err() {
            e.diff_summary = format!("transport_error: {stderr_text}");
        }
        if let Some(s) = ctx.script_sha {
            e.script_sha256 = Some(s.to_string());
        }
        if let Some(b) = ctx.script_bytes {
            e.script_bytes = Some(b);
        }
        if let Some(p) = ctx.script_path {
            e.script_path = Some(p.to_string());
        }
        if args.stream {
            e.streamed = true;
        }
        if redactor.was_active() {
            e.secrets_masked_kinds = Some(
                redactor
                    .active_kinds()
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
            );
        }
        // L13 (v0.1.3): stamp manifest target-list index when the
        // step is dispatched in parallel so a post-mortem walking
        // entries in completion order can sort by `target_idx` to
        // recover manifest order. Sequential entries elide the
        // field (manifest order == log order).
        if is_parallel {
            e.target_idx = Some(target_idx);
        }
        e.revert = Some(match &spec.revert_cmd {
            Some(cmd) if !cmd.trim().is_empty() => Revert::command_pair(
                cmd.clone(),
                format!(
                    "step '{}' inverse on {}: {}",
                    spec.name,
                    target_label,
                    truncate(cmd, 80)
                ),
            ),
            _ => Revert::unsupported(format!(
                "step '{}' has no declared revert_cmd; \
                 --revert-on-failure will skip it",
                spec.name
            )),
        });
        e.applied = Some(matches!(status, StepStatus::Ok | StepStatus::Failed));
        audit_id_for_target = Some(e.id.clone());
        let _ = store.append(&e);
    }

    TargetStepResult {
        label: target_label.to_string(),
        exit: exit_code,
        duration_ms: dur,
        stdout: step_stdout,
        stderr: stderr_text,
        output_truncated,
        status,
        audit_id: audit_id_for_target,
        retried: outcome.retried,
    }
}

/// Build the composite-payload entry for a single step. Independent of
/// dispatch outcome (the manifest's declared inverse is what the
/// composite revert walks, regardless of which target succeeded).
fn composite_item_for_spec(spec: &StepSpec) -> serde_json::Value {
    match &spec.revert_cmd {
        Some(cmd) if !cmd.trim().is_empty() => serde_json::json!({
            "step_name": spec.name,
            "kind": "command_pair",
            "payload": cmd,
        }),
        _ => serde_json::json!({
            "step_name": spec.name,
            "kind": "unsupported",
            "payload": "",
        }),
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n.min(s.len())])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parses_minimal_two_step_form() {
        let body = r#"{"steps":[
            {"name":"a","cmd":"echo hi"},
            {"name":"b","cmd":"echo bye","on_failure":"continue"}
        ]}"#;
        let m: Manifest = serde_json::from_str(body).unwrap();
        assert_eq!(m.steps.len(), 2);
        assert_eq!(m.steps[0].name, "a");
        assert_eq!(m.steps[0].on_failure, OnFailure::Stop);
        assert_eq!(m.steps[1].on_failure, OnFailure::Continue);
    }

    #[test]
    fn manifest_parses_yaml_with_same_shape() {
        let body = "steps:\n  - name: a\n    cmd: echo hi\n  - name: b\n    cmd: echo bye\n    on_failure: continue\n";
        let m: Manifest = serde_yaml::from_str(body).unwrap();
        assert_eq!(m.steps.len(), 2);
        assert_eq!(m.steps[1].on_failure, OnFailure::Continue);
    }

    #[test]
    fn validate_rejects_empty_manifest() {
        let m: Manifest = serde_json::from_str(r#"{"steps":[]}"#).unwrap();
        assert!(validate_manifest(&m).is_err());
    }

    #[test]
    fn validate_rejects_duplicate_names() {
        let m: Manifest = serde_json::from_str(
            r#"{"steps":[
                {"name":"a","cmd":"true"},
                {"name":"a","cmd":"true"}
            ]}"#,
        )
        .unwrap();
        assert!(validate_manifest(&m).is_err());
    }

    #[test]
    fn validate_rejects_step_with_neither_cmd_nor_cmd_file() {
        let m: Manifest = serde_json::from_str(r#"{"steps":[{"name":"a"}]}"#).unwrap();
        assert!(validate_manifest(&m).is_err());
    }

    #[test]
    fn validate_rejects_step_with_both_cmd_and_cmd_file() {
        let m: Manifest =
            serde_json::from_str(r#"{"steps":[{"name":"a","cmd":"true","cmd_file":"./x.sh"}]}"#)
                .unwrap();
        assert!(validate_manifest(&m).is_err());
    }

    #[test]
    fn revert_cmd_field_round_trips() {
        let m: Manifest = serde_json::from_str(
            r#"{"steps":[{"name":"a","cmd":"true","revert_cmd":"undo-true"}]}"#,
        )
        .unwrap();
        assert_eq!(m.steps[0].revert_cmd.as_deref(), Some("undo-true"));
    }

    #[test]
    fn timeout_field_round_trips() {
        let m: Manifest =
            serde_json::from_str(r#"{"steps":[{"name":"a","cmd":"true","timeout_s":600}]}"#)
                .unwrap();
        assert_eq!(m.steps[0].timeout_s, Some(600));
    }
}
