//! `inspect run <sel> -- <cmd>`.
//!
//! Read-only execution counterpart to [`crate::verbs::write::exec`]. Streams
//! the remote command's output line-by-line, never touches the audit log,
//! and has no `--apply`/confirmation gating. Use when you want to inspect
//! state with an ad-hoc shell snippet (`ps`, `cat /proc/...`, `df -h`,
//! `redis-cli info`, ...) without paying for the write-verb interlock.
//!
//! Operators routinely typed `inspect exec ... -- <read-only thing>` and
//! ran into the exec safety prompts on every iteration; this verb exists
//! so the read-only path doesn't pay that cost.

use anyhow::Result;

use crate::cli::RunArgs;
use crate::error::ExitKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::dispatch::{iter_steps, plan};
use crate::verbs::output::{Envelope, JsonOut, Renderer};
use crate::verbs::quote::shquote;

/// Strip ANSI CSI / OSC escape sequences from a
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

/// Default cap on forwarded stdin per `inspect run` invocation.
/// Above this the verb refuses, with a chained hint pointing at
/// `inspect put` for bulk file transfer (uncapped, audit-tracked,
/// fully revertible).
pub const DEFAULT_STDIN_MAX: u64 = 10 * 1024 * 1024;

/// Parse a size string like `10m`, `512k`, `1g`, or a raw
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

/// Is local stdin a tty? When `true`, `inspect run` does not
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

/// Read local stdin into a `Vec<u8>`, refusing if the
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

/// A resolved script-mode payload. The body is shipped
/// to the remote via `bash -s` (or a different interpreter declared in
/// the script's shebang); `script_path` is `Some(path)` for `--file`
/// mode and `None` for `--stdin-script` mode.
pub(crate) struct ScriptSource {
    pub body: Vec<u8>,
    pub sha256: String,
    pub interp: String,
    pub script_path: Option<String>,
}

/// Interpreter dispatched on the remote when no shebang
/// is declared. `bash -s` is a strict superset of `sh -s` for the
/// cross-layer-quoting use case that drove script mode; if a target
/// lacks `bash`, the operator declares `#!/bin/sh` in the script and
/// the runner honors it.
const DEFAULT_SCRIPT_INTERP: &str = "bash";

/// Build the per-(SHA, pid) remote temp path for the
/// two-phase script delivery. The pid suffix prevents two
/// concurrent `inspect run` invocations on the same script from
/// stomping on each other's temp file (rare but real on shared
/// developer hosts). The leading `.` keeps it out of plain `ls
/// /tmp` output.
fn build_remote_script_temp_path(sha256: &str) -> String {
    let sha_short: String = sha256.chars().take(8).collect();
    let pid = std::process::id();
    format!("/tmp/.inspect-l11-{sha_short}-{pid}.sh")
}

/// Parse the first line of a script for `#!` and pick
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

/// Allow only `[A-Za-z0-9_.-]` in interpreter names so
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

/// Does this interpreter accept `-s` as the
/// "read-script-from-stdin" flag? Bash and `sh` (POSIX) do; everything
/// else (python, node, ruby, ...) takes a bare `-` instead.
fn interp_uses_dash_s(interp: &str) -> bool {
    matches!(interp, "bash" | "sh" | "zsh" | "ksh" | "dash")
}

/// Render the remote dispatch shape for a script-mode
/// invocation. For bash-family interpreters this is `bash -s -- a b c`
/// (positional args land in `$1` / `$2` / ...). For others (python,
/// node, ...) this is `python3 - a b c` (POSIX convention; args land
/// in `sys.argv[1:]` etc.).
fn render_script_invocation(interp: &str, args: &[String]) -> String {
    // Interpreter is a controlled identifier (parsed from the
    // shebang's basename or the default `bash`); rendering it
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

/// Resolve the script-mode payload (or `None` if the
/// caller is in classic argv-cmd mode). Reads the file or stdin once,
/// hashes it, and detects the interpreter from the shebang. Honors
/// the same `--stdin-max` cap as ordinary stdin forwarding.
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
            crate::safety::snapshot::hex_encode(&h.finalize())
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
            crate::safety::snapshot::hex_encode(&h.finalize())
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

/// Write the script body to the content-addressed store
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
    // SMOKE 2026-05-09 fail-fast (v0.1.3): catch the
    // `inspect run --apply` muscle-memory trap before any dispatch.
    // `inspect run` is read-only; the audited mutation verb is
    // `inspect exec --apply`. The `apply` field on `RunArgs` exists
    // *only* to surface a clear chained hint here — without the
    // explicit flag, `--apply` would slip into the trailing-var-arg
    // cmd vec and the remote `bash -c` would die with
    // `bash: --: invalid option`. The pre-fix `inspect run --apply`
    // recipes in the smoke runbook were also wrong (now
    // `inspect exec --apply` / structured lifecycle verbs in
    // `docs/SMOKE_v0.1.3.md`).
    if args.apply {
        crate::error::emit(
            "`--apply` is not on `inspect run` (which is read-only and not audited). \
             For audited mutations use `inspect exec --apply -- '<command>'` (free-form, requires \
             `--no-revert` because the inverse cannot be synthesised) or the structured write \
             verbs (`inspect stop` / `restart` / `start` / `put` / `chmod` / `chown` / `edit` / \
             `rm` / `mkdir` / `touch`) which capture a real inverse.",
        );
        return Ok(ExitKind::Error);
    }
    // Multi-step runner mode short-circuits the
    // classic per-target dispatch loop. The steps module owns its
    // own selector resolution, env-overlay merge, per-step audit
    // entries, and parent composite-revert capture; the bare-`run`
    // path below is bypassed entirely. Mutual exclusion with
    // `--file` / `--stdin-script` / cross-mutex between `--steps`
    // and `--steps-yaml` is clap-enforced (see RunArgs.steps and
    // RunArgs.steps_yaml).
    if args.steps.is_some() || args.steps_yaml.is_some() {
        return crate::verbs::steps::run(&args);
    }
    // Script mode (`--file` / `--stdin-script`) does not
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
        crate::tee_eprintln!("# reason: {r}");
    }

    let fmt = args.format.resolve()?;
    let json = matches!(fmt, crate::format::OutputFormat::Json);

    // ---------------------------------------------------------------
    // Stdin handling. Decide what (if anything) to
    // forward to the remote command's stdin BEFORE we resolve targets
    // or dial out — both --no-stdin's loud-failure and the size-cap
    // exit must fire pre-dispatch, with zero remote commands issued.
    //
    // Three cases:
    //   * `--file <path>`    : script body is the local file; remote
    //                          command becomes `bash -s -- <args>`
    //                          and the body rides on the stdin pipe.
    //   * `--stdin-script`   : script body is local stdin (must be
    //                          non-tty, non-empty).
    //   * neither set        : pipe local stdin to the
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
    // Resolve the script source first. If this fails (file
    // missing, stdin tty, payload above cap), exit 2 BEFORE dispatch
    // — same invariant as ordinary stdin forwarding.
    let script: Option<ScriptSource> = match resolve_script_source(&args, cap_bytes) {
        Ok(s) => s,
        Err(e) => {
            crate::error::emit(format!("{e}"));
            return Ok(ExitKind::Error);
        }
    };
    let stdin_payload: Option<Vec<u8>> = if let Some(s) = &script {
        // Script body claims the remote stdin pipe. Local stdin
        // is NOT additionally forwarded — the spec is explicit that
        // operators wanting both pipe a script via `--file` and let
        // ordinary stdin forwarding flow through (a future
        // follow-up).
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
            crate::safety::snapshot::hex_encode(&h.finalize())
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

    // Construct the streaming `--select` filter ONCE at
    // function entry so a parse error fails fast before any frame is
    // emitted. The streaming line closure captures a `&mut Option<…>`
    // re-borrow each per-step iteration; the post-stream summary
    // envelope at the bottom of this verb shares the same filter so a
    // single `--select '.line'` (or `select(.phase==…)`) covers both
    // stream frames and the run-level summary uniformly.
    let mut select = args.format.select_filter()?;

    // Per-invocation env overrides. Validate once,
    // before the per-step loop, so a typo in `--env` short-circuits
    // the whole run instead of failing N times.
    let user_env: Vec<(String, String)> = {
        let mut out = Vec::with_capacity(args.env.len());
        for raw in &args.env {
            out.push(crate::exec::env_overlay::parse_kv(raw)?);
        }
        out
    };

    // Streaming runs default to an 8-hour timeout
    // (matches `inspect logs --follow`) since the operator is
    // expected to terminate via Ctrl-C, not by reaching the timeout.
    // Non-streaming runs keep the existing 120s default. The operator
    // can override either default with `--timeout-secs`.
    let timeout_secs = args
        .timeout_secs
        .unwrap_or(if args.stream { 60 * 60 * 8 } else { 120 });
    let mut ok = 0usize;
    let mut bad = 0usize;
    let mut last_inner: Option<i32> = None;
    let mut all_same = true;
    // One redactor per step (declared inside the loop).
    // PEM block state must not leak across steps because a step
    // truncated mid-block would otherwise poison the next step's
    // detection. The composer is cheap to construct; regex are
    // compiled once globally via Lazy.
    // When stdin is being forwarded, every step's run is
    // audited (verb=`run`, with stdin_bytes / optional stdin_sha256)
    // so a post-hoc audit can answer "what input did this command
    // consume?" by size. Without forwarded stdin, `inspect run`
    // remains un-audited (matches v0.1.2 read-verb behavior).
    let audit_store = match crate::safety::audit::AuditStore::open() {
        // Always open the audit store on `run` so the
        // `connect.reauth` audit entry can be written when the
        // dispatch wrapper fires the reauth path. Stdin-forwarded
        // runs additionally write per-step audit entries (the
        // stdin-audit contract preserved); plain runs write only
        // when the wrapper actually triggers reauth.
        Ok(s) => Some(s),
        Err(e) => {
            if stdin_payload.is_some() {
                crate::tee_eprintln!("warning: audit log unavailable ({e}); proceeding");
            }
            None
        }
    };
    let stdin_audited = stdin_payload.is_some() && audit_store.is_some();
    // Streamed runs are always audited so a post-hoc
    // audit can tell `--stream` invocations apart from short-lived
    // commands (e.g. `tail -f` vs `ls -la`) without parsing args text.
    let stream_audited = args.stream && audit_store.is_some();
    // Dedup-store the script body once, before the
    // per-step loop. Errors here are non-fatal — the audit entry
    // still references the body by hash, and the operator's `--file`
    // is still on disk.
    if let Some(sc) = &script {
        if let Err(e) = store_script_body(&sc.body, &sc.sha256) {
            crate::tee_eprintln!("warning: script dedup-store write failed ({e}); proceeding");
        }
    }
    // Track transport-class outcomes across steps so the
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

    // Bidirectional dispatch is `--stream` + a script
    // source. This combo was originally clap-rejected because
    // feeding the script body via SSH stdin and forcing `-tt` PTY
    // for streaming put both directions through the same tty layer
    // (line-discipline echo, cooked-mode munging, interactive bash
    // prompts on a non-tty stdin). The split: phase 1 cats the
    // script body into a remote temp file with no PTY (buffered),
    // phase 2 runs the temp file with PTY for line-streaming. The
    // directions never interleave.
    let bidirectional = args.stream && script.is_some();

    for s in &steps {
        if crate::exec::cancel::is_cancelled() {
            break;
        }
        let svc_label = s.service().map(|x| format!("/{x}")).unwrap_or_default();
        let label = format!("{}{}", s.ns.namespace, svc_label);
        let redactor = crate::redact::OutputRedactor::new(args.show_secrets, args.redact_all);

        // If bidirectional, do phase 1 (write the script bytes to a
        // remote temp file) before constructing the streaming
        // command. The temp file's lifecycle is bounded by phase 3
        // (cleanup, after the streaming dispatch) so a Ctrl-C
        // between phases leaves at most one orphaned
        // `.inspect-l11-<sha>-<pid>.sh` file per namespace —
        // bounded, signed (the SHA in the name maps back to the
        // audit entry), and cheap to manually clean.
        let bidir_temp_path: Option<String> = if bidirectional {
            let sc = script
                .as_ref()
                .expect("bidirectional implies script.is_some()");
            let path = build_remote_script_temp_path(&sc.sha256);
            // Phase 1 command shape differs between host and
            // container selectors:
            //   host       : umask 077; cat > <p> && chmod 700 <p>
            //   container  : docker exec -i <ctr> sh -c 'umask 077; cat > <p> && chmod 700 <p>'
            // Both pipe the script body via SSH stdin in a single
            // round-trip (no PTY).
            let host_cmd = format!("umask 077; cat > {p} && chmod 700 {p}", p = shquote(&path),);
            let phase1_cmd = match s.container() {
                Some(container) => format!(
                    "docker exec -i {} sh -c {}",
                    shquote(container),
                    shquote(&host_cmd),
                ),
                None => host_cmd,
            };
            let phase1_opts =
                crate::ssh::exec::RunOpts::with_timeout(60).with_stdin(sc.body.clone());
            let phase1_out =
                match runner
                    .as_ref()
                    .run(&s.ns.namespace, &s.ns.target, &phase1_cmd, phase1_opts)
                {
                    Ok(o) => o,
                    Err(e) => {
                        crate::error::emit(format!(
                            "L11 phase 1 (script write) on {ns} failed: {e}. \
                         hint: check /tmp writability and disk space; \
                         `inspect put` (F15) writes scripts as a separate verb.",
                            ns = s.ns.namespace,
                        ));
                        return Ok(ExitKind::Error);
                    }
                };
            if !phase1_out.ok() {
                crate::error::emit(format!(
                    "L11 phase 1 (script write) on {ns} exited {ec}: {err}. \
                     hint: check /tmp writability and disk space; \
                     `inspect put` (F15) writes scripts as a separate verb.",
                    ns = s.ns.namespace,
                    ec = phase1_out.exit_code,
                    err = phase1_out.stderr.trim(),
                ));
                return Ok(ExitKind::Error);
            }
            Some(path)
        } else {
            None
        };

        // Wrap in `docker exec` when the selector points at a container.
        // Apply server-side line filter (--filter-line-pattern) by piping
        // through `grep -E`, mirroring the same pushdown logs/grep use.
        //
        // In script-mode, the remote command becomes
        // `<interp> -s -- <args>` (or `<interp> - <args>` for non-bash
        // interpreters). The container variant adds `-i` so docker
        // exec keeps stdin attached for the script body to flow in.
        //
        // When bidirectional, the remote command instead
        // runs the temp file written in phase 1. No stdin payload is
        // forwarded in phase 2 — the body is already on disk on the
        // remote.
        let inner = if let Some(temp_path) = bidir_temp_path.as_deref() {
            let interp = &script.as_ref().unwrap().interp;
            let positional: Vec<String> = args.cmd.iter().map(|a| shquote(a)).collect();
            // The pre-fix shape was `<interp> <temp> -- <args>` (e.g.
            // `bash /tmp/script.sh -- inspect-smoke-X`). Without
            // `-s`, bash treats `<temp>` as the script and `--` as a
            // literal positional arg — so the script saw `$1=--` and
            // `$2=inspect-smoke-X` instead of `$1=inspect-smoke-X`.
            // The corrected shape `<interp> -- <temp> <args>` puts
            // the leading `--` ahead of `<temp>` so it terminates
            // bash's option parsing, then `<temp>` becomes the
            // script and `<args>` become its positionals
            // (`$1=arg1`). Handles args starting with `-` correctly
            // (still a positional, not a bash flag).
            let body = if positional.is_empty() {
                format!("{interp} -- {temp}", temp = shquote(temp_path))
            } else {
                format!(
                    "{interp} -- {temp} {}",
                    positional.join(" "),
                    temp = shquote(temp_path),
                )
            };
            // Phase 3 is a separate dispatch after the streaming
            // run returns; we deliberately do NOT chain `; rm -f`
            // into the streaming command because (a) under PTY a
            // mid-stream Ctrl-C may bypass the cleanup and (b) the
            // cleanup's own output would tail-end the operator's
            // streaming view. Cleanup runs unconditionally below.
            match s.container() {
                Some(container) => format!(
                    "docker exec {} sh -c {}",
                    shquote(container),
                    shquote(&body)
                ),
                None => body,
            }
        } else if let Some(sc) = &script {
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
        // `--clean-output` prepends `TERM=dumb` so any
        // remote tool that consults $TERM (less, ls --color=auto,
        // progress bars) downgrades to plain text. ANSI stripping
        // happens client-side post-mask as a belt-and-braces second
        // line of defense.
        let cmd = if args.clean_output {
            format!("TERM=dumb {cmd}")
        } else {
            cmd
        };

        // Apply the per-namespace env overlay (merged
        // with `--env` overrides). Overlay is empty when neither the
        // namespace config nor `--env` provides anything, in which
        // case `apply_to_cmd` returns the cmd borrowed unchanged.
        let effective_overlay =
            crate::exec::env_overlay::merge(Some(&s.ns.env_overlay), &user_env, args.env_clear);
        let cmd = crate::exec::env_overlay::apply_to_cmd(&cmd, &effective_overlay).into_owned();
        if args.debug {
            crate::tee_eprintln!("[inspect] rendered command for {}: {}", s.ns.namespace, cmd);
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
            // Per-step re-borrow of the verb-level
            // `--select` filter handle so the streaming closure can
            // reach it without owning it across per-step iterations.
            let select_ref: &mut Option<crate::query::ndjson::Filter> = &mut select;
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
                    // In bidirectional mode the script
                    // body was already shipped in phase 1; phase 2
                    // must NOT also pipe it via stdin (would re-feed
                    // it as input to the running script and corrupt
                    // semantics). Only forward stdin in non-
                    // bidirectional script mode (the ordinary script-mode path).
                    if !bidirectional {
                        if let Some(bytes) = stdin_payload_ref {
                            opts_call = opts_call.with_stdin(bytes.clone());
                        }
                    }
                    // Force PTY allocation for --stream
                    // so the remote process line-buffers and SIGINT
                    // propagates back through the tty layer.
                    opts_call = opts_call.with_tty(args.stream);
                    runner_ref.run_streaming(
                        ns_name_ref,
                        &s.ns.target,
                        cmd_ref,
                        opts_call,
                        &mut |line| {
                            // The redactor returns None
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
                                if let Err(e) = JsonOut::write(
                                    &Envelope::new(ns_name_ref, "run", "run")
                                        .with_service(svc_name_ref)
                                        .put(
                                            "line",
                                            crate::format::safe::safe_machine_line(&masked)
                                                .as_ref(),
                                        ),
                                    select_ref.as_mut(),
                                ) {
                                    crate::error::emit(format!("run stream emit: {e}"));
                                }
                            } else {
                                let safe =
                                    crate::format::safe::safe_terminal_line(&masked, line_budget);
                                if line_budget != usize::MAX && masked.len() > line_budget {
                                    *truncated_lines_ref += 1;
                                }
                                crate::tee_println!("{label} | {safe}");
                            }
                        },
                    )
                },
            );
            outcome
        };

        // Stamp `retry_of` / `reauth_id` / `failure_class` onto
        // every audit entry produced for this step so a downstream
        // consumer can correlate the original failed attempt and its
        // retry across the audit log.
        // Also stamp script-mode metadata (path / sha256 / bytes
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
                        crate::tee_eprintln!("{label}: exit {code}");
                    }
                }
                if let Some(prev) = last_inner {
                    if prev != code {
                        all_same = false;
                    }
                }
                last_inner = Some(code);
                // Per-step audit only when stdin is being forwarded;
                // also audited when the wrapper retried (so the
                // retry stamps `retry_of` / `reauth_id` for
                // correlation).
                if stdin_audited || stream_audited || exit.retried {
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
                        e.rendered_cmd = Some(redact_rendered(&cmd, args.show_secrets));
                        e.secrets_masked_kinds = collect_kinds(&redactor);
                        e.streamed = args.stream;
                        e.bidirectional = bidirectional;
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
                    crate::tee_eprintln!("{label}: {e}");
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
                    entry.rendered_cmd = Some(redact_rendered(&cmd, args.show_secrets));
                    entry.secrets_masked_kinds = collect_kinds(&redactor);
                    entry.streamed = args.stream;
                    entry.bidirectional = bidirectional;
                    stamp_audit(&mut entry, Some(class.as_str()));
                    let _ = store.append(&entry);
                }
            }
            (Err(e), None) => {
                bad += 1;
                all_same = false;
                if !json {
                    crate::tee_eprintln!("{label}: {e}");
                }
                if stdin_audited || stream_audited {
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
                        entry.rendered_cmd = Some(redact_rendered(&cmd, args.show_secrets));
                        entry.secrets_masked_kinds = collect_kinds(&redactor);
                        entry.streamed = args.stream;
                        entry.bidirectional = bidirectional;
                        let _ = store.append(&entry);
                    }
                }
            }
        }

        // Phase 3 — clean up the remote temp file
        // unconditionally after the streaming dispatch returns.
        // Failures here are warnings, not errors — the operator's
        // verb has already run; an orphaned `.inspect-l11-*.sh`
        // file is a small, signed (the SHA in the name maps back
        // to the audit entry) bounded leak.
        if let Some(temp_path) = bidir_temp_path.as_deref() {
            let host_cmd = format!("rm -f {p}", p = shquote(temp_path));
            let phase3_cmd = match s.container() {
                Some(container) => format!(
                    "docker exec {} sh -c {}",
                    shquote(container),
                    shquote(&host_cmd),
                ),
                None => host_cmd,
            };
            let phase3_opts = crate::ssh::exec::RunOpts::with_timeout(15);
            match runner
                .as_ref()
                .run(&s.ns.namespace, &s.ns.target, &phase3_cmd, phase3_opts)
            {
                Ok(o) if o.ok() => {}
                Ok(o) => {
                    crate::tee_eprintln!(
                        "warning: L11 phase 3 (script cleanup) on {ns} exited {ec}: \
                         {temp} may be orphaned. {err}",
                        ns = s.ns.namespace,
                        ec = o.exit_code,
                        temp = temp_path,
                        err = o.stderr.trim(),
                    );
                }
                Err(e) => {
                    crate::tee_eprintln!(
                        "warning: L11 phase 3 (script cleanup) on {ns} failed: {e}; \
                         {temp} may be orphaned",
                        ns = s.ns.namespace,
                        temp = temp_path,
                    );
                }
            }
        }
    }

    // Determine the verb-level failure_class. Priority:
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
            crate::tee_eprintln!(
                "── output truncated: {n} line{s} exceeded the {budget}-byte per-line cap (re-run with --no-truncate to see full content) ──",
                n = truncated_lines,
                s = if truncated_lines == 1 { "" } else { "s" },
                budget = line_budget,
            );
        }
    } else {
        // Emit a final summary envelope so JSON consumers can
        // read the verb-level outcome (ok/failed counts +
        // failure_class) in one structured record. Streaming line
        // envelopes earlier in the run remain unchanged.
        // The same `--select` filter that gated the
        // streaming line envelopes also gates this summary envelope,
        // so a single `--select '.phase'` (or
        // `select(.failure_class!=null)`) covers both per-frame and
        // verb-level emission uniformly.
        JsonOut::write(
            &Envelope::new(&args.selector, "run", "run")
                .put("phase", "summary")
                .put("ok", ok)
                .put("failed", bad)
                .put("failure_class", verb_failure_class),
            select.as_mut(),
        )?;
        crate::verbs::output::flush_filter(select.as_mut())?;
    }

    // When every failure shared the same transport class, exit
    // with the dedicated transport exit code (12/13/14) and a chained
    // hint. Mixed / command-only failures fall through to existing
    // `ExitKind::Inner` / `ExitKind::Error` logic.
    if let Some(c) = uniform_transport {
        if bad > 0 {
            return Ok(ExitKind::Transport(c));
        }
    }

    // Surface the remote command's exit code on
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

/// Tag the audit `args` text with the redaction outcome,
/// matching the `inspect exec` convention so audit reviewers can tell
/// `[secrets_exposed=true]` (operator opted out via `--show-secrets`)
/// from `[secrets_masked=true]` (the redactor fired during this step)
/// from a clean run (neither tag).
///
/// G2 (post-v0.1.3 audit hardening): the `user_cmd` text itself is
/// passed through [`crate::redact::redact_for_audit`] so embedded
/// secrets (`psql -p s3cret`, `curl -H "Authorization: Bearer …"`,
/// `DATABASE_URL=postgres://u:p@h/d`) never reach the audit log in
/// plaintext. When `--show-secrets` is set the operator has explicitly
/// opted into verbatim recording and the original text is preserved
/// alongside the `[secrets_exposed=true]` tag.
fn stamp_args(
    user_cmd: &str,
    show_secrets: bool,
    redactor: &crate::redact::OutputRedactor,
) -> String {
    if show_secrets {
        format!("{user_cmd} [secrets_exposed=true]")
    } else {
        let masked = crate::redact::redact_for_audit(user_cmd);
        if redactor.was_active() || matches!(&masked, std::borrow::Cow::Owned(_)) {
            format!("{} [secrets_masked=true]", masked.as_ref())
        } else {
            user_cmd.to_string()
        }
    }
}

/// Collect the redactor's per-kind activity for
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

/// G2 (post-v0.1.3 audit hardening): redact the wrapped shell command
/// stored in `AuditEntry::rendered_cmd`. The wrapped form (e.g.
/// `docker exec ctr sh -c '<user_cmd>'`) embeds whatever the operator
/// typed and would otherwise leak secrets to the audit log even when
/// the `args` field is masked. With `--show-secrets` the operator has
/// opted into verbatim recording.
fn redact_rendered(cmd: &str, show_secrets: bool) -> String {
    if show_secrets {
        cmd.to_string()
    } else {
        crate::redact::redact_for_audit(cmd).into_owned()
    }
}
