//! `inspect rollout <k8s-ns>/<deploy> [--to-revision N]` — roll a Deployment
//! back to a prior revision (K17, v0.1.4).
//!
//! The community's documented fast rollback of a bad deploy (`kubectl rollout
//! undo`). Low-risk: it moves to an EXISTING prior revision. Its own inverse
//! is another rollout undo — so the F11 revert is a `command_pair` back to the
//! pre-undo revision (captured first). Dry-run by default; resolved-target
//! echo (K5) on every path.

use anyhow::Result;

use crate::cli::RolloutArgs;
use crate::error::ExitKind;
use crate::safety::gate::ConfirmResult;
use crate::safety::{AuditEntry, AuditStore, Confirm, Revert, SafetyGate};
use crate::verbs::output::Renderer;

pub fn run(args: RolloutArgs) -> Result<ExitKind> {
    let ns_name = args.selector.split('/').next().unwrap_or("");
    let resolved = crate::config::resolver::resolve(ns_name)?;
    if resolved.config.runtime_kind() != crate::exec::runtime::RuntimeKind::K8s {
        crate::error::emit("rollout is a Kubernetes verb (rollout undo).");
        return Ok(ExitKind::Error);
    }
    rollout_k8s(&args, ns_name, &resolved.config)
}

fn rollout_k8s(
    args: &RolloutArgs,
    ns: &str,
    cfg: &crate::config::namespace::NamespaceConfig,
) -> Result<ExitKind> {
    let workload = args.selector.split_once('/').map(|(_, r)| r).unwrap_or("");
    if workload.is_empty() {
        crate::error::emit(format!("rollout: specify a workload — `{ns}/<deploy>`"));
        return Ok(ExitKind::Error);
    }
    let context = cfg.context.as_deref().unwrap_or("<none>");
    let k8s_ns = cfg.k8s_namespace.as_deref().unwrap_or("default");
    let to = args
        .to_revision
        .map(|n| format!(" to revision {n}"))
        .unwrap_or_else(|| " to the previous revision".to_string());
    let target_line =
        format!("deploy/{workload}{to} in namespace '{k8s_ns}' on context '{context}'");

    // The current revision is the undo target for the F11 revert.
    let cur_out = crate::exec::kubectl::kubectl_base(cfg)
        .args([
            "get",
            &format!("deploy/{workload}"),
            "-o",
            "jsonpath={.metadata.annotations.deployment\\.kubernetes\\.io/revision}",
        ])
        .output()?;
    let current_rev = String::from_utf8_lossy(&cur_out.stdout).trim().to_string();

    let gate = SafetyGate::new(args.apply, args.yes, args.yes_all);
    if !gate.should_apply() {
        let mut r = Renderer::new();
        r.summary(format!("DRY RUN. Would roll back {target_line}"));
        let mut cmd = format!("kubectl rollout undo deploy/{workload}");
        if let Some(n) = args.to_revision {
            cmd.push_str(&format!(" --to-revision={n}"));
        }
        r.data_line(format!("command: {cmd}"));
        if current_rev.is_empty() {
            r.data_line("revert:  <current revision unavailable>".to_string());
        } else {
            r.data_line(format!(
                "revert:  kubectl rollout undo --to-revision={current_rev} (current revision)"
            ));
        }
        r.next("Re-run with --apply to execute".to_string());
        r.print();
        return Ok(ExitKind::Success);
    }
    match gate.confirm(
        Confirm::LargeFanout,
        1,
        &format!("Roll back {target_line}?"),
    ) {
        ConfirmResult::Aborted(why) => {
            eprintln!("aborted: {why}");
            return Ok(ExitKind::Error);
        }
        ConfirmResult::DryRun => unreachable!(),
        ConfirmResult::Apply => {}
    }

    let revert = if current_rev.is_empty() {
        Revert::unsupported(format!(
            "kubectl rollout undo deploy/{workload} (revision unknown)"
        ))
    } else {
        Revert::command_pair(
            format!(
                "{} rollout undo deploy/{workload} --to-revision={current_rev}",
                crate::exec::kubectl::revert_kubectl_prefix(cfg)
            ),
            format!("rollout undo deploy/{workload} back to revision {current_rev}"),
        )
    };
    if args.revert_preview {
        eprintln!(
            "[inspect] revert preview {ns}/{workload}: {}",
            revert.preview
        );
    }

    let mut cmd = crate::exec::kubectl::kubectl_base(cfg);
    cmd.args(["rollout", "undo", &format!("deploy/{workload}")]);
    if let Some(n) = args.to_revision {
        cmd.arg(format!("--to-revision={n}"));
    }
    let started = std::time::Instant::now();
    let out = cmd.output()?;
    let dur = started.elapsed().as_millis() as u64;
    let success = out.status.success();

    let mut entry = AuditEntry::new("rollout", &format!("{ns}/{workload}"));
    entry.exit = out.status.code().unwrap_or(1);
    entry.duration_ms = dur;
    entry.reason = crate::safety::validate_reason(args.reason.as_deref())?;
    entry.context = Some(context.to_string());
    entry.k8s_namespace = Some(k8s_ns.to_string());
    entry.revert = Some(revert);
    entry.applied = Some(success);
    AuditStore::open()?.append(&entry)?;
    crate::verbs::cache::invalidate(ns);

    if success {
        println!("SUMMARY: rolled back {target_line}");
        println!("DATA:    revert captured (rollout undo to revision {current_rev})");
        println!("NEXT:    inspect audit ls   inspect status {ns}/{workload}");
        Ok(ExitKind::Success)
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let f = crate::exec::kubectl::classify_kubectl_failure(&stderr, entry.exit);
        crate::tee_eprintln!("rollout: [{}] {}", f.failure_class(), f.hint(""));
        Ok(ExitKind::Inner(f.exit_code()))
    }
}
