//! `inspect run <sel> -- <cmd>` (P6, v0.1.1).
//!
//! Read-only execution counterpart to [`crate::verbs::write::exec`]. Streams
//! the remote command's output line-by-line, never touches the audit log,
//! and has no `--apply`/confirmation gating. Use when you want to inspect
//! state with an ad-hoc shell snippet (`ps`, `cat /proc/...`, `df -h`,
//! `redis-cli info`, ...) without paying for the write-verb interlock.
//!
//! Field-pitfall driver: P6 in [INSPECT_v0.1.1_PATCH_SPEC.md]. Operators
//! routinely typed `inspect exec ... -- <read-only thing>` and ran into
//! the exec safety prompts on every iteration.

use anyhow::Result;

use crate::cli::RunArgs;
use crate::error::ExitKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::{Envelope, JsonOut, Renderer};
use crate::verbs::quote::shquote;

/// F10.7 (v0.1.3): strip ANSI CSI / OSC escape sequences from a
/// rendered output line. Conservative: matches `ESC [ ... <final>`
/// (CSI) and `ESC ] ... BEL` / `ESC ] ... ESC \` (OSC). Anything
/// else (single-byte controls already stripped by `safe_terminal_line`)
/// passes through. Allocation-free fast-path when no ESC byte is
/// present.
fn strip_ansi(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains('\u{001b}') {
        return std::borrow::Cow::Borrowed(s);
    }
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() {
            let nxt = bytes[i + 1];
            if nxt == b'[' {
                // CSI: ESC [ <params> <final 0x40-0x7E>
                let mut j = i + 2;
                while j < bytes.len() {
                    let c = bytes[j];
                    if (0x40..=0x7e).contains(&c) {
                        j += 1;
                        break;
                    }
                    j += 1;
                }
                i = j;
                continue;
            }
            if nxt == b']' {
                // OSC: ESC ] ... ( BEL | ESC \ )
                let mut j = i + 2;
                while j < bytes.len() {
                    if bytes[j] == 0x07 {
                        j += 1;
                        break;
                    }
                    if bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                        j += 2;
                        break;
                    }
                    j += 1;
                }
                i = j;
                continue;
            }
            // Two-char escape (ESC <char>): drop both.
            i += 2;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    // Bytes were a valid UTF-8 string and we only removed ASCII
    // ranges, so the result is still valid UTF-8.
    std::borrow::Cow::Owned(String::from_utf8(out).unwrap_or_default())
}

/// F9 (v0.1.3): default cap on forwarded stdin per `inspect run` invocation.
/// Above this the verb refuses, with a chained hint pointing at `inspect cp`
/// for bulk transfer (faster, resumable, audit-tracked separately).
pub const DEFAULT_STDIN_MAX: u64 = 10 * 1024 * 1024;

/// F9 (v0.1.3): parse a size string like `10m`, `512k`, `1g`, or a raw
/// byte count. Case-insensitive. `0` means "no cap".
pub fn parse_stdin_max(s: &str) -> Result<u64> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(anyhow::anyhow!("--stdin-max requires a value"));
    }
    let (num_part, mult) = match trimmed.chars().last() {
        Some('k') | Some('K') => (&trimmed[..trimmed.len() - 1], 1024u64),
        Some('m') | Some('M') => (&trimmed[..trimmed.len() - 1], 1024u64 * 1024),
        Some('g') | Some('G') => (&trimmed[..trimmed.len() - 1], 1024u64 * 1024 * 1024),
        _ => (trimmed, 1u64),
    };
    let n: u64 = num_part
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("--stdin-max: '{s}' is not a valid size"))?;
    n.checked_mul(mult)
        .ok_or_else(|| anyhow::anyhow!("--stdin-max: '{s}' overflows u64"))
}

/// F9 (v0.1.3): is local stdin a tty? When `true`, `inspect run` does not
/// forward stdin (matches `ssh -T host cmd <terminal>` semantics) — we
/// never hang waiting for the operator to type input that was never piped.
fn local_stdin_is_tty() -> bool {
    #[cfg(unix)]
    {
        // Safety: STDIN_FILENO (0) is always a valid FD on a hosted
        // process; isatty has no side effects.
        unsafe { libc::isatty(0) == 1 }
    }
    #[cfg(not(unix))]
    {
        // Conservative default off-unix: assume tty so we never read
        // (and potentially block on) stdin on platforms where the tty
        // detection isn't wired.
        true
    }
}

/// F9 (v0.1.3): read local stdin into a `Vec<u8>`, refusing if the
/// payload exceeds `cap_bytes`. `cap_bytes == 0` means "no cap".
fn read_stdin_capped(cap_bytes: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let stdin = std::io::stdin();
    let mut handle = stdin.lock();
    if cap_bytes == 0 {
        let mut buf = Vec::new();
        handle.read_to_end(&mut buf)?;
        return Ok(buf);
    }
    // Read one byte beyond the cap so we can detect overflow without
    // a separate syscall.
    let mut buf = Vec::new();
    let mut limited = handle.take(cap_bytes + 1);
    limited.read_to_end(&mut buf)?;
    if buf.len() as u64 > cap_bytes {
        return Err(anyhow::anyhow!(
            "stdin exceeds {}-byte cap — pass --stdin-max <SIZE> to override, \
             or use 'inspect cp' for large payloads (faster, resumable, \
             audit-tracked separately)",
            cap_bytes
        ));
    }
    Ok(buf)
}

pub fn run(args: RunArgs) -> Result<ExitKind> {
    if args.cmd.is_empty() {
        crate::error::emit("run requires a command after `--`");
        return Ok(ExitKind::Error);
    }
    let user_cmd = args.cmd.join(" ");

    // Reason is informational for `run` (not audited). Validate length so the
    // operator gets a useful error before we dial out to remote hosts.
    let reason = crate::safety::validate_reason(args.reason.as_deref())?;
    if let Some(r) = &reason {
        eprintln!("# reason: {r}");
    }

    let fmt = args.format.resolve()?;
    let json = matches!(fmt, crate::format::OutputFormat::Json);

    // ---------------------------------------------------------------
    // F9 (v0.1.3): stdin handling. Decide what (if anything) to
    // forward to the remote command's stdin BEFORE we resolve targets
    // or dial out — both --no-stdin's loud-failure and the size-cap
    // exit must fire pre-dispatch, with zero remote commands issued.
    // ---------------------------------------------------------------
    let cap_bytes = match args.stdin_max.as_deref() {
        Some(s) => match parse_stdin_max(s) {
            Ok(n) => n,
            Err(e) => {
                crate::error::emit(format!("{e}"));
                return Ok(ExitKind::Error);
            }
        },
        None => DEFAULT_STDIN_MAX,
    };
    let stdin_payload: Option<Vec<u8>> = if local_stdin_is_tty() {
        // Tty: never forward, never read — match v0.1.2 behavior so a
        // bare `inspect run arte 'cat'` from a terminal does not hang.
        None
    } else {
        let buf = match read_stdin_capped(cap_bytes) {
            Ok(b) => b,
            Err(e) => {
                crate::error::emit(format!("{e}"));
                return Ok(ExitKind::Error);
            }
        };
        if buf.is_empty() {
            // Non-tty but empty (`true | inspect run …` or `< /dev/null`):
            // honour `--no-stdin` silently, no audit. Same shape as a
            // tty invocation with no input.
            None
        } else if args.no_stdin {
            // The contract that prevents the field user's exact failure
            // mode: never silently discard input. Surface the loud
            // error before any remote command is dispatched.
            crate::error::emit(format!(
                "stdin has {} byte(s) but forwarding is disabled (--no-stdin). \
                 → drop the redirect, or omit --no-stdin to forward, or use \
                 'inspect cp' + remote-side redirect (see 'inspect help run')",
                buf.len()
            ));
            return Ok(ExitKind::Error);
        } else {
            Some(buf)
        }
    };
    let stdin_bytes_len: u64 = stdin_payload.as_ref().map(|b| b.len() as u64).unwrap_or(0);
    let stdin_sha256: Option<String> = if args.audit_stdin_hash {
        stdin_payload.as_ref().map(|b| {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(b);
            hex::encode(h.finalize())
        })
    } else {
        None
    };

    let (runner, nses, targets) = plan(&args.selector)?;
    let steps: Vec<_> = iter_steps(&nses, &targets).collect();
    if steps.is_empty() {
        crate::error::emit(format!("'{}' matched no targets", args.selector));
        return Ok(ExitKind::Error);
    }

    let timeout_secs = args.timeout_secs.unwrap_or(120);
    let mut ok = 0usize;
    let mut bad = 0usize;
    let mut last_inner: Option<i32> = None;
    let mut all_same = true;
    let masker = crate::redact::EnvSecretMasker::new(args.show_secrets, args.redact_all);
    // F9 (v0.1.3): when stdin is being forwarded, every step's run is
    // audited (verb=`run`, with stdin_bytes / optional stdin_sha256)
    // so a post-hoc audit can answer "what input did this command
    // consume?" by size. Without forwarded stdin, `inspect run`
    // remains un-audited (matches v0.1.2 read-verb behavior).
    let audit_store = if stdin_payload.is_some() {
        match crate::safety::audit::AuditStore::open() {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("warning: audit log unavailable ({e}); proceeding");
                None
            }
        }
    } else {
        None
    };
    // B8 (v0.1.2): when --no-truncate is set, lift the per-line byte cap
    // entirely. Otherwise keep the existing 4 KiB default that protects
    // terminals from runaway 100KB+ JSON blobs.
    let line_budget = if args.no_truncate {
        usize::MAX
    } else {
        crate::format::safe::DEFAULT_MAX_LINE_BYTES
    };
    let mut truncated_lines = 0usize;

    for s in &steps {
        if crate::exec::cancel::is_cancelled() {
            break;
        }
        let svc_label = s.service().map(|x| format!("/{x}")).unwrap_or_default();
        let label = format!("{}{}", s.ns.namespace, svc_label);

        // Wrap in `docker exec` when the selector points at a container.
        // Apply server-side line filter (--filter-line-pattern) by piping
        // through `grep -E`, mirroring the same pushdown logs/grep use.
        let inner = match s.container() {
            Some(container) => format!(
                "docker exec {} sh -c {}",
                shquote(container),
                shquote(&user_cmd)
            ),
            None => user_cmd.clone(),
        };
        let cmd = match &args.filter_line_pattern {
            Some(pat) => format!("{inner} | grep -E {}", shquote(pat)),
            None => inner,
        };
        // F10.7 (v0.1.3): `--clean-output` prepends `TERM=dumb` so any
        // remote tool that consults $TERM (less, ls --color=auto,
        // progress bars) downgrades to plain text. ANSI stripping
        // happens client-side post-mask as a belt-and-braces second
        // line of defense.
        let cmd = if args.clean_output {
            format!("TERM=dumb {cmd}")
        } else {
            cmd
        };

        let opts = RunOpts::with_timeout(timeout_secs);
        let opts = match &stdin_payload {
            Some(bytes) => opts.with_stdin(bytes.clone()),
            None => opts,
        };
        let svc_name = s.service().unwrap_or("_").to_string();
        let ns_name = s.ns.namespace.clone();
        let step_started = std::time::Instant::now();

        let exit = runner.run_streaming(&ns_name, &s.ns.target, &cmd, opts, &mut |line| {
            let masked = masker.mask_line(line);
            let masked: std::borrow::Cow<'_, str> = if args.clean_output {
                match strip_ansi(&masked) {
                    std::borrow::Cow::Borrowed(_) => std::borrow::Cow::Owned(masked.into_owned()),
                    std::borrow::Cow::Owned(s) => std::borrow::Cow::Owned(s),
                }
            } else {
                masked
            };
            if json {
                JsonOut::write(
                    &Envelope::new(&ns_name, "run", "run")
                        .with_service(&svc_name)
                        .put(
                            "line",
                            crate::format::safe::safe_machine_line(&masked).as_ref(),
                        ),
                );
            } else {
                let safe = crate::format::safe::safe_terminal_line(&masked, line_budget);
                if line_budget != usize::MAX && masked.len() > line_budget {
                    truncated_lines += 1;
                }
                println!("{label} | {safe}");
            }
        });

        match exit {
            Ok(code) => {
                if code == 0 {
                    ok += 1;
                } else {
                    bad += 1;
                    if !json {
                        eprintln!("{label}: exit {code}");
                    }
                }
                if let Some(prev) = last_inner {
                    if prev != code {
                        all_same = false;
                    }
                }
                last_inner = Some(code);
                if let Some(store) = &audit_store {
                    let mut e = crate::safety::audit::AuditEntry::new("run", &label);
                    e.args = user_cmd.clone();
                    e.exit = code;
                    e.duration_ms = step_started.elapsed().as_millis() as u64;
                    e.reason = reason.clone();
                    e.stdin_bytes = stdin_bytes_len;
                    e.stdin_sha256 = stdin_sha256.clone();
                    let _ = store.append(&e);
                }
            }
            Err(e) => {
                bad += 1;
                all_same = false;
                if !json {
                    eprintln!("{label}: {e}");
                }
                if let Some(store) = &audit_store {
                    let mut entry = crate::safety::audit::AuditEntry::new("run", &label);
                    entry.args = user_cmd.clone();
                    entry.exit = -1;
                    entry.duration_ms = step_started.elapsed().as_millis() as u64;
                    entry.reason = reason.clone();
                    entry.stdin_bytes = stdin_bytes_len;
                    entry.stdin_sha256 = stdin_sha256.clone();
                    entry.diff_summary = format!("transport_error: {e}");
                    let _ = store.append(&entry);
                }
            }
        }
    }

    if !json {
        let mut r = Renderer::new();
        r.summary(format!("run: {ok} ok, {bad} failed"));
        r.print();
        // B8: surface a single, unmissable end-of-stream warning when any
        // line was truncated mid-content. Goes to stderr so it doesn't
        // interleave with the data captured by `> file` redirects.
        if truncated_lines > 0 {
            eprintln!(
                "── output truncated: {n} line{s} exceeded the {budget}-byte per-line cap (re-run with --no-truncate to see full content) ──",
                n = truncated_lines,
                s = if truncated_lines == 1 { "" } else { "s" },
                budget = line_budget,
            );
        }
    }

    // P11 (v0.1.1): surface the remote command's exit code on
    // single-target / uniform multi-target runs so shell idioms like
    // `inspect run arte/api -- 'exit 7'` behave the way they would
    // for a direct ssh.
    if let Some(inner_code) = last_inner {
        if all_same {
            return Ok(ExitKind::Inner(crate::error::clamp_inner_exit(inner_code)));
        }
    }
    Ok(if bad == 0 {
        ExitKind::Success
    } else {
        ExitKind::Error
    })
}
