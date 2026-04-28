//! `inspect rm <sel>:<path>` (bible §8). Always prompts on apply unless `-y`.

use std::time::Instant;

use anyhow::Result;

use crate::cli::PathArgArgs;
use crate::error::ExitKind;
use crate::safety::gate::ConfirmResult;
use crate::safety::snapshot::sha256_hex;
use crate::safety::{AuditEntry, AuditStore, Confirm, Revert, SafetyGate, SnapshotStore};
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::Renderer;
use crate::verbs::quote::shquote;

pub fn run(args: PathArgArgs) -> Result<ExitKind> {
    let (runner, nses, targets) = plan(&args.target)?;
    let mut steps_with_path = Vec::new();
    for s in iter_steps(&nses, &targets) {
        let Some(p) = s.path.clone() else {
            crate::error::emit("rm requires a :path on selector");
            return Ok(ExitKind::Error);
        };
        steps_with_path.push((s, p));
    }
    if steps_with_path.is_empty() {
        crate::error::emit(format!("'{}' matched no targets", args.target));
        return Ok(ExitKind::Error);
    }

    let gate = SafetyGate::new(args.apply, args.yes, args.yes_all);
    if !gate.should_apply() {
        let mut r = Renderer::new();
        r.summary(format!(
            "DRY RUN. Would rm {} path(s):",
            steps_with_path.len()
        ));
        for (s, p) in &steps_with_path {
            let svc = s.service().map(|x| format!("/{x}")).unwrap_or_default();
            r.data_line(format!("{}{svc}:{p}", s.ns.namespace));
        }
        r.next("Re-run with --apply to execute");
        r.print();
        return Ok(ExitKind::Success);
    }
    if let ConfirmResult::Aborted(why) = gate.confirm(
        Confirm::Always,
        steps_with_path.len(),
        &format!("Delete {} file(s)?", steps_with_path.len()),
    ) {
        eprintln!("aborted: {why}");
        return Ok(ExitKind::Error);
    }

    let store = AuditStore::open()?;
    let snaps = SnapshotStore::open()?;
    let mut ok = 0usize;
    let mut bad = 0usize;
    let mut renderer = Renderer::new();

    for (s, path) in &steps_with_path {
        // F11 (v0.1.3): snapshot the file content before deletion so
        // `inspect revert` can restore it via the snapshot store.
        // base64 -w0 keeps the wire payload single-line.
        let cat_inner = format!("base64 -w0 -- {}", shquote(path));
        let cat_cmd = match s.container() {
            Some(container) => format!(
                "docker exec {} sh -c {}",
                shquote(container),
                shquote(&cat_inner)
            ),
            None => cat_inner,
        };
        let prev_hash = match runner.run(
            &s.ns.namespace,
            &s.ns.target,
            &cat_cmd,
            RunOpts::with_timeout(60),
        ) {
            Ok(o) if o.ok() => {
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD
                    .decode(o.stdout.trim())
                    .ok()
                    .map(|bytes| {
                        let h = sha256_hex(&bytes);
                        let _ = snaps.put(&bytes);
                        h
                    })
            }
            _ => None,
        };
        let label = format!(
            "{}{}:{path}",
            s.ns.namespace,
            s.service().map(|x| format!("/{x}")).unwrap_or_default()
        );
        let revert = match prev_hash.as_ref() {
            Some(h) => Revert::state_snapshot(
                format!("sha256:{h}"),
                format!("restore {label} from snapshot sha256:{}", &h[..12]),
            ),
            None => Revert::unsupported(format!(
                "could not snapshot {path} before delete; revert unavailable"
            )),
        };
        if args.revert_preview {
            eprintln!(
                "[inspect] revert preview {label}: {kind} -- {preview}",
                kind = revert.kind.as_str(),
                preview = revert.preview,
            );
        }
        let inner = format!("rm -f -- {}", shquote(path));
        let cmd = match s.container() {
            Some(container) => format!(
                "docker exec {} sh -c {}",
                shquote(container),
                shquote(&inner)
            ),
            None => inner.clone(),
        };
        let started = Instant::now();
        let out = runner.run(
            &s.ns.namespace,
            &s.ns.target,
            &cmd,
            RunOpts::with_timeout(30),
        )?;
        let dur = started.elapsed().as_millis() as u64;

        let mut e = AuditEntry::new("rm", &label);
        e.previous_hash = prev_hash.as_ref().map(|h| format!("sha256:{h}"));
        e.snapshot = prev_hash
            .as_ref()
            .map(|h| snaps.path_for(h).display().to_string());
        e.exit = out.exit_code;
        e.duration_ms = dur;
        e.reason = crate::safety::validate_reason(args.reason.as_deref())?;
        e.revert = Some(revert);
        e.applied = Some(out.ok());
        store.append(&e)?;

        if out.ok() {
            ok += 1;
            renderer.data_line(format!("{label}: removed"));
        } else {
            bad += 1;
            renderer.data_line(format!(
                "{label}: FAILED (exit {}): {}",
                out.exit_code,
                out.stderr.trim()
            ));
        }
    }
    renderer
        .summary(format!("rm: {ok} ok, {bad} failed"))
        .next("inspect audit ls");
    renderer.print();
    Ok(if bad == 0 {
        ExitKind::Success
    } else {
        ExitKind::Error
    })
}
