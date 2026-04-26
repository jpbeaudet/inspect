//! `inspect fleet <verb>` — Phase 11 multi-namespace orchestrator.
//!
//! Resolves `--ns` to a list of configured namespaces (glob, comma list,
//! or `@group`), applies the large-fanout interlock, and then fans out the
//! inner verb across the matched set with bounded concurrency. Failures
//! on individual namespaces do not abort the run by default — the bible
//! says "if one namespace fails, fleet continues with the rest".
//!
//! Implementation strategy: each per-namespace step is executed by
//! re-invoking the same `inspect` binary as a child process. This keeps
//! credential handling, profile loading, mock-runner detection, and the
//! safety gate identical to a direct invocation. Two injection modes are
//! supported:
//!
//! * **Selector verbs** (status, ps, logs, restart, …): the child
//!   inherits `INSPECT_FLEET_FORCE_NS=<ns>`, which the selector resolver
//!   honors as an override for the server portion of the selector. The
//!   user's verb args are forwarded verbatim.
//! * **Namespace-positional verbs** (setup, test, connect, …): the
//!   namespace is injected as the first positional argument, no env
//!   override is set.

use std::collections::BTreeSet;
use std::io::Read;
use std::process::{Command as StdCommand, Stdio};
use std::sync::mpsc;
use std::thread;

use anyhow::{anyhow, Result};

use crate::cli::FleetArgs;
use crate::commands::list::json_string;
use crate::config::groups;
use crate::config::resolver as ns_resolver;
use crate::error::ExitKind;
use crate::safety::gate::{Confirm, ConfirmResult, SafetyGate};

/// Default concurrency cap (bible §13).
const DEFAULT_FLEET_CONCURRENCY: usize = 8;
/// Hard ceiling so a misconfigured env var can't spawn thousands of children.
const MAX_FLEET_CONCURRENCY: usize = 64;
/// Large-fanout interlock threshold (bible §8.2).
const FANOUT_THRESHOLD: usize = 10;

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
        return Err(anyhow!("missing inner verb (e.g. 'inspect fleet status --ns prod-*')"));
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

    // Resolve --ns to a concrete list.
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

    // Large-fanout interlock — applied at the namespace count level. The
    // bible says "large-fanout interlock triggers on total target count";
    // since target count >= namespace count, gating on namespace count is
    // a conservative lower bound that always fires when the selector
    // covers >10 namespaces.
    let gate = SafetyGate {
        apply: true,
        yes: args.yes_all,
        yes_all: args.yes_all,
        fanout_threshold: FANOUT_THRESHOLD,
        non_interactive: std::env::var("INSPECT_NON_INTERACTIVE").is_ok()
            || !std::io::IsTerminal::is_terminal(&std::io::stdin()),
    };
    match gate.confirm(
        Confirm::LargeFanout,
        chosen.len(),
        "About to fan out to a large number of namespaces. Continue?",
    ) {
        ConfirmResult::Apply | ConfirmResult::DryRun => {}
        ConfirmResult::Aborted(why) => {
            return Err(anyhow!("fleet aborted: {why}"));
        }
    }

    // Resolve concurrency.
    let concurrency = resolve_concurrency(args.concurrency)?;

    // Self-binary path for child invocations.
    let self_exe = std::env::current_exe()
        .map_err(|e| anyhow!("cannot locate inspect binary for fleet fanout: {e}"))?;

    // Build the per-namespace plan.
    let plan: Vec<NsPlan> = chosen
        .iter()
        .map(|ns| NsPlan {
            namespace: ns.clone(),
            child_args: build_child_args(&args.verb, &args.args, ns),
            force_ns: !NAMESPACE_POSITIONAL_VERBS.contains(&args.verb.as_str()),
        })
        .collect();

    let abort_on_error = args.abort_on_error;
    let results = fan_out(&self_exe, plan, concurrency, abort_on_error);

    let total = results.len();
    let ok = results.iter().filter(|r| r.exit == 0).count();
    let failed = total - ok;

    if args.json {
        emit_json(&args.verb, &results, total, ok, failed);
    } else {
        emit_human(&args.verb, &results, total, ok, failed);
    }

    if failed == 0 {
        Ok(ExitKind::Success)
    } else if ok == 0 && results.iter().all(|r| r.exit == 1) {
        // All children reported NoMatches.
        Ok(ExitKind::NoMatches)
    } else {
        Ok(ExitKind::Error)
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
/// * `@<group>` — looked up in `~/.inspect/groups.toml`
/// * `a,b,c` — comma-separated explicit list (each entry may itself be
///   a glob)
/// * a single glob or literal
fn expand_ns_pattern(pat: &str, known: &[String]) -> Result<Vec<String>> {
    let trimmed = pat.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("--ns must not be empty"));
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
    let n = if let Some(v) = flag {
        v
    } else if let Ok(env) = std::env::var("INSPECT_FLEET_CONCURRENCY") {
        env.parse::<usize>().map_err(|e| {
            anyhow!("INSPECT_FLEET_CONCURRENCY='{env}' is not a positive integer: {e}")
        })?
    } else {
        DEFAULT_FLEET_CONCURRENCY
    };
    if n == 0 {
        return Err(anyhow!("fleet concurrency must be >= 1"));
    }
    Ok(n.min(MAX_FLEET_CONCURRENCY))
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
/// the child, captures stdout/stderr, and pushes a result back.
fn fan_out(
    self_exe: &std::path::Path,
    plan: Vec<NsPlan>,
    concurrency: usize,
    abort_on_error: bool,
) -> Vec<NsResult> {
    let total = plan.len();
    let (job_tx, job_rx) = mpsc::channel::<NsPlan>();
    let job_rx = std::sync::Arc::new(std::sync::Mutex::new(job_rx));
    let (res_tx, res_rx) = mpsc::channel::<NsResult>();
    let abort = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    let workers: Vec<_> = (0..concurrency.min(total).max(1))
        .map(|_| {
            let job_rx = job_rx.clone();
            let res_tx = res_tx.clone();
            let abort = abort.clone();
            let exe = self_exe.to_path_buf();
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
                let result = run_child(&exe, &job);
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

fn run_child(exe: &std::path::Path, plan: &NsPlan) -> NsResult {
    let mut cmd = StdCommand::new(exe);
    cmd.args(&plan.child_args);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    if plan.force_ns {
        cmd.env("INSPECT_FLEET_FORCE_NS", &plan.namespace);
    } else {
        cmd.env_remove("INSPECT_FLEET_FORCE_NS");
    }
    // Children inherit INSPECT_HOME, INSPECT_MOCK_REMOTE_FILE, etc.
    cmd.env("INSPECT_NON_INTERACTIVE", "1");

    match cmd.spawn() {
        Ok(mut child) => {
            let mut stdout = String::new();
            let mut stderr = String::new();
            if let Some(mut s) = child.stdout.take() {
                let _ = s.read_to_string(&mut stdout);
            }
            if let Some(mut s) = child.stderr.take() {
                let _ = s.read_to_string(&mut stderr);
            }
            let status = child.wait().ok();
            let exit = status.and_then(|s| s.code()).unwrap_or(2);
            NsResult {
                namespace: plan.namespace.clone(),
                exit,
                stdout,
                stderr,
            }
        }
        Err(e) => NsResult {
            namespace: plan.namespace.clone(),
            exit: 2,
            stdout: String::new(),
            stderr: format!("fleet: failed to spawn child for ns '{}': {e}", plan.namespace),
        },
    }
}

fn emit_human(verb: &str, results: &[NsResult], total: usize, ok: usize, failed: usize) {
    println!(
        "SUMMARY: fleet '{verb}' over {total} namespace(s): {ok} ok, {failed} failed"
    );
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
        println!("NEXT:    inspect fleet {verb} --ns <pattern> --json   # for machine-readable output");
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
    s.push_str(&format!("\"total\":{total},\"ok\":{ok},\"failed\":{failed}"));
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
            vec!["setup".to_string(), "prod-1".to_string(), "--force".to_string()]
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
        let known = vec!["prod-1".to_string(), "prod-2".to_string(), "staging".to_string()];
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
}
