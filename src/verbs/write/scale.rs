//! `inspect scale <k8s-ns>/<workload> --replicas N` (K15, v0.1.4).
//!
//! A Kubernetes write verb: `kubectl scale deploy/<w> --replicas=N`. The
//! cleanest revertible write — the F11 inverse is a `kubectl scale` back to
//! the prior replica count (a real `command_pair`). Dry-run by default; every
//! path echoes the resolved {context, k8s_namespace, workload} (K5). Scaling
//! to 0 (a full outage) trips an extra confirmation unless `--yes-all`.

use anyhow::Result;

use crate::cli::ScaleArgs;
use crate::error::ExitKind;
use crate::safety::gate::ConfirmResult;
use crate::safety::{AuditEntry, AuditStore, Confirm, Revert, SafetyGate};
use crate::verbs::output::Renderer;

pub fn run(args: ScaleArgs) -> Result<ExitKind> {
    let Some((ns_name, cfg)) = crate::verbs::dispatch::require_k8s(
        &args.selector,
        "scale is a Kubernetes verb. For docker use `inspect restart` / compose.",
    )?
    else {
        return Ok(ExitKind::Error);
    };
    scale_k8s(&args, &ns_name, &cfg)
}

fn scale_k8s(
    args: &ScaleArgs,
    ns: &str,
    cfg: &crate::config::namespace::NamespaceConfig,
) -> Result<ExitKind> {
    let workload = args.selector.split_once('/').map(|(_, r)| r).unwrap_or("");
    if workload.is_empty() {
        crate::error::emit(format!("scale: specify a workload — `{ns}/<deploy>`"));
        return Ok(ExitKind::Error);
    }
    let context = cfg.context.as_deref().unwrap_or("<none>");
    // H3/O1: resolve the namespace kubectl will ACTUALLY act in (config → the
    // context's default → "default") so the echo / confirm / AuditEntry name
    // the real target, not a fabricated "default", and pin it explicitly below.
    let k8s_ns = crate::exec::kubectl::effective_namespace(cfg);
    let target_line = format!(
        "deploy/{workload} to {} replica(s) in namespace '{k8s_ns}' on context '{context}'",
        args.replicas
    );

    // Capture the current replica count first — it is the revert target.
    let cur_out = crate::exec::kubectl::kubectl_base_in(cfg, &k8s_ns)
        .args([
            "get",
            &format!("deploy/{workload}"),
            "-o",
            "jsonpath={.spec.replicas}",
        ])
        .output()?;
    let prior: Option<u32> = if cur_out.status.success() {
        String::from_utf8_lossy(&cur_out.stdout).trim().parse().ok()
    } else {
        None
    };

    let gate = SafetyGate::new(args.apply, args.yes, args.yes_all);
    if !gate.should_apply() {
        let mut r = Renderer::new();
        r.summary(format!("DRY RUN. Would scale {target_line}"));
        r.data_line(format!(
            "command: kubectl scale deploy/{workload} --replicas={}",
            args.replicas
        ));
        match prior {
            Some(p) => r.data_line(format!(
                "revert:  kubectl scale deploy/{workload} --replicas={p} (current count)"
            )),
            None => r.data_line("revert:  <current replica count unavailable>".to_string()),
        };
        if args.replicas == 0 {
            r.data_line("WARNING: --replicas 0 is a full outage of this workload".to_string());
        }
        r.next("Re-run with --apply to execute".to_string());
        r.print();
        return Ok(ExitKind::Success);
    }

    // Outage guard: scaling to 0 needs the stronger interlock unless --yes-all.
    let confirm_kind = if args.replicas == 0 {
        Confirm::Always
    } else {
        Confirm::LargeFanout
    };
    let prompt = if args.replicas == 0 {
        format!("Scale {target_line}? THIS IS A FULL OUTAGE.")
    } else {
        format!("Scale {target_line}?")
    };
    match gate.confirm(confirm_kind, 1, &prompt) {
        ConfirmResult::Aborted(why) => {
            eprintln!("aborted: {why}");
            return Ok(ExitKind::Error);
        }
        ConfirmResult::DryRun => unreachable!(),
        ConfirmResult::Apply => {}
    }

    let revert = match prior {
        Some(p) => Revert::command_pair(
            format!(
                "{} scale deploy/{workload} --replicas={p}",
                crate::exec::kubectl::revert_kubectl_prefix_in(cfg, &k8s_ns)
            ),
            format!("scale deploy/{workload} back to {p} replica(s)"),
        ),
        None => Revert::unsupported(format!(
            "kubectl scale deploy/{workload} --replicas=<prior> (prior count unknown)"
        )),
    };
    if args.revert_preview {
        eprintln!(
            "[inspect] revert preview {ns}/{workload}: {}",
            revert.preview
        );
    }

    let mut cmd = crate::exec::kubectl::kubectl_base_in(cfg, &k8s_ns);
    cmd.args([
        "scale",
        &format!("deploy/{workload}"),
        &format!("--replicas={}", args.replicas),
    ]);
    if let Some(cr) = args.current_replicas {
        cmd.arg(format!("--current-replicas={cr}"));
    }
    let started = std::time::Instant::now();
    let out = cmd.output()?;
    let dur = started.elapsed().as_millis() as u64;
    let success = out.status.success();

    let mut entry = AuditEntry::new("scale", &format!("{ns}/{workload}"));
    entry.exit = out.status.code().unwrap_or(1);
    entry.duration_ms = dur;
    entry.args = format!("replicas={}", args.replicas);
    entry.reason = crate::safety::validate_reason(args.reason.as_deref())?;
    entry.context = Some(context.to_string());
    entry.k8s_namespace = Some(k8s_ns.clone());
    entry.revert = Some(revert);
    entry.applied = Some(success);
    AuditStore::open()?.append(&entry)?;
    crate::verbs::cache::invalidate(ns);

    if success {
        println!("SUMMARY: scaled {target_line}");
        if let Some(p) = prior {
            println!("DATA:    revert captured (scale back to {p} replica(s))");
        }
        println!("NEXT:    inspect audit ls   inspect status {ns}/{workload}");
        Ok(ExitKind::Success)
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let f = crate::exec::kubectl::classify_kubectl_failure(&stderr, entry.exit);
        crate::tee_eprintln!(
            "scale: [{}] {}",
            f.failure_class(),
            crate::exec::kubectl::deploy_write_hint(f, workload, &k8s_ns, context)
        );
        Ok(ExitKind::Inner(f.exit_code()))
    }
}
