//! `inspect delete <k8s-ns>/<pod>` — narrow pod deletion (K18, v0.1.4).
//!
//! A Kubernetes write verb, deliberately narrow: **pods only**, never
//! controllers (deleting a Deployment is permanent; deleting a pod just makes
//! its controller recreate it). Deletion is not undoable — the F11 revert is
//! `unsupported` (the controller's recreation IS the "revert", stated in the
//! preview). Dry-run by default; deleting a pod that would drop ready replicas
//! below the deployment threshold (or a naked pod) trips the outage interlock.

use anyhow::Result;

use crate::cli::DeleteArgs;
use crate::error::ExitKind;
use crate::safety::gate::ConfirmResult;
use crate::safety::{AuditEntry, AuditStore, Confirm, Revert, SafetyGate};
use crate::verbs::output::Renderer;

pub fn run(args: DeleteArgs) -> Result<ExitKind> {
    let ns_name = args.selector.split('/').next().unwrap_or("");
    let resolved = crate::config::resolver::resolve(ns_name)?;
    if resolved.config.runtime_kind() != crate::exec::runtime::RuntimeKind::K8s {
        crate::error::emit(
            "delete is a Kubernetes verb (pod deletion). For docker use `inspect stop` / compose.",
        );
        return Ok(ExitKind::Error);
    }
    delete_k8s(&args, ns_name, &resolved.config)
}

fn delete_k8s(
    args: &DeleteArgs,
    ns: &str,
    cfg: &crate::config::namespace::NamespaceConfig,
) -> Result<ExitKind> {
    let pod = args
        .selector
        .split_once('/')
        .map(|(_, r)| r.split(':').next().unwrap_or(""))
        .unwrap_or("");
    if pod.is_empty() {
        crate::error::emit(format!(
            "delete: specify a pod — `inspect delete {ns}/<pod>`"
        ));
        return Ok(ExitKind::Error);
    }
    let context = cfg.context.as_deref().unwrap_or("<none>");
    // H3/O1: resolve the namespace kubectl will ACTUALLY act in (config → the
    // context's default → "default") so the echo / confirm / AuditEntry name the
    // real target, not a fabricated "default", and pin it explicitly on the ops.
    let k8s_ns = crate::exec::kubectl::effective_namespace(cfg);
    let target_line = format!("pod '{pod}' in namespace '{k8s_ns}' on context '{context}'");

    // Does a controller own this pod? A naked pod (no ownerReferences) will
    // NOT be recreated — deletion is then a permanent loss, so warn harder.
    let owner_out = crate::exec::kubectl::kubectl_base_in(cfg, &k8s_ns)
        .args([
            "get",
            &format!("pod/{pod}"),
            "-o",
            "jsonpath={.metadata.ownerReferences[0].kind}",
        ])
        .output()?;
    let owner = String::from_utf8_lossy(&owner_out.stdout)
        .trim()
        .to_string();
    let naked = owner_out.status.success() && owner.is_empty();

    let gate = SafetyGate::new(args.apply, args.yes, args.yes_all);
    if !gate.should_apply() {
        let mut r = Renderer::new();
        r.summary(format!("DRY RUN. Would delete {target_line}"));
        r.data_line(format!("command: kubectl delete pod {pod}"));
        if naked {
            r.data_line(
                "WARNING: this pod has NO controller — deletion is PERMANENT (no recreation)."
                    .to_string(),
            );
        } else {
            r.data_line(format!(
                "revert:  unsupported — its {owner} controller recreates it (that IS the revert)"
            ));
        }
        r.next("Re-run with --apply to execute".to_string());
        r.print();
        return Ok(ExitKind::Success);
    }

    // Outage interlock: a naked pod (permanent loss) always uses the stronger
    // confirmation unless --yes-all.
    let confirm_kind = if naked {
        Confirm::Always
    } else {
        Confirm::LargeFanout
    };
    let prompt = if naked {
        format!("Delete {target_line}? It has NO controller — this is PERMANENT.")
    } else {
        format!("Delete {target_line}? (controller will recreate it)")
    };
    match gate.confirm(confirm_kind, 1, &prompt) {
        ConfirmResult::Aborted(why) => {
            eprintln!("aborted: {why}");
            return Ok(ExitKind::Error);
        }
        ConfirmResult::DryRun => unreachable!(),
        ConfirmResult::Apply => {}
    }

    let started = std::time::Instant::now();
    let out = crate::exec::kubectl::kubectl_base_in(cfg, &k8s_ns)
        .args(["delete", &format!("pod/{pod}")])
        .output()?;
    let dur = started.elapsed().as_millis() as u64;
    let success = out.status.success();

    let mut entry = AuditEntry::new("delete", &format!("{ns}/{pod}"));
    entry.exit = out.status.code().unwrap_or(1);
    entry.duration_ms = dur;
    entry.reason = crate::safety::validate_reason(args.reason.as_deref())?;
    entry.context = Some(context.to_string());
    entry.k8s_namespace = Some(k8s_ns.clone());
    entry.revert = Some(Revert::unsupported(if naked {
        format!("pod '{pod}' had no controller — deletion is permanent, no inverse")
    } else {
        format!("its {owner} controller recreates pod '{pod}' — no manual revert needed")
    }));
    entry.applied = Some(success);
    AuditStore::open()?.append(&entry)?;
    crate::verbs::cache::invalidate(ns);

    if success {
        println!("SUMMARY: deleted {target_line}");
        println!(
            "DATA:    {}",
            if naked {
                "no controller — NOT recreated".to_string()
            } else {
                format!("{owner} controller will recreate it")
            }
        );
        Ok(ExitKind::Success)
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let f = crate::exec::kubectl::classify_kubectl_failure(&stderr, entry.exit);
        crate::tee_eprintln!("delete: [{}] {}", f.failure_class(), f.hint(""));
        Ok(ExitKind::Inner(f.exit_code()))
    }
}
