//! `restart` / `stop` / `start` / `reload` (bible §8.1).
//!
//! All four share the same shape: a docker (container) or systemctl
//! (systemd) command applied to each resolved target. Implemented here
//! together so the dry-run renderer and audit recording stay in one place.

use std::time::Instant;

use anyhow::Result;

use crate::cli::LifecycleArgs;
use crate::error::ExitKind;
use crate::exec::runtime::{runtime_for, LifecycleAction, RuntimeKind};
use crate::profile::schema::ServiceKind;
use crate::safety::gate::ConfirmResult;
use crate::safety::{AuditEntry, AuditStore, Confirm, Revert, SafetyGate};
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan, Step};
use crate::verbs::output::Renderer;
use crate::verbs::quote::shquote;

#[derive(Clone, Copy, Debug)]
pub enum Action {
    Restart,
    Stop,
    Start,
    Reload,
}

impl Action {
    fn as_str(self) -> &'static str {
        match self {
            Action::Restart => "restart",
            Action::Stop => "stop",
            Action::Start => "start",
            Action::Reload => "reload",
        }
    }
    fn past_tense(self) -> &'static str {
        match self {
            Action::Restart => "restarted",
            Action::Stop => "stopped",
            Action::Start => "started",
            Action::Reload => "reloaded",
        }
    }
}

pub fn restart(args: LifecycleArgs) -> Result<ExitKind> {
    run(Action::Restart, args)
}
pub fn stop(args: LifecycleArgs) -> Result<ExitKind> {
    run(Action::Stop, args)
}
pub fn start(args: LifecycleArgs) -> Result<ExitKind> {
    run(Action::Start, args)
}
pub fn reload(args: LifecycleArgs) -> Result<ExitKind> {
    run(Action::Reload, args)
}

fn run(act: Action, args: LifecycleArgs) -> Result<ExitKind> {
    // K16 (v0.1.4): a k8s namespace runs the lifecycle op via kubectl —
    // `restart`/`reload` -> `kubectl rollout restart`; `stop`/`start` refuse
    // with a `scale --replicas` hint (K20). Every write echoes the resolved
    // {context, namespace, workload} (K5 anti-footgun) and is dry-run by
    // default. Branch before the SSH plan().
    if let Some(ns_name) = args.selector.split('/').next() {
        if let Ok(resolved) = crate::config::resolver::resolve(ns_name) {
            if resolved.config.runtime_kind() == crate::exec::runtime::RuntimeKind::K8s {
                return lifecycle_k8s(act, &args, ns_name, &resolved.config);
            }
        }
    }

    let (runner, nses, targets) = plan(&args.selector)?;
    let steps: Vec<Step> = iter_steps(&nses, &targets)
        .filter(|s| s.service().is_some())
        .collect();
    if steps.is_empty() {
        crate::error::emit(format!("'{}' matched no service targets", args.selector));
        return Ok(ExitKind::Error);
    }

    let gate = SafetyGate::new(args.apply, args.yes, args.yes_all);
    if !gate.should_apply() {
        // Dry-run preview.
        let mut r = Renderer::new();
        r.summary(format!(
            "DRY RUN. Would {} {} service(s):",
            act.as_str(),
            steps.len()
        ));
        for s in &steps {
            r.data_line(format!("{}/{}", s.ns.namespace, s.service().unwrap_or("?")));
        }
        r.next("Re-run with --apply to execute".to_string());
        r.print();
        return Ok(ExitKind::Success);
    }
    match gate.confirm(Confirm::LargeFanout, steps.len(), "Continue?") {
        ConfirmResult::Aborted(why) => {
            eprintln!("aborted: {why}");
            return Ok(ExitKind::Error);
        }
        ConfirmResult::DryRun => unreachable!(),
        ConfirmResult::Apply => {}
    }

    let store = AuditStore::open()?;
    let mut ok = 0usize;
    let mut bad = 0usize;
    let mut renderer = Renderer::new();
    // Track every namespace that successfully had at
    // least one service mutated, so we can invalidate the runtime
    // cache exactly once per ns at the end. Without this, the next
    // `inspect status` would happily serve the pre-restart snapshot
    // for up to TTL seconds — exactly the 3rd field user's bug.
    let mut mutated_namespaces: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();

    for s in &steps {
        let svc = s.service().unwrap_or("?");
        let container = s.container().unwrap_or(svc);
        let kind = s
            .service_def()
            .map(|d| d.kind)
            .unwrap_or(ServiceKind::Container);
        let cmd = build_cmd(act, svc, container, kind);
        // Capture-before-apply. Build the inverse
        // *before* dispatching so the audit entry records what
        // `inspect revert` would run, even on partial failure.
        let revert = build_revert(act, svc, container, kind);
        if args.revert_preview {
            eprintln!(
                "[inspect] revert preview {ns}/{svc}: {kind} -- {preview}",
                ns = s.ns.namespace,
                svc = svc,
                kind = revert.kind.as_str(),
                preview = revert.preview,
            );
        }
        let started = Instant::now();
        let out = runner.run(
            &s.ns.namespace,
            &s.ns.target,
            &cmd,
            RunOpts::with_timeout(60),
        )?;
        let dur = started.elapsed().as_millis() as u64;

        let mut entry = AuditEntry::new(act.as_str(), &format!("{}/{svc}", s.ns.namespace));
        entry.exit = out.exit_code;
        entry.duration_ms = dur;
        entry.reason = crate::safety::validate_reason(args.reason.as_deref())?;
        entry.revert = Some(revert);
        entry.applied = Some(out.ok());
        store.append(&entry)?;

        if out.ok() {
            ok += 1;
            mutated_namespaces.insert(s.ns.namespace.clone());
            renderer.data_line(format!("{}/{svc}: {}", s.ns.namespace, act.past_tense()));
        } else {
            bad += 1;
            renderer.data_line(format!(
                "{}/{svc}: FAILED (exit {}): {}",
                s.ns.namespace,
                out.exit_code,
                out.stderr.trim()
            ));
        }
    }

    // Invalidate runtime cache for every namespace touched.
    // Best-effort: invalidation is a file unlink; if it fails (e.g.
    // permissions) the next read verb's TTL check still protects
    // freshness within `INSPECT_RUNTIME_TTL_SECS`. We intentionally
    // never fail the whole verb on this.
    for ns in &mutated_namespaces {
        crate::verbs::cache::invalidate(ns);
    }

    renderer
        .summary(format!(
            "{action}: {ok} ok, {bad} failed",
            action = act.as_str()
        ))
        .next("inspect audit ls");
    renderer.print();

    Ok(if bad == 0 {
        ExitKind::Success
    } else {
        ExitKind::Error
    })
}

fn build_cmd(act: Action, svc: &str, container: &str, kind: ServiceKind) -> String {
    // For systemd / host-listener we operate on the user-facing name
    // (the unit name). For containers we operate on the real
    // container name to defeat the v0.1.0 phantom-service bug.
    let svc_q = shquote(svc);
    match (kind, act) {
        // systemd unit → systemctl
        (ServiceKind::Systemd, Action::Restart) => format!("systemctl restart {svc_q}"),
        (ServiceKind::Systemd, Action::Stop) => format!("systemctl stop {svc_q}"),
        (ServiceKind::Systemd, Action::Start) => format!("systemctl start {svc_q}"),
        (ServiceKind::Systemd, Action::Reload) => format!("systemctl reload {svc_q}"),
        // host listener → kill -HUP for reload, otherwise no-op-ish
        (ServiceKind::HostListener, Action::Reload) => {
            format!("pkill -HUP -f {svc_q} || true")
        }
        // Container default — dispatched through the runtime executor
        // seam (K1, v0.1.4). `from_type(None)` selects docker today;
        // K2 wires the namespace `type` config field so a k8s namespace
        // routes to `rollout restart`/`scale` instead. Byte-identical
        // to the prior inline `docker restart|stop|start|kill -s HUP`
        // strings (HostListener Restart/Stop/Start still fall through
        // here, unchanged).
        (_, act) => {
            let action = match act {
                Action::Restart => LifecycleAction::Restart,
                Action::Stop => LifecycleAction::Stop,
                Action::Start => LifecycleAction::Start,
                Action::Reload => LifecycleAction::Reload,
            };
            runtime_for(RuntimeKind::from_type(None)).build_lifecycle(action, container)
        }
    }
}

/// Pre-stage the inverse of a lifecycle action so it
/// can be reapplied via `inspect revert` even if the original step
/// failed mid-flight. `restart` and `reload` have no clean inverse,
/// so they record `kind: unsupported` with a human-readable preview.
fn build_revert(act: Action, svc: &str, container: &str, kind: ServiceKind) -> Revert {
    let svc_q = shquote(svc);
    let cont_q = shquote(container);
    match (kind, act) {
        (ServiceKind::Systemd, Action::Stop) => Revert::command_pair(
            format!("systemctl start {svc_q}"),
            format!("systemctl start {svc}"),
        ),
        (ServiceKind::Systemd, Action::Start) => Revert::command_pair(
            format!("systemctl stop {svc_q}"),
            format!("systemctl stop {svc}"),
        ),
        (_, Action::Stop) => Revert::command_pair(
            format!("docker start {cont_q}"),
            format!("docker start {container}"),
        ),
        (_, Action::Start) => Revert::command_pair(
            format!("docker stop {cont_q}"),
            format!("docker stop {container}"),
        ),
        (_, Action::Restart) => Revert::unsupported(format!(
            "restart has no inverse; re-run `inspect restart {svc}` to repeat"
        )),
        (_, Action::Reload) => {
            Revert::unsupported(format!("reload (SIGHUP) has no inverse for {svc}"))
        }
    }
}

/// K16 (v0.1.4): k8s lifecycle via kubectl. `restart`/`reload` map to
/// `kubectl rollout restart deploy/<w>`; `stop`/`start` refuse with a
/// `scale --replicas` hint (K20). Dry-run by default; on `--apply` the
/// current rollout revision is captured FIRST so the F11 revert is a real
/// `rollout undo --to-revision=<n>` command_pair. Every path echoes the
/// resolved {context, k8s_namespace, workload} (K5 anti-footgun).
fn lifecycle_k8s(
    act: Action,
    args: &LifecycleArgs,
    ns: &str,
    cfg: &crate::config::namespace::NamespaceConfig,
) -> Result<ExitKind> {
    let workload = args.selector.split_once('/').map(|(_, r)| r).unwrap_or("");
    if workload.is_empty() {
        crate::error::emit(format!(
            "{}: specify a workload — `{ns}/<deploy>`",
            act.as_str()
        ));
        return Ok(ExitKind::Error);
    }
    // Only rollout-restart is a supported k8s write here; stop/start -> scale.
    if !matches!(act, Action::Restart | Action::Reload) {
        crate::error::emit(format!(
            "{act} is not supported for k8s pods — scale the workload instead: \
             `inspect scale {ns}/{workload} --replicas=<n>` (0 to stop). Pods are \
             immutable; there is no stop/start.",
            act = act.as_str(),
        ));
        return Ok(ExitKind::Error);
    }

    let context = cfg.context.as_deref().unwrap_or("<none>");
    // H3/O1: resolve the namespace kubectl will ACTUALLY act in (config → the
    // context's default → "default") so the echo / confirm / AuditEntry name the
    // real target, not a fabricated "default", and pin it explicitly on the ops.
    let k8s_ns = crate::exec::kubectl::effective_namespace(cfg);
    // The K5 anti-footgun echo — which cluster + namespace + workload.
    let target_line = format!("deploy/{workload} in namespace '{k8s_ns}' on context '{context}'");

    let gate = SafetyGate::new(args.apply, args.yes, args.yes_all);
    if !gate.should_apply() {
        let mut r = Renderer::new();
        r.summary(format!("DRY RUN. Would rollout-restart {target_line}"));
        r.data_line(format!(
            "command: kubectl rollout restart deploy/{workload}"
        ));
        r.data_line(
            "revert:  kubectl rollout undo --to-revision=<current> (captured at --apply time)"
                .to_string(),
        );
        r.next("Re-run with --apply to execute".to_string());
        r.print();
        return Ok(ExitKind::Success);
    }
    match gate.confirm(
        Confirm::LargeFanout,
        1,
        &format!("Rollout-restart {target_line}?"),
    ) {
        ConfirmResult::Aborted(why) => {
            eprintln!("aborted: {why}");
            return Ok(ExitKind::Error);
        }
        ConfirmResult::DryRun => unreachable!(),
        ConfirmResult::Apply => {}
    }

    // F11 capture-before-apply: the current revision is the undo target.
    let rev_out = crate::exec::kubectl::kubectl_base_in(cfg, &k8s_ns)
        .args([
            "get",
            &format!("deploy/{workload}"),
            "-o",
            "jsonpath={.metadata.annotations.deployment\\.kubernetes\\.io/revision}",
        ])
        .output()?;
    let current_rev = String::from_utf8_lossy(&rev_out.stdout).trim().to_string();
    let revert = if current_rev.is_empty() {
        Revert::unsupported(format!(
            "kubectl rollout undo deploy/{workload} (revision unknown)"
        ))
    } else {
        Revert::command_pair(
            format!(
                "{} rollout undo deploy/{workload} --to-revision={current_rev}",
                crate::exec::kubectl::revert_kubectl_prefix_in(cfg, &k8s_ns)
            ),
            format!("rollout undo deploy/{workload} to revision {current_rev}"),
        )
    };
    if args.revert_preview {
        eprintln!(
            "[inspect] revert preview {ns}/{workload}: {}",
            revert.preview
        );
    }

    let started = Instant::now();
    let out = crate::exec::kubectl::kubectl_base_in(cfg, &k8s_ns)
        .args(["rollout", "restart", &format!("deploy/{workload}")])
        .output()?;
    let dur = started.elapsed().as_millis() as u64;
    let success = out.status.success();

    let mut entry = AuditEntry::new("restart", &format!("{ns}/{workload}"));
    entry.exit = out.status.code().unwrap_or(1);
    entry.duration_ms = dur;
    entry.reason = crate::safety::validate_reason(args.reason.as_deref())?;
    entry.context = Some(context.to_string());
    entry.k8s_namespace = Some(k8s_ns.clone());
    entry.revert = Some(revert);
    entry.applied = Some(success);
    AuditStore::open()?.append(&entry)?;
    crate::verbs::cache::invalidate(ns);

    if success {
        println!("SUMMARY: rollout-restarted {target_line}");
        println!("DATA:    revert captured (rollout undo to revision {current_rev})");
        println!("NEXT:    inspect audit ls   inspect status {ns}/{workload}");
        Ok(ExitKind::Success)
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let f = crate::exec::kubectl::classify_kubectl_failure(&stderr, entry.exit);
        crate::tee_eprintln!("restart: [{}] {}", f.failure_class(), f.hint(""));
        Ok(ExitKind::Inner(f.exit_code()))
    }
}
