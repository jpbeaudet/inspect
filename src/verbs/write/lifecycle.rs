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
use crate::safety::{AuditEntry, AuditStore, Confirm, SafetyGate};
use crate::safety::gate::ConfirmResult;
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
            r.data_line(format!(
                "{}/{}",
                s.ns.namespace,
                s.service().unwrap_or("?")
            ));
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

    for s in &steps {
        let svc = s.service().unwrap_or("?");
        let kind = s.service_def().map(|d| d.kind).unwrap_or(ServiceKind::Container);
        let cmd = build_cmd(act, svc, kind);
        let started = Instant::now();
        let out = runner.run(&s.ns.namespace, &s.ns.target, &cmd, RunOpts::with_timeout(60))?;
        let dur = started.elapsed().as_millis() as u64;

        let mut entry = AuditEntry::new(act.as_str(), &format!("{}/{svc}", s.ns.namespace));
        entry.exit = out.exit_code;
        entry.duration_ms = dur;
        store.append(&entry)?;

        if out.ok() {
            ok += 1;
            renderer.data_line(format!(
                "{}/{svc}: {}",
                s.ns.namespace,
                act.past_tense()
            ));
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

    renderer
        .summary(format!(
            "{action}: {ok} ok, {bad} failed",
            action = act.as_str()
        ))
        .next("inspect audit ls");
    renderer.print();

    Ok(if bad == 0 { ExitKind::Success } else { ExitKind::Error })
}

fn build_cmd(act: Action, svc: &str, kind: ServiceKind) -> String {
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
        // container default
        (_, Action::Restart) => format!("docker restart {svc_q}"),
        (_, Action::Stop) => format!("docker stop {svc_q}"),
        (_, Action::Start) => format!("docker start {svc_q}"),
        (_, Action::Reload) => {
            // Best-effort SIGHUP into the container.
            format!("docker kill -s HUP {svc_q}")
        }
    }
}
