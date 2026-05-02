//! `inspect fleet <verb>` — Phase 11 multi-namespace orchestrator.
//!
//! Resolves `--ns` to a list of configured namespaces (glob, comma list,
//! or `@group`), applies the large-fanout interlock against the **total
//! target count** (bible §13), and then fans out the inner verb across
//! the matched set with bounded concurrency. Failures on individual
//! namespaces do not abort the run by default — the bible says "if one
//! namespace fails, fleet continues with the rest".
//!
//! Implementation strategy: each per-namespace step is executed by
//! re-invoking the same `inspect` binary as a child process. This keeps
//! credential handling, profile loading, mock-runner detection, and the
//! safety gate identical to a direct invocation. Two injection modes are
//! supported:
//!
//! * **Selector verbs** (status, ps, logs, restart, …): the child
//!   inherits a private env-var pair (see [`FORCE_NS_VAR`] /
//!   [`FORCE_PARENT_PID_VAR`]) that the selector resolver honors as an
//!   override for the server portion of the selector. The user's verb
//!   args are forwarded verbatim, except that any explicit server atoms
//!   in the inner selector are rejected up-front (M2) — there is no
//!   silent intersection.
//! * **Namespace-positional verbs** (setup, test, connect, …): the
//!   namespace is injected as the first positional argument, no env
//!   override is set.

use std::collections::{BTreeMap, BTreeSet};
use std::process::{Command as StdCommand, Stdio};
use std::sync::mpsc;
use std::thread;

use anyhow::{anyhow, Result};

use crate::cli::FleetArgs;
use crate::commands::list::json_string;
use crate::config::groups;
use crate::config::resolver as ns_resolver;
use crate::error::ExitKind;
use crate::profile::cache::load_profile;
use crate::profile::schema::{Profile, ServiceKind};
use crate::safety::gate::{Confirm, ConfirmResult, SafetyGate};
use crate::selector::ast::{Selector, ServiceSpec};
use crate::selector::parser::parse_selector;

/// Default concurrency cap (bible §13).
const DEFAULT_FLEET_CONCURRENCY: usize = 8;
/// Hard ceiling so a misconfigured env var can't spawn thousands of children.
const MAX_FLEET_CONCURRENCY: usize = 64;
/// Large-fanout interlock threshold (bible §8.2).
const FANOUT_THRESHOLD: usize = 10;
/// Internal env var the selector resolver consults to pin selector
/// resolution to a single namespace. Renamed from the Phase-11 draft
/// (`INSPECT_FLEET_FORCE_NS`) so the contract is explicitly private:
/// honoring the override requires both this var and a matching parent
/// pid (see [`FORCE_PARENT_PID_VAR`]).
pub const FORCE_NS_VAR: &str = "INSPECT_INTERNAL_FLEET_FORCE_NS";
/// PID of the fleet parent that set [`FORCE_NS_VAR`]. The selector
/// resolver only honors the override when this matches the resolver's
/// own parent pid, so a stray exported value in a user shell can't
/// silently scope every subsequent `inspect` invocation.
pub const FORCE_PARENT_PID_VAR: &str = "INSPECT_INTERNAL_FLEET_PARENT_PID";

/// Verbs that accept a namespace as their first positional arg rather
/// than a selector. Fleet must inject the namespace into the args list
/// for these instead of using the env-based selector override.
const NAMESPACE_POSITIONAL_VERBS: &[&str] = &[
    "setup",
    "discover",
    "test",
    "show",
    "connect",
    "disconnect",
    "profile",
];

/// Verbs that must NOT be invoked through fleet (recursion / nonsense).
const DISALLOWED_INNER_VERBS: &[&str] = &[
    "fleet",
    "add",
    "remove",
    "list",
    "connections",
    "disconnect-all",
    "alias",
    "audit",
    "revert",
    "resolve",
    "search",
];

pub fn run(args: FleetArgs) -> Result<ExitKind> {
    if args.verb.is_empty() {
        return Err(anyhow!(
            "missing inner verb (e.g. 'inspect fleet status --ns prod-*')"
        ));
    }
    if DISALLOWED_INNER_VERBS.contains(&args.verb.as_str()) {
        return Err(anyhow!(
            "verb '{}' is not supported under 'inspect fleet' (it is single-namespace or global by design)",
            args.verb
        ));
    }

    // Phase 11: enumerate configured namespaces (env ∪ file).
    let all = ns_resolver::list_all()?;
    if all.is_empty() {
        return Err(anyhow!(
            "no namespaces configured; run 'inspect add <name>' before using fleet"
        ));
    }
    let known: Vec<String> = all.iter().map(|n| n.name.clone()).collect();

    // Resolve --ns to a concrete list. `@group` is exclusive; mixing it
    // into a comma-list is rejected up-front so the precedence is
    // unambiguous (L1).
    let chosen = expand_ns_pattern(&args.ns, &known)?;
    if chosen.is_empty() {
        return Err(anyhow!(
            "fleet --ns '{}' matched no configured namespaces (known: {})",
            args.ns,
            if known.is_empty() {
                "<none>".to_string()
            } else {
                known.join(", ")
            }
        ));
    }

    // M2: when fleet pins namespace resolution via the env override,
    // any server portion in the inner selector would be silently
    // discarded. We handle this in two ways:
    //   * a bare token with no `/` (e.g. `pulse`, `_`) is rewritten to
    //     `_/<token>` so the user's intent ("this service / host on
    //     each chosen namespace") survives the override unchanged.
    //   * an explicit server portion (`arte/pulse`, `~prod-1/_`,
    //     `*/pulse`) is rejected up-front — there is no safe
    //     interpretation that wouldn't surprise the user.
    let force_ns_mode = !NAMESPACE_POSITIONAL_VERBS.contains(&args.verb.as_str());
    let mut inner_args = args.args.clone();
    if force_ns_mode {
        if let Some(sel_idx) = first_positional_index(&inner_args) {
            let raw = inner_args[sel_idx].clone();
            if let Some(rewritten) = rewrite_inner_selector(&raw)? {
                inner_args[sel_idx] = rewritten;
            }
        }
    }

    // H2: compute the total target count before fanout so the
    // large-fanout interlock fires on the actual fanout size, not just
    // the namespace count. We do a best-effort dry-resolve against each
    // chosen namespace's cached profile; if a profile isn't present the
    // namespace contributes 1 (the same lower bound the live verb uses).
    let inner_selector = first_positional_index(&inner_args).map(|i| inner_args[i].as_str());
    let total_targets = estimate_total_targets(&chosen, inner_selector, force_ns_mode);

    // L7: build the safety gate via the public constructor so any future
    // side-effects in `SafetyGate::new` stay in sync.
    let mut gate = SafetyGate::new(true, args.yes_all, args.yes_all);
    gate.fanout_threshold = FANOUT_THRESHOLD;
    let prompt = if total_targets > chosen.len() {
        format!(
            "About to fan out to {} target(s) across {} namespace(s). Continue?",
            total_targets,
            chosen.len()
        )
    } else {
        format!(
            "About to fan out to {} namespace(s). Continue?",
            chosen.len()
        )
    };
    match gate.confirm(Confirm::LargeFanout, total_targets, &prompt) {
        ConfirmResult::Apply | ConfirmResult::DryRun => {}
        ConfirmResult::Aborted(why) => {
            return Err(anyhow!("fleet aborted: {why}"));
        }
    }

    // Resolve concurrency. Emits a stderr warning if an env-supplied
    // value was clamped (L4).
    let concurrency = resolve_concurrency(args.concurrency)?;

    // Self-binary path for child invocations.
    let self_exe = std::env::current_exe()
        .map_err(|e| anyhow!("cannot locate inspect binary for fleet fanout: {e}"))?;

    // M5: pre-warm one SSH master per chosen namespace BEFORE fanout.
    // Without this, N children each try to open their own master in
    // parallel, which (a) hammers the local ssh binary and remote sshd,
    // and (b) often gets us rate-limited by managed SSH providers. With
    // the pre-warm, every child hits the `MasterStatus::Alive` fast
    // path in `start_master` and reuses one socket per ns.
    //
    // Pre-warm is bounded to a small connect concurrency (independent
    // of fleet fanout concurrency) and is best-effort: a per-ns failure
    // is logged to stderr and the child will surface the real error
    // when it tries again. Skipped entirely under the mock runner.
    if force_ns_mode {
        prewarm_masters(&chosen, &all);
    }

    // Build the per-namespace plan.
    let plan: Vec<NsPlan> = chosen
        .iter()
        .map(|ns| NsPlan {
            namespace: ns.clone(),
            child_args: build_child_args(&args.verb, &inner_args, ns),
            force_ns: force_ns_mode,
        })
        .collect();

    // S1: collect every `key_passphrase_env` configured for OTHER
    // namespaces so each child only sees its own credential env var.
    let foreign_passphrase_envs = passphrase_envs_by_ns(&all);

    let abort_on_error = args.abort_on_error;
    let parent_pid = std::process::id();

    // Field pitfall §4.3: optional canary phase. Run the first N
    // namespaces (in the same sorted order as `chosen`) with
    // abort-on-error forced, then proceed to the remainder only if
    // every canary returned exit 0. Selectors with --canary > total
    // collapse to "run everything as canary".
    let mut results: Vec<NsResult> = Vec::with_capacity(plan.len());
    let canary_n = args.canary.unwrap_or(0).min(plan.len());
    let mut plan_iter = plan.into_iter();
    if canary_n > 0 {
        let canary_plan: Vec<NsPlan> = (&mut plan_iter).take(canary_n).collect();
        let canary_names: Vec<String> = canary_plan.iter().map(|p| p.namespace.clone()).collect();
        eprintln!(
            "fleet: canary phase: running {} of {} namespace(s) first: {}",
            canary_n,
            canary_n + plan_iter.len(),
            canary_names.join(", ")
        );
        let canary_results = fan_out(
            &self_exe,
            canary_plan,
            concurrency,
            true, // canary always aborts on first failure
            parent_pid,
            &foreign_passphrase_envs,
        );
        let canary_failed: Vec<&NsResult> = canary_results.iter().filter(|r| r.exit != 0).collect();
        if !canary_failed.is_empty() {
            let names: Vec<String> = canary_failed
                .iter()
                .map(|r| format!("{}(exit {})", r.namespace, r.exit))
                .collect();
            let remaining: Vec<String> = plan_iter.map(|p| p.namespace).collect();
            eprintln!(
                "fleet: canary failed on {} namespace(s): {}. \
                 Aborting before {} remaining namespace(s): {}",
                canary_failed.len(),
                names.join(", "),
                remaining.len(),
                if remaining.is_empty() {
                    "<none>".to_string()
                } else {
                    remaining.join(", ")
                }
            );
            results.extend(canary_results);
            // Skip rest of fleet; emit and return as failure.
            let total = results.len();
            let ok = results.iter().filter(|r| r.exit == 0).count();
            let failed = total - ok;
            if args.json {
                emit_json(&args.verb, &results, total, ok, failed);
            } else {
                emit_human(&args.verb, &results, total, ok, failed);
            }
            return Ok(ExitKind::Error);
        }
        results.extend(canary_results);
    }

    let rest_plan: Vec<NsPlan> = plan_iter.collect();
    if !rest_plan.is_empty() {
        let rest_results = fan_out(
            &self_exe,
            rest_plan,
            concurrency,
            abort_on_error,
            parent_pid,
            &foreign_passphrase_envs,
        );
        results.extend(rest_results);
    }
    results.sort_by(|a, b| a.namespace.cmp(&b.namespace));

    let total = results.len();
    let ok = results.iter().filter(|r| r.exit == 0).count();
    let failed = total - ok;

    if args.json {
        emit_json(&args.verb, &results, total, ok, failed);
    } else {
        emit_human(&args.verb, &results, total, ok, failed);
    }

    // M3: collapse per-child exit codes into a single fleet exit code
    // with explicit, documented semantics:
    //   * any child fails with code != 0 and != 1   -> Error (2)
    //   * else if every child returned NoMatches    -> NoMatches (1)
    //   * else (any ok, possibly mixed with 1s)     -> Success (0)
    if results.iter().any(|r| r.exit > 1) {
        Ok(ExitKind::Error)
    } else if total > 0 && results.iter().all(|r| r.exit == 1) {
        Ok(ExitKind::NoMatches)
    } else {
        Ok(ExitKind::Success)
    }
}

/// One per-namespace child plan.
struct NsPlan {
    namespace: String,
    child_args: Vec<String>,
    force_ns: bool,
}

/// Result of one per-namespace child run.
pub(crate) struct NsResult {
    pub namespace: String,
    pub exit: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Expand a `--ns` argument to a concrete, deduplicated, sorted list of
/// configured namespace names. Accepts:
/// * `@<group>` — looked up in `~/.inspect/groups.toml`. Exclusive form;
///   may not be combined with comma-listed entries.
/// * `a,b,c` — comma-separated explicit list (each entry may itself be
///   a glob)
/// * a single glob or literal
fn expand_ns_pattern(pat: &str, known: &[String]) -> Result<Vec<String>> {
    let trimmed = pat.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("--ns must not be empty"));
    }
    // L1: `@group` is exclusive. Mixing into a comma-list is ambiguous;
    // require the user to either use the group alone or spell members out.
    let has_group_marker = trimmed.contains('@');
    let has_comma = trimmed.contains(',');
    if has_group_marker && (has_comma || !trimmed.starts_with('@')) {
        return Err(anyhow!(
            "--ns: '@<group>' is exclusive; mix with a comma list is ambiguous. \
             Either use '--ns @<group>' alone or spell the members out (e.g. '--ns a,b,c')."
        ));
    }
    if let Some(name) = trimmed.strip_prefix('@') {
        let file = groups::load()?;
        let group = file.groups.get(name).ok_or_else(|| {
            anyhow!(
                "group '@{name}' not found in groups.toml (known groups: {})",
                if file.groups.is_empty() {
                    "<none>".to_string()
                } else {
                    file.groups.keys().cloned().collect::<Vec<_>>().join(", ")
                }
            )
        })?;
        return Ok(groups::expand_members(&group.members, known));
    }
    let mut out: BTreeSet<String> = BTreeSet::new();
    for piece in trimmed.split(',') {
        let p = piece.trim();
        if p.is_empty() {
            continue;
        }
        for m in groups::expand_members(&[p.to_string()], known) {
            out.insert(m);
        }
    }
    Ok(out.into_iter().collect())
}

fn resolve_concurrency(flag: Option<usize>) -> Result<usize> {
    let (raw, source) = if let Some(v) = flag {
        (v, "--concurrency")
    } else if let Ok(env) = std::env::var("INSPECT_FLEET_CONCURRENCY") {
        let parsed = env.parse::<usize>().map_err(|e| {
            anyhow!("INSPECT_FLEET_CONCURRENCY='{env}' is not a positive integer: {e}")
        })?;
        (parsed, "INSPECT_FLEET_CONCURRENCY")
    } else {
        (DEFAULT_FLEET_CONCURRENCY, "default")
    };
    if raw == 0 {
        return Err(anyhow!("fleet concurrency must be >= 1"));
    }
    let clamped = raw.min(MAX_FLEET_CONCURRENCY);
    if clamped != raw {
        eprintln!(
            "warning: fleet concurrency from {source} ({raw}) clamped to hard ceiling of {clamped}"
        );
    }
    // Field pitfall §4.2: clamp again by the per-process file-descriptor
    // budget. Each child fleet process opens its own SSH master
    // (typically 4 fds) plus the parent's own pipes for stdout/stderr
    // (2 fds), so high concurrency on tight ulimits silently EMFILEs.
    let final_cap = crate::sys::ulimit::clamp_with_warning(clamped, "fleet --concurrency");
    Ok(final_cap)
}

/// Best-effort index of the inner verb's selector positional. Returns
/// the index of the last non-flag-shaped token in `args`. This is only
/// a heuristic (we don't know each verb's flag schema), so the M2
/// inner-selector check below treats parse failures as "not a
/// selector" and silently skips them rather than producing false
/// positives.
fn first_positional_index(args: &[String]) -> Option<usize> {
    args.iter().enumerate().rev().find_map(|(i, a)| {
        if a == "--" || a.starts_with('-') {
            None
        } else {
            Some(i)
        }
    })
}

/// M2: rewrite the inner-verb selector so fleet's namespace pin is
/// safe.
///
/// * Returns `Ok(None)` if the selector should be left as-is.
/// * Returns `Ok(Some(rewritten))` for bare service-portion tokens
///   (e.g. `pulse` -> `_/pulse`).
/// * Returns `Err(...)` for selectors that explicitly target a server
///   set, since those would be silently overridden by the env pin.
fn rewrite_inner_selector(raw: &str) -> Result<Option<String>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    // Bare host marker: leave alone (resolves to host-level on each ns).
    if trimmed == "_" {
        return Ok(None);
    }
    // Already in `server/service[:path]` form.
    if trimmed.contains('/') {
        // We only allow `_/...` (host-marker server) here; any other
        // server portion is rejected.
        if let Some(rest) = trimmed.strip_prefix("_/") {
            // `_/...` is unambiguous; keep verbatim.
            let _ = rest; // explicit no-op to document intent
            return Ok(None);
        }
        // Permit a leading regex like `/foo/_` as the SERVICE portion
        // only when there is no preceding non-empty server segment.
        // Otherwise reject.
        return Err(anyhow!(
            "selector '{raw}' specifies a server portion inside 'fleet', which would be silently overridden. \
             Drop the server portion (use 'fleet --ns <pattern>' for namespace selection) and pass only the service \
             portion to the inner verb (e.g. '_' or 'pulse')."
        ));
    }
    // Wildcards / exclusions in a bare token are server atoms by syntax
    // and have no service-portion meaning under the env pin.
    if trimmed.contains('*') || trimmed.starts_with('~') || trimmed == "all" {
        return Err(anyhow!(
            "selector '{raw}' looks like a server pattern, but 'fleet' has already chosen the namespace set \
             via '--ns'. Use '--ns' for namespace selection and pass a service portion (e.g. '_' or 'pulse') \
             as the inner selector."
        ));
    }
    // Bare service-portion token: rewrite as `_/<token>` so the
    // selector resolver sees the host-marker server (which the env
    // override will replace with the chosen namespace) plus the user's
    // service.
    Ok(Some(format!("_/{trimmed}")))
}

/// H2: estimate the total number of targets fleet will end up invoking
/// the inner verb on. Uses each namespace's cached profile when
/// available; falls back to `1` per namespace.
fn estimate_total_targets(chosen: &[String], selector: Option<&str>, force_ns_mode: bool) -> usize {
    if !force_ns_mode {
        return chosen.len();
    }
    let parsed = selector.and_then(|s| parse_selector(s).ok());
    let mut total = 0usize;
    for ns in chosen {
        let profile = load_profile(ns).ok().flatten();
        total += target_count_for_ns(parsed.as_ref(), profile.as_ref());
    }
    total.max(chosen.len())
}

fn target_count_for_ns(parsed: Option<&Selector>, profile: Option<&Profile>) -> usize {
    let sel = match parsed {
        Some(s) => s,
        None => return 1,
    };
    match &sel.service {
        None | Some(ServiceSpec::Host) => 1,
        Some(ServiceSpec::All) => profile
            .map(|p| {
                p.services
                    .iter()
                    .filter(|s| matches!(s.kind, ServiceKind::Container | ServiceKind::Systemd))
                    .count()
            })
            .filter(|n| *n > 0)
            .unwrap_or(1),
        Some(ServiceSpec::Atoms(atoms)) => atoms.len().max(1),
    }
}

/// S1 helper: build a map `namespace -> {passphrase env names}` from the
/// resolved namespace list. Children get every other namespace's vars
/// stripped from their environment before spawn.
fn passphrase_envs_by_ns(
    all: &[crate::config::namespace::ResolvedNamespace],
) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for ns in all {
        let mut s: BTreeSet<String> = BTreeSet::new();
        if let Some(v) = ns.config.key_passphrase_env.as_ref() {
            if !v.is_empty() {
                s.insert(v.clone());
            }
        }
        out.insert(ns.name.clone(), s);
    }
    out
}

/// Hard cap on simultaneous SSH connect attempts during pre-warm.
/// Independent of `--concurrency` so a 64-way fleet doesn't trigger
/// 64 simultaneous fresh sshd handshakes against a managed provider.
const PREWARM_CONNECT_CONCURRENCY: usize = 4;

/// M5: open one SSH master per chosen namespace BEFORE child fanout so
/// every child reuses the alive socket via `MasterStatus::Alive`.
///
/// * Skipped under `INSPECT_MOCK_REMOTE_FILE` (test harness) or when
///   the user sets `INSPECT_FLEET_SKIP_PREWARM=1`.
/// * Bounded to [`PREWARM_CONNECT_CONCURRENCY`] simultaneous connects
///   regardless of `--concurrency`.
/// * Best-effort: per-ns failures are reported on stderr and the
///   matching child will surface the real error when it runs.
fn prewarm_masters(chosen: &[String], all: &[crate::config::namespace::ResolvedNamespace]) {
    if std::env::var_os("INSPECT_MOCK_REMOTE_FILE").is_some() {
        return;
    }
    if std::env::var_os("INSPECT_FLEET_SKIP_PREWARM").is_some() {
        return;
    }

    use crate::ssh::master::{
        check_socket, socket_path, start_master, AuthSelection, MasterStatus,
    };
    use crate::ssh::options::SshTarget;

    let by_name: BTreeMap<&str, &crate::config::namespace::ResolvedNamespace> =
        all.iter().map(|n| (n.name.as_str(), n)).collect();

    let (job_tx, job_rx) = mpsc::channel::<String>();
    let job_rx = std::sync::Arc::new(std::sync::Mutex::new(job_rx));

    let workers: Vec<_> = (0..PREWARM_CONNECT_CONCURRENCY.min(chosen.len()))
        .map(|_| {
            let job_rx = job_rx.clone();
            // Each worker borrows from the same map snapshot via Arc.
            let by_name = by_name
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).clone()))
                .collect::<BTreeMap<String, _>>();
            thread::spawn(move || loop {
                let ns = {
                    let lock = job_rx.lock().unwrap();
                    match lock.recv() {
                        Ok(j) => j,
                        Err(_) => break,
                    }
                };
                let resolved = match by_name.get(&ns) {
                    Some(r) => r,
                    None => continue,
                };
                let target = match SshTarget::from_resolved(resolved) {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("fleet: prewarm: ns '{ns}' skipped (target: {e})");
                        continue;
                    }
                };
                // Already alive? Cheap check, no connect.
                let sock = socket_path(&ns);
                if matches!(check_socket(&sock, &target), MasterStatus::Alive) {
                    continue;
                }
                // L4 (v0.1.3): fleet prewarm honors per-namespace
                // auth mode but never prompts (allow_interactive=false);
                // a password-auth namespace without `password_env` set
                // will fail prewarm fast and retry in the child.
                let password_auth = resolved.config.auth.as_deref() == Some("password");
                let auth = AuthSelection {
                    passphrase_env: resolved.config.key_passphrase_env.as_deref(),
                    allow_interactive: false,
                    skip_existing_mux_check: false,
                    password_auth,
                    password_env: resolved.config.password_env.as_deref(),
                };
                let ttl = crate::ssh::ttl::resolve_with_ns(
                    None,
                    resolved.config.session_ttl.as_deref(),
                    Some(password_auth),
                )
                .map(|(t, _)| t)
                .unwrap_or_else(|_| "8h".to_string());
                if let Err(e) = start_master(&ns, &target, &ttl, auth) {
                    eprintln!("fleet: prewarm: ns '{ns}' will retry in child (reason: {e})");
                }
            })
        })
        .collect();

    for ns in chosen {
        if job_tx.send(ns.clone()).is_err() {
            break;
        }
    }
    drop(job_tx);
    for w in workers {
        let _ = w.join();
    }
}

/// Build the argv (excluding the program name) for the child invocation
/// of `inspect <verb>` for one namespace.
fn build_child_args(verb: &str, user_args: &[String], ns: &str) -> Vec<String> {
    let mut out = Vec::with_capacity(user_args.len() + 2);
    out.push(verb.to_string());
    if NAMESPACE_POSITIONAL_VERBS.contains(&verb) {
        // Inject namespace as first positional. The user's args follow.
        out.push(ns.to_string());
        out.extend(user_args.iter().cloned());
    } else {
        // Selector-style verbs: forward args verbatim. The selector
        // resolver picks up INSPECT_FLEET_FORCE_NS from the environment.
        out.extend(user_args.iter().cloned());
    }
    out
}

/// Bounded-concurrency worker pool. Each worker pulls one plan, spawns
/// the child, captures stdout/stderr concurrently, and pushes a result
/// back.
fn fan_out(
    self_exe: &std::path::Path,
    plan: Vec<NsPlan>,
    concurrency: usize,
    abort_on_error: bool,
    parent_pid: u32,
    passphrase_envs: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<NsResult> {
    let total = plan.len();
    if total == 0 {
        return Vec::new();
    }
    let (job_tx, job_rx) = mpsc::channel::<NsPlan>();
    let job_rx = std::sync::Arc::new(std::sync::Mutex::new(job_rx));
    let (res_tx, res_rx) = mpsc::channel::<NsResult>();
    let abort = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let envs = std::sync::Arc::new(passphrase_envs.clone());

    let workers: Vec<_> = (0..concurrency.min(total))
        .map(|_| {
            let job_rx = job_rx.clone();
            let res_tx = res_tx.clone();
            let abort = abort.clone();
            let exe = self_exe.to_path_buf();
            let envs = envs.clone();
            thread::spawn(move || loop {
                if abort.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                let job = {
                    let lock = job_rx.lock().unwrap();
                    match lock.recv() {
                        Ok(j) => j,
                        Err(_) => break,
                    }
                };
                let result = run_child(&exe, &job, parent_pid, &envs);
                let exit_code = result.exit;
                let _ = res_tx.send(result);
                if abort_on_error && exit_code != 0 {
                    abort.store(true, std::sync::atomic::Ordering::SeqCst);
                    break;
                }
            })
        })
        .collect();

    for p in plan {
        if job_tx.send(p).is_err() {
            break;
        }
    }
    drop(job_tx);
    for w in workers {
        let _ = w.join();
    }
    drop(res_tx);

    let mut out: Vec<NsResult> = res_rx.into_iter().collect();
    out.sort_by(|a, b| a.namespace.cmp(&b.namespace));
    out
}

fn run_child(
    exe: &std::path::Path,
    plan: &NsPlan,
    parent_pid: u32,
    passphrase_envs: &BTreeMap<String, BTreeSet<String>>,
) -> NsResult {
    let mut cmd = StdCommand::new(exe);
    cmd.args(&plan.child_args);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    if plan.force_ns {
        cmd.env(FORCE_NS_VAR, &plan.namespace);
        cmd.env(FORCE_PARENT_PID_VAR, parent_pid.to_string());
    } else {
        cmd.env_remove(FORCE_NS_VAR);
        cmd.env_remove(FORCE_PARENT_PID_VAR);
    }
    // S1: drop foreign-namespace passphrase env vars from the child's
    // environment. Each child only sees the `key_passphrase_env`
    // configured for its own namespace (if any).
    let own = passphrase_envs.get(&plan.namespace);
    for var in passphrase_envs.values().flatten() {
        let keep = own.map(|s| s.contains(var)).unwrap_or(false);
        if !keep {
            cmd.env_remove(var);
        }
    }
    cmd.env("INSPECT_NON_INTERACTIVE", "1");

    // H1: `output()` drains both pipes concurrently, avoiding the
    // deadlock that `read_to_string` on each pipe in sequence can hit
    // when a child writes >64 KB to stderr before exiting.
    match cmd.output() {
        Ok(out) => {
            let exit = out.status.code().unwrap_or(2);
            NsResult {
                namespace: plan.namespace.clone(),
                exit,
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            }
        }
        Err(e) => NsResult {
            namespace: plan.namespace.clone(),
            exit: 2,
            stdout: String::new(),
            stderr: format!(
                "fleet: failed to spawn child for ns '{}': {e}",
                plan.namespace
            ),
        },
    }
}

fn emit_human(verb: &str, results: &[NsResult], total: usize, ok: usize, failed: usize) {
    println!("SUMMARY: fleet '{verb}' over {total} namespace(s): {ok} ok, {failed} failed");
    println!("DATA:");
    for r in results {
        let tag = if r.exit == 0 {
            "OK"
        } else if r.exit == 1 {
            "NO-MATCH"
        } else {
            "FAIL"
        };
        println!("  --- [{tag}] {} (exit={}) ---", r.namespace, r.exit);
        for line in r.stdout.lines() {
            println!("    {line}");
        }
        if !r.stderr.trim().is_empty() {
            for line in r.stderr.lines() {
                println!("    err: {line}");
            }
        }
    }
    if failed == 0 {
        println!(
            "NEXT:    inspect fleet {verb} --ns <pattern> --json   # for machine-readable output"
        );
    } else {
        println!(
            "NEXT:    inspect show <ns>   # inspect any failing namespace; rerun with --abort-on-error to fail-fast"
        );
    }
}

fn emit_json(verb: &str, results: &[NsResult], total: usize, ok: usize, failed: usize) {
    let mut s = String::from("{\"schema_version\":1,\"fleet\":{\"verb\":");
    s.push_str(&json_string(verb));
    s.push_str(",\"namespaces\":[");
    for (i, r) in results.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"name\":{name},\"exit\":{exit},\"stdout\":{stdout},\"stderr\":{stderr}}}",
            name = json_string(&r.namespace),
            exit = r.exit,
            stdout = json_string(&r.stdout),
            stderr = json_string(&r.stderr),
        ));
    }
    s.push_str("]},\"summary\":{");
    s.push_str(&format!(
        "\"total\":{total},\"ok\":{ok},\"failed\":{failed}"
    ));
    s.push_str("}}");
    println!("{s}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_child_args_selector_verb() {
        let got = build_child_args("status", &["pulse".to_string()], "prod-1");
        assert_eq!(got, vec!["status".to_string(), "pulse".to_string()]);
    }

    #[test]
    fn build_child_args_namespace_verb_injects_ns() {
        let got = build_child_args("setup", &["--force".to_string()], "prod-1");
        assert_eq!(
            got,
            vec![
                "setup".to_string(),
                "prod-1".to_string(),
                "--force".to_string()
            ]
        );
    }

    #[test]
    fn expand_pattern_comma_list() {
        let known = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let got = expand_ns_pattern("a,c", &known).unwrap();
        assert_eq!(got, vec!["a".to_string(), "c".to_string()]);
    }

    #[test]
    fn expand_pattern_glob() {
        let known = vec![
            "prod-1".to_string(),
            "prod-2".to_string(),
            "staging".to_string(),
        ];
        let got = expand_ns_pattern("prod-*", &known).unwrap();
        assert_eq!(got, vec!["prod-1".to_string(), "prod-2".to_string()]);
    }

    #[test]
    fn resolve_concurrency_clamps() {
        assert_eq!(resolve_concurrency(Some(4)).unwrap(), 4);
        assert_eq!(
            resolve_concurrency(Some(MAX_FLEET_CONCURRENCY * 2)).unwrap(),
            MAX_FLEET_CONCURRENCY
        );
        assert!(resolve_concurrency(Some(0)).is_err());
    }

    #[test]
    fn expand_pattern_rejects_group_in_comma_list() {
        let known = vec!["a".to_string(), "b".to_string()];
        let err = expand_ns_pattern("@grp,a", &known).unwrap_err();
        assert!(
            err.to_string().contains("'@<group>' is exclusive"),
            "expected exclusivity error, got: {err}"
        );
    }

    #[test]
    fn expand_pattern_rejects_group_marker_in_middle() {
        let known = vec!["a".to_string()];
        let err = expand_ns_pattern("a,@grp", &known).unwrap_err();
        assert!(err.to_string().contains("'@<group>' is exclusive"));
    }

    #[test]
    fn first_positional_skips_flags_and_values() {
        let args: Vec<String> = vec![
            "--json".into(),
            "--since".into(),
            "5m".into(),
            "pulse".into(),
        ];
        assert_eq!(first_positional_index(&args), Some(3));
        let args2: Vec<String> = vec!["--apply".into(), "_".into()];
        assert_eq!(first_positional_index(&args2), Some(1));
        let args3: Vec<String> = vec!["--key=val".into(), "_".into()];
        assert_eq!(first_positional_index(&args3), Some(1));
        let args4: Vec<String> = vec!["--apply".into(), "--json".into()];
        assert_eq!(first_positional_index(&args4), None);
    }

    #[test]
    fn rewrite_inner_selector_rejects_server_portion() {
        let err = rewrite_inner_selector("prod-1/pulse").unwrap_err();
        assert!(
            err.to_string().contains("silently overridden"),
            "got: {err}"
        );
    }

    #[test]
    fn rewrite_inner_selector_rewrites_bare_service() {
        let r = rewrite_inner_selector("pulse").unwrap();
        assert_eq!(r, Some("_/pulse".to_string()));
    }

    #[test]
    fn rewrite_inner_selector_passes_through_host_marker() {
        assert_eq!(rewrite_inner_selector("_").unwrap(), None);
        assert_eq!(rewrite_inner_selector("_/pulse").unwrap(), None);
    }

    #[test]
    fn rewrite_inner_selector_rejects_wildcards() {
        assert!(rewrite_inner_selector("*").is_err());
        assert!(rewrite_inner_selector("~prod-1").is_err());
        assert!(rewrite_inner_selector("all").is_err());
    }
}
