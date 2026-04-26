//! `inspect edit <sel>:<path> '<sed-expr>'` (bible §8.3).
//!
//! Strategy:
//! 1. Pull current content via `cat`.
//! 2. Apply the `sed` expression *locally* (using GNU sed if present,
//!    otherwise the BSD-compatible `sed -E` fallback) to compute the
//!    proposed new content.
//! 3. Render a unified diff for dry-run.
//! 4. On `--apply`: snapshot the original, push the new content via the
//!    same atomic temp-rename used by `cp`, audit-log everything.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{anyhow, Result};
use base64::Engine as _;

use crate::cli::EditArgs;
use crate::error::ExitKind;
use crate::safety::{
    diff::{diff_summary, unified_diff},
    snapshot::sha256_hex,
    AuditEntry, AuditStore, Confirm, SafetyGate, SnapshotStore,
};
use crate::safety::gate::ConfirmResult;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::Renderer;
use crate::verbs::quote::shquote;

pub fn run(args: EditArgs) -> Result<ExitKind> {
    if !looks_like_sed_expr(&args.expr) {
        eprintln!(
            "error: expression '{}' does not look like a sed substitution. \
             Expected `s/old/new/[flags]`.",
            args.expr
        );
        return Ok(ExitKind::Error);
    }

    let (runner, nses, targets) = plan(&args.target)?;
    let mut planned = Vec::new();
    for s in iter_steps(&nses, &targets) {
        let Some(p) = s.path.clone() else {
            eprintln!("error: edit requires a :path on selector");
            return Ok(ExitKind::Error);
        };
        planned.push((s, p));
    }
    if planned.is_empty() {
        eprintln!("error: '{}' matched no targets", args.target);
        return Ok(ExitKind::Error);
    }

    // Pull + apply locally for every target so we can render diffs and
    // detect no-op edits up-front.
    let mut work: Vec<EditWork> = Vec::with_capacity(planned.len());
    for (s, path) in planned {
        let label = format!(
            "{}{}:{path}",
            s.ns.namespace,
            s.service().map(|x| format!("/{x}")).unwrap_or_default()
        );
        let original = read_remote(&*runner, &s, &path).ok_or_else(|| {
            anyhow!("could not read remote '{label}' (file missing or unreadable)")
        })?;
        let new_text = apply_sed_local(&args.expr, &original)?;
        work.push(EditWork {
            label,
            ns: s.ns.namespace.clone(),
            target: s.ns.target.clone(),
            service: s.service().map(str::to_string),
            path,
            original,
            new_text,
        });
    }

    let gate = SafetyGate::new(args.apply, args.yes, args.yes_all);

    if !gate.should_apply() {
        let mut r = Renderer::new();
        let summary = diff_summary(
            &work
                .iter()
                .map(|w| (w.original.clone(), w.new_text.clone()))
                .collect::<Vec<_>>(),
        );
        r.summary(format!(
            "DRY RUN. Would edit {} file(s) [{summary}]",
            work.len()
        ));
        for w in &work {
            let block = unified_diff(
                &w.original,
                &w.new_text,
                &w.label,
                &format!("{} (proposed)", w.label),
            );
            if block.is_empty() {
                r.data_line(format!("{}: no change", w.label));
            } else {
                r.data_line(block);
            }
        }
        r.next("Re-run with --apply to execute");
        r.print();
        return Ok(ExitKind::Success);
    }

    if let ConfirmResult::Aborted(why) =
        gate.confirm(Confirm::LargeFanout, work.len(), "Continue?")
    {
        eprintln!("aborted: {why}");
        return Ok(ExitKind::Error);
    }

    let snaps = SnapshotStore::open()?;
    let store = AuditStore::open()?;
    let mut ok = 0usize;
    let mut bad = 0usize;
    let mut renderer = Renderer::new();

    for w in &work {
        if w.original == w.new_text {
            renderer.data_line(format!("{}: no change", w.label));
            ok += 1;
            continue;
        }

        let prev_hash = snaps.put(w.original.as_bytes())?;
        let new_hash = sha256_hex(w.new_text.as_bytes());
        let b64 = base64::engine::general_purpose::STANDARD.encode(w.new_text.as_bytes());

        let tmp = format!("{}.inspect.{}.tmp", w.path, &new_hash[..8]);
        let inner = format!(
            "set -e; printf %s {b64_q} | base64 -d > {tmp_q} && mv {tmp_q} {path_q}",
            b64_q = shquote(&b64),
            tmp_q = shquote(&tmp),
            path_q = shquote(&w.path),
        );
        let cmd = match w.service.as_deref() {
            Some(svc) => format!("docker exec {} sh -c {}", shquote(svc), shquote(&inner)),
            None => format!("sh -c {}", shquote(&inner)),
        };
        let started = Instant::now();
        let out = runner.run(&w.ns, &w.target, &cmd, RunOpts::with_timeout(60))?;
        let dur = started.elapsed().as_millis() as u64;

        let mut entry = AuditEntry::new("edit", &w.label);
        entry.args = args.expr.clone();
        entry.previous_hash = Some(format!("sha256:{prev_hash}"));
        entry.new_hash = Some(format!("sha256:{new_hash}"));
        entry.snapshot = Some(snaps.path_for(&prev_hash).display().to_string());
        entry.diff_summary = diff_summary(&[(w.original.clone(), w.new_text.clone())]);
        entry.exit = out.exit_code;
        entry.duration_ms = dur;
        store.append(&entry)?;

        if out.ok() {
            ok += 1;
            renderer.data_line(format!("{}: edited (audit {})", w.label, entry.id));
        } else {
            bad += 1;
            renderer.data_line(format!(
                "{}: FAILED (exit {}): {}",
                w.label,
                out.exit_code,
                out.stderr.trim()
            ));
        }
    }
    renderer
        .summary(format!("edit: {ok} ok, {bad} failed"))
        .next("inspect audit ls")
        .next("inspect revert <audit-id> to undo");
    renderer.print();
    Ok(if bad == 0 { ExitKind::Success } else { ExitKind::Error })
}

struct EditWork {
    label: String,
    ns: String,
    target: crate::ssh::options::SshTarget,
    service: Option<String>,
    path: String,
    original: String,
    new_text: String,
}

fn looks_like_sed_expr(s: &str) -> bool {
    let s = s.trim();
    // We accept: `s<delim>...<delim>...<delim>[flags]`. Common delims: / | : , #
    let bytes = s.as_bytes();
    if bytes.len() < 4 || bytes[0] != b's' {
        return false;
    }
    let delim = bytes[1];
    if !matches!(delim, b'/' | b'|' | b':' | b',' | b'#' | b'@') {
        return false;
    }
    // Count unescaped delimiters; need at least 3.
    let mut count = 0usize;
    let mut i = 1;
    while i < bytes.len() {
        if bytes[i] == delim && (i == 1 || bytes[i - 1] != b'\\') {
            count += 1;
        }
        i += 1;
    }
    count >= 3
}

fn apply_sed_local(expr: &str, input: &str) -> Result<String> {
    // Use the local `sed` binary so the expression syntax matches what
    // would have been applied on the remote. Keep it deterministic with
    // `LC_ALL=C`.
    let mut child = Command::new("sed")
        .arg("-E")
        .arg(expr)
        .env("LC_ALL", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("local `sed` failed to start: {e}"))?;
    if let Some(mut s) = child.stdin.take() {
        s.write_all(input.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(anyhow!(
            "local `sed` exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn read_remote(
    runner: &dyn crate::verbs::runtime::RemoteRunner,
    s: &crate::verbs::dispatch::Step<'_>,
    path: &str,
) -> Option<String> {
    let inner = format!("cat -- {}", shquote(path));
    let cmd = match s.service() {
        Some(svc) => format!("docker exec {} sh -c {}", shquote(svc), shquote(&inner)),
        None => inner,
    };
    let out = runner
        .run(&s.ns.namespace, &s.ns.target, &cmd, RunOpts::with_timeout(20))
        .ok()?;
    if out.ok() {
        Some(out.stdout)
    } else {
        None
    }
}
