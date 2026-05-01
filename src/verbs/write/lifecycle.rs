//! `restart` / `stop` / `start` / `reload` (bible §8.1).
//!
//! All four share the same shape: a docker (container) or systemctl
//! (systemd) command applied to each resolved target. Implemented here
//! together so the dry-run renderer and audit recording stay in one place.

use std::time::Instant;

use anyhow::Result;

use crate::cli::LifecycleArgs;
use crate::error::ExitKind;
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
    // F8 (v0.1.3): track every namespace that successfully had at
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
        // F11 (v0.1.3): capture-before-apply. Build the inverse
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

    // F8: invalidate runtime cache for every namespace touched.
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
    let cont_q = shquote(container);
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
        // container default
        (_, Action::Restart) => format!("docker restart {cont_q}"),
        (_, Action::Stop) => format!("docker stop {cont_q}"),
        (_, Action::Start) => format!("docker start {cont_q}"),
        (_, Action::Reload) => {
            // Best-effort SIGHUP into the container.
            format!("docker kill -s HUP {cont_q}")
        }
    }
}

/// F11 (v0.1.3): pre-stage the inverse of a lifecycle action so it
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
