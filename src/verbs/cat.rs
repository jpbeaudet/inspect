//! `inspect cat <sel>:<path>` — read a file.

use anyhow::{anyhow, Result};

use crate::cli::CatArgs;
use crate::error::ExitKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::{Envelope, JsonOut, Renderer};
use crate::verbs::quote::shquote;

/// F10.2 (v0.1.3): inclusive 1-based line slice. `None` means "no
/// slice — print the whole file" (today's behavior).
#[derive(Clone, Copy, Debug)]
struct LineSlice {
    start: usize,
    end: Option<usize>,
}

impl LineSlice {
    fn contains(&self, n: usize) -> bool {
        n >= self.start && self.end.map_or(true, |e| n <= e)
    }
}

/// Parse `--lines L-R` (inclusive). Accepts `5-10`, `5-`, or just
/// `5` (single line). Returns an error so clap surface a clear
/// message rather than a panic.
fn parse_lines_spec(s: &str) -> Result<LineSlice> {
    let s = s.trim();
    if let Some((l, r)) = s.split_once('-') {
        let start: usize = l
            .trim()
            .parse()
            .map_err(|_| anyhow!("invalid --lines spec '{s}': start must be a positive integer"))?;
        if start == 0 {
            return Err(anyhow!("invalid --lines spec '{s}': lines are 1-based"));
        }
        let end = if r.trim().is_empty() {
            None
        } else {
            let e: usize = r.trim().parse().map_err(|_| {
                anyhow!("invalid --lines spec '{s}': end must be a positive integer")
            })?;
            if e < start {
                return Err(anyhow!(
                    "invalid --lines spec '{s}': end ({e}) must be >= start ({start})"
                ));
            }
            Some(e)
        };
        Ok(LineSlice { start, end })
    } else {
        let n: usize = s
            .parse()
            .map_err(|_| anyhow!("invalid --lines spec '{s}': expected 'L-R' or single integer"))?;
        if n == 0 {
            return Err(anyhow!("invalid --lines spec '{s}': lines are 1-based"));
        }
        Ok(LineSlice {
            start: n,
            end: Some(n),
        })
    }
}

fn resolve_slice(args: &CatArgs) -> Result<Option<LineSlice>> {
    if let Some(spec) = args.lines.as_deref() {
        return Ok(Some(parse_lines_spec(spec)?));
    }
    if args.start.is_some() || args.end.is_some() {
        let start = args.start.unwrap_or(1);
        if start == 0 {
            return Err(anyhow!("invalid --start 0: lines are 1-based"));
        }
        if let Some(e) = args.end {
            if e < start {
                return Err(anyhow!(
                    "invalid range: --end ({e}) must be >= --start ({start})"
                ));
            }
        }
        return Ok(Some(LineSlice {
            start,
            end: args.end,
        }));
    }
    Ok(None)
}

pub fn run(args: CatArgs) -> Result<ExitKind> {
    let (runner, nses, targets) = plan(&args.target)?;
    let slice = resolve_slice(&args)?;

    let mut printed_any = false;
    let mut errored_any = false;

    for step in iter_steps(&nses, &targets) {
        // L7 (v0.1.3): per-target redactor. `inspect cat` is the
        // single highest-risk verb for accidentally printing a PEM
        // private key (it will literally cat any file the operator
        // points at), which is why the default behavior collapses
        // private-key blocks to `[REDACTED PEM KEY]` before they
        // reach stdout. `--show-secrets` opts out for vetted files.
        let redactor = crate::redact::OutputRedactor::new(args.show_secrets, false);
        let Some(path) = step.path.as_deref() else {
            crate::tee_eprintln!(
                "warning: '{}' has no :path; cat requires a file path (e.g. arte/atlas:/etc/atlas.conf)",
                step.ns.namespace
            );
            errored_any = true;
            continue;
        };
        // F5 dual-axis (v0.1.3): docker exec must receive the
        // container_name (e.g. `luminary-api`), not the canonical
        // service name (`api`). See `Step::container()` doc.
        let cmd = build_cat(step.container(), path);
        let out = runner.run(
            &step.ns.namespace,
            &step.ns.target,
            &cmd,
            RunOpts::with_timeout(30),
        )?;
        if !out.ok() {
            errored_any = true;
            if args.format.is_json() {
                JsonOut::write(
                    &Envelope::new(&step.ns.namespace, "file", format!("file:{path}"))
                        .with_service(step.service().unwrap_or("_"))
                        .put("path", path)
                        .put("error", out.stderr.trim())
                        .put("exit", out.exit_code),
                );
            } else {
                crate::tee_eprintln!(
                    "{}: cat failed (exit {}): {}",
                    step.ns.namespace,
                    out.exit_code,
                    out.stderr.trim()
                );
            }
            continue;
        }
        printed_any = true;
        // L7 (v0.1.3): always iterate lines (even on the
        // previously-fast `print!("{}", out.stdout)` path) so the PEM
        // masker can collapse multi-line key blocks into a single
        // `[REDACTED PEM KEY]` marker. The trailing-newline behavior
        // is preserved: every emitted line gets `\n` (matching the
        // pre-L7 "print verbatim, append newline if missing" rule).
        if args.format.is_json() {
            for (idx, line) in out.stdout.lines().enumerate() {
                let n = idx + 1;
                if let Some(sl) = slice {
                    if !sl.contains(n) {
                        // PEM block detection still needs to see the
                        // line so its in-block state stays accurate;
                        // but the slice filter says "not in range",
                        // so we only feed the masker, never emit.
                        let _ = redactor.mask_line(line);
                        continue;
                    }
                }
                let masked = match redactor.mask_line(line) {
                    Some(m) => m,
                    None => continue,
                };
                JsonOut::write(
                    &Envelope::new(&step.ns.namespace, "file", format!("file:{path}"))
                        .with_service(step.service().unwrap_or("_"))
                        .put("path", path)
                        .put("n", n as u64)
                        .put("line", masked.as_ref()),
                );
            }
        } else if let Some(sl) = slice {
            for (idx, line) in out.stdout.lines().enumerate() {
                let n = idx + 1;
                if !sl.contains(n) {
                    let _ = redactor.mask_line(line);
                    continue;
                }
                let masked = match redactor.mask_line(line) {
                    Some(m) => m,
                    None => continue,
                };
                crate::tee_println!("{masked}");
            }
        } else {
            for line in out.stdout.lines() {
                let masked = match redactor.mask_line(line) {
                    Some(m) => m,
                    None => continue,
                };
                crate::tee_println!("{masked}");
            }
        }
    }

    if args.format.is_json() {
        return Ok(if printed_any {
            ExitKind::Success
        } else {
            ExitKind::Error
        });
    }
    if !printed_any && errored_any {
        Renderer::new()
            .summary("cat failed on every target")
            .next("inspect ls <sel>:<dir> to find the right path")
            .print();
        return Ok(ExitKind::Error);
    }
    Ok(ExitKind::Success)
}

fn build_cat(service: Option<&str>, path: &str) -> String {
    match service {
        Some(svc) => format!("docker exec {} cat -- {}", shquote(svc), shquote(path)),
        None => format!("cat -- {}", shquote(path)),
    }
}
