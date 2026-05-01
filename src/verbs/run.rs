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
/// Above this the verb refuses, with a chained hint pointing at `inspect put`
/// (F15) for bulk file transfer (uncapped, audit-tracked, F11-revertible).
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
             or use 'inspect put <local> <ns>:<path>' (F15) for bulk file \
             transfer (uncapped, audit-tracked, F11-revertible)",
            cap_bytes
        ));
    }
    Ok(buf)
}

/// F14 (v0.1.3): a resolved script-mode payload. The body is shipped
/// to the remote via `bash -s` (or a different interpreter declared in
/// the script's shebang); `script_path` is `Some(path)` for `--file`
/// mode and `None` for `--stdin-script` mode.
pub(crate) struct ScriptSource {
    pub body: Vec<u8>,
    pub sha256: String,
    pub interp: String,
    pub script_path: Option<String>,
}

/// F14 (v0.1.3): interpreter dispatched on the remote when no shebang
/// is declared. `bash -s` is a strict superset of `sh -s` for the
/// cross-layer-quoting use case the operator's field feedback called
/// out; if a target lacks `bash`, the operator declares `#!/bin/sh`
/// in the script and F14 honors it.
const DEFAULT_SCRIPT_INTERP: &str = "bash";

/// F14 (v0.1.3): parse the first line of a script for `#!` and pick
/// the interpreter. Recognizes `#!/usr/bin/env <interp>` and
/// `#!/path/to/<interp>` forms. Falls back to [`DEFAULT_SCRIPT_INTERP`]
/// when no shebang is present or the line is malformed.
fn detect_interpreter(body: &[u8]) -> String {
    if !body.starts_with(b"#!") {
        return DEFAULT_SCRIPT_INTERP.to_string();
    }
    // First line only; cap at 256 bytes so a binary file with a
    // leading "#!" doesn't pull a multi-megabyte slice through here.
    let end = body
        .iter()
        .take(256)
        .position(|&b| b == b'\n')
        .unwrap_or_else(|| body.len().min(256));
    let line = std::str::from_utf8(&body[2..end]).unwrap_or("").trim();
    if line.is_empty() {
        return DEFAULT_SCRIPT_INTERP.to_string();
    }
    // Tokens after `#!`: [path, ...rest]. For `/usr/bin/env <interp>`
    // the second token is the interpreter; otherwise the basename of
    // the first token is.
    let mut toks = line.split_whitespace();
    let first = match toks.next() {
        Some(t) => t,
        None => return DEFAULT_SCRIPT_INTERP.to_string(),
    };
    let basename = first.rsplit('/').next().unwrap_or(first);
    if basename == "env" {
        if let Some(interp) = toks.next() {
            return sanitize_interp(interp);
        }
        return DEFAULT_SCRIPT_INTERP.to_string();
    }
    sanitize_interp(basename)
}

/// F14 (v0.1.3): allow only `[A-Za-z0-9_.-]` in interpreter names so
/// a hostile or malformed shebang cannot inject shell metacharacters
/// into the rendered remote command. Anything else falls back to
/// [`DEFAULT_SCRIPT_INTERP`].
fn sanitize_interp(s: &str) -> String {
    if s.is_empty()
        || !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return DEFAULT_SCRIPT_INTERP.to_string();
    }
    s.to_string()
}

/// F14 (v0.1.3): does this interpreter accept `-s` as the
/// "read-script-from-stdin" flag? Bash and `sh` (POSIX) do; everything
/// else (python, node, ruby, ...) takes a bare `-` instead.
fn interp_uses_dash_s(interp: &str) -> bool {
    matches!(interp, "bash" | "sh" | "zsh" | "ksh" | "dash")
}

/// F14 (v0.1.3): render the remote dispatch shape for a script-mode
/// invocation. For bash-family interpreters this is `bash -s -- a b c`
/// (positional args land in `$1` / `$2` / ...). For others (python,
/// node, ...) this is `python3 - a b c` (POSIX convention; args land
/// in `sys.argv[1:]` etc.).
fn render_script_invocation(interp: &str, args: &[String]) -> String {
    // Interpreter is a controlled identifier (parsed from the
    // shebang's basename or the F14 default `bash`); rendering it
    // unquoted keeps the audit `rendered_cmd` field readable and
    // matches the standard `bash -s` / `python3 -` shape operators
    // expect to see in dispatch logs.
    let stdin_marker = if interp_uses_dash_s(interp) {
        "-s"
    } else {
        "-"
    };
    if args.is_empty() {
        return format!("{interp} {stdin_marker}");
    }
    let quoted: Vec<String> = args.iter().map(|a| shquote(a)).collect();
    if interp_uses_dash_s(interp) {
        format!("{interp} -s -- {}", quoted.join(" "))
    } else {
        // POSIX `<interp> - arg1 arg2`: most non-bash REPL-style
        // interpreters accept `-` as "read from stdin" with positional
        // args after.
        format!("{interp} - {}", quoted.join(" "))
    }
}

/// F14 (v0.1.3): resolve the script-mode payload (or `None` if the
/// caller is in classic argv-cmd mode). Reads the file or stdin once,
/// hashes it, and detects the interpreter from the shebang. Honors
/// the same `--stdin-max` cap as F9 stdin forwarding.
pub(crate) fn resolve_script_source(
    args: &RunArgs,
    cap_bytes: u64,
) -> Result<Option<ScriptSource>> {
    if let Some(path) = args.file.as_deref() {
        let p = std::path::Path::new(path);
        let meta = std::fs::metadata(p)
            .map_err(|e| anyhow::anyhow!("--file '{}' is not readable: {e}", path))?;
        if meta.is_dir() {
            return Err(anyhow::anyhow!(
                "--file '{}' is a directory; expected a script file",
                path
            ));
        }
        if cap_bytes != 0 && meta.len() > cap_bytes {
            return Err(anyhow::anyhow!(
                "--file '{}' is {} bytes, above the {}-byte cap — pass \
                 --stdin-max <SIZE> to raise (or `0` to disable), or use \
                 'inspect put' (F15) to ship the script first, then \
                 `inspect run -- bash /remote/path`",
                path,
                meta.len(),
                cap_bytes
            ));
        }
        let body =
            std::fs::read(p).map_err(|e| anyhow::anyhow!("--file '{}': read failed: {e}", path))?;
        if body.is_empty() {
            return Err(anyhow::anyhow!("--file '{}' is empty", path));
        }
        let sha = {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(&body);
            hex::encode(h.finalize())
        };
        let interp = detect_interpreter(&body);
        let abs = std::fs::canonicalize(p)
            .map(|x| x.display().to_string())
            .unwrap_or_else(|_| path.to_string());
        return Ok(Some(ScriptSource {
            body,
            sha256: sha,
            interp,
            script_path: Some(abs),
        }));
    }
    if args.stdin_script {
        if local_stdin_is_tty() {
            return Err(anyhow::anyhow!(
                "--stdin-script requires piped stdin (got a tty) — pass \
                 --file <path> for a script on disk, or pipe the script: \
                 `inspect run <ns> --stdin-script <<'BASH' ... BASH`"
            ));
        }
        let body = read_stdin_capped(cap_bytes)?;
        if body.is_empty() {
            return Err(anyhow::anyhow!(
                "--stdin-script: stdin is empty — pipe a script body, or \
                 pass --file <path>"
            ));
        }
        let sha = {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(&body);
            hex::encode(h.finalize())
        };
        let interp = detect_interpreter(&body);
        return Ok(Some(ScriptSource {
            body,
            sha256: sha,
            interp,
            script_path: None,
        }));
    }
    Ok(None)
}

/// F14 (v0.1.3): write the script body to the content-addressed store
/// at `~/.inspect/scripts/<sha256>.sh` (mode 0600 inside a 0700 dir),
/// idempotently. Errors are non-fatal — the audit entry still
/// references the body by hash even if the dedup write failed; the
/// operator can recover from the original `--file` path.
fn store_script_body(body: &[u8], sha: &str) -> Result<()> {
    let dir = crate::paths::scripts_dir();
    if !dir.exists() {
        std::fs::create_dir_all(&dir)?;
        let _ = crate::paths::set_dir_mode_0700(&dir);
    }
    let path = dir.join(format!("{sha}.sh"));
    if path.exists() {
        return Ok(());
    }
    // Write to a temp path first then rename so a concurrent reader
    // never sees a half-written script body.
    let tmp = dir.join(format!(".{sha}.tmp"));
    std::fs::write(&tmp, body)?;
    let _ = crate::paths::set_file_mode_0600(&tmp);
    std::fs::rename(&tmp, &path)?;
    let _ = crate::paths::set_file_mode_0600(&path);
    Ok(())
}

pub fn run(args: RunArgs) -> Result<ExitKind> {
    // F14 (v0.1.3): script mode (`--file` / `--stdin-script`) does not
    // require a command after `--`. Classic argv-cmd mode still does.
    let script_mode_requested = args.file.is_some() || args.stdin_script;
    if !script_mode_requested && args.cmd.is_empty() {
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
    // F9 / F14 (v0.1.3): stdin handling. Decide what (if anything) to
    // forward to the remote command's stdin BEFORE we resolve targets
    // or dial out — both --no-stdin's loud-failure and the size-cap
    // exit must fire pre-dispatch, with zero remote commands issued.
    //
    // F14 cases:
    //   * `--file <path>`    : script body is the local file; remote
    //                          command becomes `bash -s -- <args>`
    //                          and the body rides on the stdin pipe.
    //   * `--stdin-script`   : script body is local stdin (must be
    //                          non-tty, non-empty).
    //   * neither set        : F9 contract — pipe local stdin to the
    //                          remote command verbatim.
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
    // F14: resolve the script source first. If this fails (file
    // missing, stdin tty, payload above cap), exit 2 BEFORE dispatch
    // — same invariant as F9.
    let script: Option<ScriptSource> = match resolve_script_source(&args, cap_bytes) {
        Ok(s) => s,
        Err(e) => {
            crate::error::emit(format!("{e}"));
            return Ok(ExitKind::Error);
        }
    };
    let stdin_payload: Option<Vec<u8>> = if let Some(s) = &script {
        // F14: script body claims the remote stdin pipe. Local stdin
        // is NOT additionally forwarded — the spec is explicit that
        // operators wanting both pipe a script via `--file` and let
        // F9 forward stdin (a v0.1.5 follow-up).
        Some(s.body.clone())
    } else if local_stdin_is_tty() {
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
                 'inspect put <local> <ns>:<path>' (F15) to ship the file \
                 separately and reference it from the remote command",
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

    // F12 (v0.1.3): per-invocation env overrides. Validate once,
    // before the per-step loop, so a typo in `--env` short-circuits
    // the whole run instead of failing N times.
    let user_env: Vec<(String, String)> = {
        let mut out = Vec::with_capacity(args.env.len());
        for raw in &args.env {
            out.push(crate::exec::env_overlay::parse_kv(raw)?);
        }
        out
    };

    let timeout_secs = args.timeout_secs.unwrap_or(120);
    let mut ok = 0usize;
    let mut bad = 0usize;
    let mut last_inner: Option<i32> = None;
    let mut all_same = true;
    // L7 (v0.1.3): one redactor per step (declared inside the loop).
    // PEM block state must not leak across steps because a step
    // truncated mid-block would otherwise poison the next step's
    // detection. The composer is cheap to construct; regex are
    // compiled once globally via Lazy.
    // F9 (v0.1.3): when stdin is being forwarded, every step's run is
    // audited (verb=`run`, with stdin_bytes / optional stdin_sha256)
    // so a post-hoc audit can answer "what input did this command
    // consume?" by size. Without forwarded stdin, `inspect run`
    // remains un-audited (matches v0.1.2 read-verb behavior).
    let audit_store = match crate::safety::audit::AuditStore::open() {
        // F13 (v0.1.3): always open the audit store on `run` so the
        // `connect.reauth` audit entry can be written when the
        // dispatch wrapper fires the reauth path. Stdin-forwarded
        // runs additionally write per-step audit entries (F9
        // contract preserved); plain runs write only when the
        // wrapper actually triggers reauth.
        Ok(s) => Some(s),
        Err(e) => {
            if stdin_payload.is_some() {
                eprintln!("warning: audit log unavailable ({e}); proceeding");
            }
            None
        }
    };
    let stdin_audited = stdin_payload.is_some() && audit_store.is_some();
    // F14 (v0.1.3): dedup-store the script body once, before the
    // per-step loop. Errors here are non-fatal — the audit entry
    // still references the body by hash, and the operator's `--file`
    // is still on disk.
    if let Some(sc) = &script {
        if let Err(e) = store_script_body(&sc.body, &sc.sha256) {
            eprintln!("warning: script dedup-store write failed ({e}); proceeding");
        }
    }
    // F13 (v0.1.3): track transport-class outcomes across steps so the
    // SUMMARY trailer / JSON `failure_class` field / exit code can
    // reflect a uniform transport failure when every failed step
    // shares the same class. `Some(c)` means every transport failure
    // seen so far classified as `c`; `None` after a divergent class
    // means we won't promote to `ExitKind::Transport`.
    // SUMMARY trailer / JSON `failure_class` field / exit code can
    // reflect a uniform transport failure when every failed step
    // shares the same class. `Some(c)` means every transport failure
    // seen so far classified as `c`; `None` after a divergent class
    // means we won't promote to `ExitKind::Transport`.
    let mut uniform_transport: Option<crate::ssh::transport::TransportClass> = None;
    let mut transport_failures = 0usize;
    let mut command_failures = 0usize;
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
        let redactor = crate::redact::OutputRedactor::new(args.show_secrets, args.redact_all);

        // Wrap in `docker exec` when the selector points at a container.
        // Apply server-side line filter (--filter-line-pattern) by piping
        // through `grep -E`, mirroring the same pushdown logs/grep use.
        //
        // F14 (v0.1.3): in script-mode, the remote command becomes
        // `<interp> -s -- <args>` (or `<interp> - <args>` for non-bash
        // interpreters). The container variant adds `-i` so docker
        // exec keeps stdin attached for the script body to flow in.
        let inner = if let Some(sc) = &script {
            let invocation = render_script_invocation(&sc.interp, &args.cmd);
            match s.container() {
                Some(container) => {
                    format!("docker exec -i {} {}", shquote(container), invocation)
                }
                None => invocation,
            }
        } else {
            match s.container() {
                Some(container) => format!(
                    "docker exec {} sh -c {}",
                    shquote(container),
                    shquote(&user_cmd)
                ),
                None => user_cmd.clone(),
            }
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

        // F12 (v0.1.3): apply the per-namespace env overlay (merged
        // with `--env` overrides). Overlay is empty when neither the
        // namespace config nor `--env` provides anything, in which
        // case `apply_to_cmd` returns the cmd borrowed unchanged.
        let effective_overlay =
            crate::exec::env_overlay::merge(Some(&s.ns.env_overlay), &user_env, args.env_clear);
        let cmd = crate::exec::env_overlay::apply_to_cmd(&cmd, &effective_overlay).into_owned();
        if args.debug {
            eprintln!("[inspect] rendered command for {}: {}", s.ns.namespace, cmd);
        }

        let svc_name = s.service().unwrap_or("_").to_string();
        let ns_name = s.ns.namespace.clone();
        let step_started = std::time::Instant::now();

        let exit = {
            let policy = crate::exec::dispatch::ReauthPolicy {
                allow_reauth: !args.no_reauth && s.ns.auto_reauth,
            };
            let stdin_payload_ref = stdin_payload.as_ref();
            let cmd_ref = &cmd;
            let ns_name_ref = &ns_name;
            let svc_name_ref = &svc_name;
            let redactor_ref = &redactor;
            let truncated_lines_ref = &mut truncated_lines;
            let runner_ref = runner.as_ref();
            let outcome = crate::exec::dispatch::dispatch_with_reauth(
                ns_name_ref,
                &s.ns.target,
                runner_ref,
                audit_store.as_ref(),
                "run",
                &label,
                policy,
                || {
                    let mut opts_call = RunOpts::with_timeout(timeout_secs);
                    if let Some(bytes) = stdin_payload_ref {
                        opts_call = opts_call.with_stdin(bytes.clone());
                    }
                    runner_ref.run_streaming(
                        ns_name_ref,
                        &s.ns.target,
                        cmd_ref,
                        opts_call,
                        &mut |line| {
                            // L7 (v0.1.3): the redactor returns None
                            // for lines inside (or ending) an active
                            // PEM private-key block. We skip emission
                            // entirely so the BEGIN-line marker is the
                            // only output for the whole block.
                            let masked = match redactor_ref.mask_line(line) {
                                Some(m) => m,
                                None => return,
                            };
                            let masked: std::borrow::Cow<'_, str> = if args.clean_output {
                                match strip_ansi(&masked) {
                                    std::borrow::Cow::Borrowed(_) => {
                                        std::borrow::Cow::Owned(masked.into_owned())
                                    }
                                    std::borrow::Cow::Owned(s) => std::borrow::Cow::Owned(s),
                                }
                            } else {
                                masked
                            };
                            if json {
                                JsonOut::write(
                                    &Envelope::new(ns_name_ref, "run", "run")
                                        .with_service(svc_name_ref)
                                        .put(
                                            "line",
                                            crate::format::safe::safe_machine_line(&masked)
                                                .as_ref(),
                                        ),
                                );
                            } else {
                                let safe =
                                    crate::format::safe::safe_terminal_line(&masked, line_budget);
                                if line_budget != usize::MAX && masked.len() > line_budget {
                                    *truncated_lines_ref += 1;
                                }
                                println!("{label} | {safe}");
                            }
                        },
                    )
                },
            );
            outcome
        };

        // F13: stamp `retry_of` / `reauth_id` / `failure_class` onto
        // every audit entry produced for this step so a downstream
        // consumer can correlate the original failed attempt and its
        // retry across the audit log.
        // F14: also stamp script-mode metadata (path / sha256 / bytes
        // / interp / optional inline body) so the audit JSONL is the
        // single source of truth for "what script ran here?".
        let script_ref = script.as_ref();
        let stamp_audit = |e: &mut crate::safety::audit::AuditEntry, class: Option<&str>| {
            if exit.retried {
                e.retry_of = Some(format!("transport_stale@{}", label));
            }
            if let Some(rid) = &exit.reauth_id {
                e.reauth_id = Some(rid.clone());
            }
            if let Some(c) = class {
                e.failure_class = Some(c.to_string());
            }
            if let Some(sc) = script_ref {
                e.script_path = sc.script_path.clone();
                e.script_sha256 = Some(sc.sha256.clone());
                e.script_bytes = Some(sc.body.len() as u64);
                e.script_interp = Some(sc.interp.clone());
                if args.audit_script_body {
                    // Best-effort UTF-8; binary scripts are pathological
                    // for an audit log anyway, and lossy decode keeps
                    // the field readable.
                    e.script_body = Some(String::from_utf8_lossy(&sc.body).into_owned());
                }
            }
        };

        match (&exit.result, exit.failure_class) {
            (Ok(code), _) => {
                let code = *code;
                if code == 0 {
                    ok += 1;
                } else {
                    bad += 1;
                    command_failures += 1;
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
                // F9 contract: per-step audit only when stdin is being
                // forwarded. F13 widens this to also audit when the
                // wrapper retried (so the retry stamps `retry_of` /
                // `reauth_id` for correlation).
                if stdin_audited || exit.retried {
                    if let Some(store) = &audit_store {
                        let mut e = crate::safety::audit::AuditEntry::new("run", &label);
                        e.args = stamp_args(&user_cmd, args.show_secrets, &redactor);
                        e.exit = code;
                        e.duration_ms = step_started.elapsed().as_millis() as u64;
                        e.reason = reason.clone();
                        e.stdin_bytes = stdin_bytes_len;
                        e.stdin_sha256 = stdin_sha256.clone();
                        if !effective_overlay.is_empty() {
                            e.env_overlay = Some(effective_overlay.clone());
                        }
                        e.rendered_cmd = Some(cmd.clone());
                        e.secrets_masked_kinds = collect_kinds(&redactor);
                        let class = if code == 0 { "ok" } else { "command_failed" };
                        stamp_audit(&mut e, Some(class));
                        let _ = store.append(&e);
                    }
                }
            }
            (Err(e), Some(class)) => {
                bad += 1;
                all_same = false;
                transport_failures += 1;
                uniform_transport = match uniform_transport {
                    None if transport_failures == 1 => Some(class),
                    Some(prev) if prev == class => Some(prev),
                    _ => None,
                };
                if !json {
                    eprintln!("{label}: {e}");
                }
                if let Some(store) = &audit_store {
                    let mut entry = crate::safety::audit::AuditEntry::new("run", &label);
                    entry.args = stamp_args(&user_cmd, args.show_secrets, &redactor);
                    entry.exit = -1;
                    entry.duration_ms = step_started.elapsed().as_millis() as u64;
                    entry.reason = reason.clone();
                    entry.stdin_bytes = stdin_bytes_len;
                    entry.stdin_sha256 = stdin_sha256.clone();
                    entry.diff_summary = format!("transport_error: {e}");
                    if !effective_overlay.is_empty() {
                        entry.env_overlay = Some(effective_overlay.clone());
                    }
                    entry.rendered_cmd = Some(cmd.clone());
                    entry.secrets_masked_kinds = collect_kinds(&redactor);
                    stamp_audit(&mut entry, Some(class.as_str()));
                    let _ = store.append(&entry);
                }
            }
            (Err(e), None) => {
                bad += 1;
                all_same = false;
                if !json {
                    eprintln!("{label}: {e}");
                }
                if stdin_audited {
                    if let Some(store) = &audit_store {
                        let mut entry = crate::safety::audit::AuditEntry::new("run", &label);
                        entry.args = stamp_args(&user_cmd, args.show_secrets, &redactor);
                        entry.exit = -1;
                        entry.duration_ms = step_started.elapsed().as_millis() as u64;
                        entry.reason = reason.clone();
                        entry.stdin_bytes = stdin_bytes_len;
                        entry.stdin_sha256 = stdin_sha256.clone();
                        entry.diff_summary = format!("transport_error: {e}");
                        if !effective_overlay.is_empty() {
                            entry.env_overlay = Some(effective_overlay.clone());
                        }
                        entry.rendered_cmd = Some(cmd.clone());
                        entry.secrets_masked_kinds = collect_kinds(&redactor);
                        let _ = store.append(&entry);
                    }
                }
            }
        }
    }

    // F13 (v0.1.3): determine the verb-level failure_class. Priority:
    // 1. Uniform transport class across every failure → that class.
    // 2. Any transport failure mixed with command failures → still
    //    surface the transport class because it's the more actionable
    //    signal (re-auth / SSH topology). We leave the exit-code path
    //    in `ExitKind::Error` though, since the run wasn't uniformly
    //    transport-failed.
    // 3. Command failures only → "command_failed".
    // 4. All ok → "ok".
    let verb_failure_class: &'static str = if let Some(c) = uniform_transport {
        c.as_str()
    } else if transport_failures > 0 {
        // Mixed: prefer the transport hint over command_failed for
        // operator visibility.
        "transport_mixed"
    } else if command_failures > 0 {
        "command_failed"
    } else {
        "ok"
    };

    if !json {
        let mut r = Renderer::new();
        let trailer = if let Some(c) = uniform_transport {
            format!(
                "run: {ok} ok, {bad} failed ({})",
                c.summary_hint(&args.selector)
            )
        } else {
            format!("run: {ok} ok, {bad} failed")
        };
        r.summary(trailer);
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
    } else {
        // F13: emit a final summary envelope so JSON consumers can
        // read the verb-level outcome (ok/failed counts +
        // failure_class) in one structured record. Streaming line
        // envelopes earlier in the run remain unchanged.
        JsonOut::write(
            &Envelope::new(&args.selector, "run", "run")
                .put("phase", "summary")
                .put("ok", ok)
                .put("failed", bad)
                .put("failure_class", verb_failure_class),
        );
    }

    // F13: when every failure shared the same transport class, exit
    // with the dedicated transport exit code (12/13/14) and a chained
    // hint. Mixed / command-only failures fall through to existing
    // `ExitKind::Inner` / `ExitKind::Error` logic.
    if let Some(c) = uniform_transport {
        if bad > 0 {
            return Ok(ExitKind::Transport(c));
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

/// L7 (v0.1.3): tag the audit `args` text with the redaction outcome,
/// matching the `inspect exec` convention so audit reviewers can tell
/// `[secrets_exposed=true]` (operator opted out via `--show-secrets`)
/// from `[secrets_masked=true]` (the redactor fired during this step)
/// from a clean run (neither tag).
fn stamp_args(
    user_cmd: &str,
    show_secrets: bool,
    redactor: &crate::redact::OutputRedactor,
) -> String {
    if show_secrets {
        format!("{user_cmd} [secrets_exposed=true]")
    } else if redactor.was_active() {
        format!("{user_cmd} [secrets_masked=true]")
    } else {
        user_cmd.to_string()
    }
}

/// L7 (v0.1.3): collect the redactor's per-kind activity for
/// `AuditEntry::secrets_masked_kinds`. Returns `None` (so
/// `skip_serializing_if` elides the field) when no masker fired.
fn collect_kinds(redactor: &crate::redact::OutputRedactor) -> Option<Vec<String>> {
    let kinds = redactor.active_kinds();
    if kinds.is_empty() {
        None
    } else {
        Some(kinds.into_iter().map(|s| s.to_string()).collect())
    }
}
