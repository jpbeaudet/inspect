//! `inspect cp` — push or pull a single file (bible §8).
//!
//! Direction is inferred from arg shape:
//! - push: `cp <local> <sel>:<path>`
//! - pull: `cp <sel>:<path> <local>`
//!
//! Pull has no preview semantics (it's read-shaped from the remote's
//! perspective) and runs immediately. Push is dry-run by default and
//! shows a unified diff against the existing remote content if any.
//!
//! Implementation is base64-inline (no SCP) so the same code path works
//! through the multiplexed SSH master we already have. Files larger than
//! 4 MiB are rejected — for big payloads the operator should use raw
//! scp or `inspect exec`.

use std::time::Instant;

use anyhow::Result;
use base64::Engine as _;

use crate::cli::CpArgs;
use crate::error::ExitKind;
use crate::safety::gate::ConfirmResult;
use crate::safety::{
    diff::{diff_summary, unified_diff},
    snapshot::sha256_hex,
    AuditEntry, AuditStore, Confirm, Revert, SafetyGate, SnapshotStore,
};
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::Renderer;
use crate::verbs::quote::shquote;

const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;
/// Field pitfall §1.4: a single bulk transfer through the multiplexed
/// SSH master temporarily monopolises the channel and starves any
/// concurrent interactive verbs against the same host. The warning
/// fires above this size; the operator can tune via
/// `INSPECT_CP_WARN_BYTES=<n>` (0 disables).
const DEFAULT_LARGE_FILE_WARN_BYTES: usize = 1024 * 1024;

fn large_file_warn_threshold() -> usize {
    if let Ok(s) = std::env::var("INSPECT_CP_WARN_BYTES") {
        if let Ok(n) = s.parse::<usize>() {
            return n;
        }
    }
    DEFAULT_LARGE_FILE_WARN_BYTES
}

pub fn run(args: CpArgs) -> Result<ExitKind> {
    let (src, dst) = (args.source.clone(), args.dest.clone());
    let src_remote = looks_remote(&src);
    let dst_remote = looks_remote(&dst);
    match (src_remote, dst_remote) {
        (false, true) => push(args, src, dst),
        (true, false) => pull(args, src, dst),
        (false, false) => {
            crate::error::emit("cp needs at least one remote endpoint (selector with `:path`)");
            Ok(ExitKind::Error)
        }
        (true, true) => {
            crate::error::emit("cp does not support remote→remote in v1");
            Ok(ExitKind::Error)
        }
    }
}

fn looks_remote(s: &str) -> bool {
    // A "remote" arg is anything containing a `:` followed by a `/`-prefixed path.
    if let Some((before, after)) = s.split_once(':') {
        !before.is_empty() && after.starts_with('/')
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// PUSH
// ---------------------------------------------------------------------------
fn push(args: CpArgs, local: String, remote_sel: String) -> Result<ExitKind> {
    let body = std::fs::read(&local)
        .map_err(|e| anyhow::anyhow!("reading local source '{local}': {e}"))?;
    if body.len() > MAX_FILE_BYTES {
        eprintln!(
            "error: file '{local}' is {} bytes (>{} max for inline transfer); use scp",
            body.len(),
            MAX_FILE_BYTES
        );
        return Ok(ExitKind::Error);
    }
    // Field pitfall §1.4: warn when one push will saturate the shared
    // multiplexed SSH channel for several seconds and starve any
    // concurrent interactive verb against the same host.
    let warn_bytes = large_file_warn_threshold();
    if warn_bytes > 0 && body.len() >= warn_bytes {
        eprintln!(
            "inspect: warning: pushing {} bytes through the multiplexed SSH channel \
             will briefly starve concurrent verbs against the same host. \
             For sustained bulk transfers use raw `scp` over a dedicated connection. \
             (silence with INSPECT_CP_WARN_BYTES=0)",
            body.len()
        );
    }

    let (runner, nses, targets) = plan(&remote_sel)?;
    let mut planned = Vec::new();
    for s in iter_steps(&nses, &targets) {
        let Some(p) = s.path.clone() else {
            crate::error::emit("cp push requires a `:path` on the destination selector");
            return Ok(ExitKind::Error);
        };
        planned.push((s, p));
    }
    if planned.is_empty() {
        crate::error::emit("'{remote_sel}' matched no targets");
        return Ok(ExitKind::Error);
    }

    let gate = SafetyGate::new(args.apply, args.yes, args.yes_all);

    // For dry-run / --diff, pull the existing remote content (best-effort)
    // and render a diff per target.
    let new_text = String::from_utf8_lossy(&body).to_string();
    let mut diffs: Vec<(String, String, String)> = Vec::new(); // (label, old, new)
    for (s, path) in &planned {
        let old = read_remote(&*runner, s, path).unwrap_or_default();
        let label = format!(
            "{}{}:{path}",
            s.ns.namespace,
            s.service().map(|x| format!("/{x}")).unwrap_or_default()
        );
        diffs.push((label, old, new_text.clone()));
    }

    if !gate.should_apply() {
        let mut r = Renderer::new();
        let summary_diffs: Vec<(String, String)> = diffs
            .iter()
            .map(|(_, o, n)| (o.clone(), n.clone()))
            .collect();
        r.summary(format!(
            "DRY RUN. Would push {} → {} target(s) [{}]",
            local,
            planned.len(),
            diff_summary(&summary_diffs),
        ));
        if args.diff || args.format.is_json()
        /* harmless */
        {
            for (lbl, old, new) in &diffs {
                let block = unified_diff(old, new, lbl, &format!("{lbl} (proposed)"));
                if !block.is_empty() {
                    r.data_line(block);
                }
            }
        } else {
            for (s, p) in &planned {
                r.data_line(format!(
                    "{}{}:{p}",
                    s.ns.namespace,
                    s.service().map(|x| format!("/{x}")).unwrap_or_default()
                ));
            }
        }
        r.next("Re-run with --apply to execute");
        r.next("Use --diff for a per-target preview");
        r.print();
        return Ok(ExitKind::Success);
    }

    if let ConfirmResult::Aborted(why) =
        gate.confirm(Confirm::LargeFanout, planned.len(), "Continue?")
    {
        eprintln!("aborted: {why}");
        return Ok(ExitKind::Error);
    }

    let snaps = SnapshotStore::open()?;
    let store = AuditStore::open()?;
    let mut ok = 0usize;
    let mut bad = 0usize;
    let mut renderer = Renderer::new();
    let new_hash = sha256_hex(&body);
    let b64 = base64::engine::general_purpose::STANDARD.encode(&body);

    for (s, path) in &planned {
        let label = format!(
            "{}{}:{path}",
            s.ns.namespace,
            s.service().map(|x| format!("/{x}")).unwrap_or_default()
        );

        // 1. Snapshot existing remote content (if any).
        let prev_text = read_remote(&*runner, s, path).unwrap_or_default();
        let prev_hash = if prev_text.is_empty() {
            None
        } else {
            Some(snaps.put(prev_text.as_bytes())?)
        };

        // 2. Atomic push: write to <path>.tmp.<rand>, preserve
        //    mode/uid/gid of the original (audit §4.2), then rename.
        let tmp = format!("{path}.inspect.{}.tmp", &new_hash[..8]);
        let inner = super::atomic::write_then_rename(&b64, &tmp, path);
        let cmd = match s.container() {
            Some(container) => format!(
                "docker exec {} sh -c {}",
                shquote(container),
                shquote(&inner)
            ),
            None => format!("sh -c {}", shquote(&inner)),
        };

        let started = Instant::now();
        let out = runner.run(
            &s.ns.namespace,
            &s.ns.target,
            &cmd,
            RunOpts::with_timeout(60),
        )?;
        let dur = started.elapsed().as_millis() as u64;

        let mut entry = AuditEntry::new("cp", &label);
        entry.args = local.clone();
        entry.previous_hash = prev_hash.clone().map(|h| format!("sha256:{h}"));
        entry.new_hash = Some(format!("sha256:{new_hash}"));
        entry.snapshot = prev_hash.clone().map(|h| snaps.path_for(&h).display().to_string());
        entry.diff_summary = diff_summary(&[(prev_text, new_text.clone())]);
        entry.exit = out.exit_code;
        entry.duration_ms = dur;
        entry.reason = crate::safety::validate_reason(args.reason.as_deref())?;
        // F11 (v0.1.3): pre-stage the inverse. cp restores the prior
        // file from the snapshot store; first-write has no prior, so
        // mark the entry unsupported and let `inspect rm --apply` be
        // the explicit follow-up if the operator wants to undo.
        entry.revert = Some(match prev_hash.as_ref() {
            Some(h) => Revert::state_snapshot(
                format!("sha256:{h}"),
                format!("restore {label} from snapshot sha256:{}", &h[..12]),
            ),
            None => Revert::unsupported(format!(
                "cp created a new file at {label}; revert by `inspect rm --apply {label}`"
            )),
        });
        entry.applied = Some(out.ok());
        store.append(&entry)?;

        if out.ok() {
            ok += 1;
            renderer.data_line(format!("{label}: pushed (audit {})", entry.id));
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
        .summary(format!("cp push: {ok} ok, {bad} failed"))
        .next("inspect audit ls")
        .next("inspect revert <audit-id> to undo");
    renderer.print();
    Ok(if bad == 0 {
        ExitKind::Success
    } else {
        ExitKind::Error
    })
}

// ---------------------------------------------------------------------------
// PULL
// ---------------------------------------------------------------------------
fn pull(_args: CpArgs, remote_sel: String, local: String) -> Result<ExitKind> {
    let (runner, nses, targets) = plan(&remote_sel)?;
    let steps: Vec<_> = iter_steps(&nses, &targets).collect();
    if steps.is_empty() {
        crate::error::emit("'{remote_sel}' matched no targets");
        return Ok(ExitKind::Error);
    }
    if steps.len() > 1 {
        eprintln!(
            "error: cp pull requires exactly one source target; got {}",
            steps.len()
        );
        return Ok(ExitKind::Error);
    }
    let s = &steps[0];
    let Some(path) = s.path.clone() else {
        crate::error::emit("cp pull requires a `:path` on the source selector");
        return Ok(ExitKind::Error);
    };

    let inner = format!("base64 -- {}", shquote(&path));
    let cmd = match s.container() {
        Some(container) => format!(
            "docker exec {} sh -c {}",
            shquote(container),
            shquote(&inner)
        ),
        None => inner,
    };
    let out = runner.run(
        &s.ns.namespace,
        &s.ns.target,
        &cmd,
        RunOpts::with_timeout(60),
    )?;
    if !out.ok() {
        eprintln!(
            "error: pulling '{}' failed (exit {}): {}",
            path,
            out.exit_code,
            out.stderr.trim()
        );
        return Ok(ExitKind::Error);
    }
    let cleaned: String = out.stdout.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(cleaned.as_bytes())
        .map_err(|e| anyhow::anyhow!("decoding remote payload: {e}"))?;

    if local == "-" {
        use std::io::Write;
        std::io::stdout().write_all(&bytes)?;
    } else {
        std::fs::write(&local, &bytes)?;
    }

    let mut r = Renderer::new();
    r.summary(format!("pulled {} bytes → {local}", bytes.len()));
    r.next("inspect cp <local> <sel>:<path> --apply  to push back");
    r.print();
    Ok(ExitKind::Success)
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------
fn read_remote(
    runner: &dyn crate::verbs::runtime::RemoteRunner,
    s: &crate::verbs::dispatch::Step<'_>,
    path: &str,
) -> Option<String> {
    let inner = format!("cat -- {}", shquote(path));
    let cmd = match s.container() {
        Some(container) => format!(
            "docker exec {} sh -c {}",
            shquote(container),
            shquote(&inner)
        ),
        None => inner,
    };
    let out = runner
        .run(
            &s.ns.namespace,
            &s.ns.target,
            &cmd,
            RunOpts::with_timeout(20),
        )
        .ok()?;
    if out.ok() {
        Some(out.stdout)
    } else {
        None
    }
}
