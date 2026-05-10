//! `inspect find <sel>:<path> [pat]` — find files matching a pattern.

use anyhow::Result;

use crate::cli::FindArgs;
use crate::error::ExitKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::{Envelope, JsonOut};
use crate::verbs::quote::shquote;

pub fn run(args: FindArgs) -> Result<ExitKind> {
    // Activate the FormatArgs mutex check
    // (e.g. `--select` without `--json` → exit 2).
    args.format.resolve()?;
    let (runner, nses, targets) = plan(&args.target)?;

    // Construct the streaming `--select` filter ONCE at
    // function entry so a parse error fails fast before any frame is
    // emitted.
    let mut select = args.format.select_filter()?;

    let mut total_hits = 0usize;
    for step in iter_steps(&nses, &targets) {
        // Per-step redactor for symmetry with the other
        // read verbs. `find` emits file paths only — secret patterns
        // rarely fire — but a path like
        // `/srv/dump/postgres://u:p@db/secret.sql` would otherwise
        // leak the embedded URL credential. Cheap; runs unconditionally.
        let redactor = crate::redact::OutputRedactor::new(args.show_secrets, false);
        let path = step.path.as_deref().unwrap_or(".");
        // Field pitfall §7.4: defensive caps against symlink loops and
        // pathological deep trees. `-P` (the default for GNU find but
        // we make it explicit) means "never follow symlinks", which
        // is the only thing that can introduce cycles. `-maxdepth`
        // bounds traversal so a misconfigured mountpoint can't hang
        // the call. Both can be loosened by the operator via
        // `INSPECT_FIND_MAXDEPTH` if they really need a deeper scan.
        let maxdepth: u32 = std::env::var("INSPECT_FIND_MAXDEPTH")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20);
        let mut find_cmd = format!("find -P {} -maxdepth {maxdepth} -type f", shquote(path));
        if let Some(pat) = args.pattern.as_deref() {
            find_cmd.push(' ');
            find_cmd.push_str("-name ");
            find_cmd.push_str(&shquote(pat));
        }
        // Docker exec must receive the
        // container_name, not the canonical service name. See
        // `Step::container()` doc; same fix shipped for cat/ls/grep.
        let cmd = match step.container() {
            Some(svc) => format!("docker exec {} sh -c {}", shquote(svc), shquote(&find_cmd)),
            None => find_cmd,
        };
        let out = runner.run(
            &step.ns.namespace,
            &step.ns.target,
            &cmd,
            RunOpts::with_timeout(60),
        )?;
        if !out.ok() && out.stdout.is_empty() {
            // find can return non-zero on permission denied while still
            // listing matches; only treat true failures (no stdout) as errors.
            if !args.format.is_json() {
                crate::tee_eprintln!(
                    "{}: find failed (exit {}): {}",
                    step.ns.namespace,
                    out.exit_code,
                    out.stderr.trim()
                );
            }
            continue;
        }
        for line in out.stdout.lines().filter(|l| !l.is_empty()) {
            let masked = match redactor.mask_line(line) {
                Some(m) => m,
                None => continue,
            };
            total_hits += 1;
            if args.format.is_json() {
                JsonOut::write(
                    &Envelope::new(&step.ns.namespace, "dir", format!("dir:{path}"))
                        .with_service(step.service().unwrap_or("_"))
                        .put("path", masked.as_ref()),
                    select.as_mut(),
                )?;
            } else {
                crate::tee_println!(
                    "{}{}: {masked}",
                    step.ns.namespace,
                    step.service().map(|s| format!("/{s}")).unwrap_or_default()
                );
            }
        }
    }
    if args.format.is_json() {
        crate::verbs::output::flush_filter(select.as_mut())?;
    }
    if total_hits == 0 {
        if !args.format.is_json() {
            crate::tee_eprintln!("(no matches)");
        }
        return Ok(ExitKind::NoMatches);
    }
    Ok(ExitKind::Success)
}
