//! `Inspect compose logs
//! <ns>/<project>[/<service>]` — aggregated logs for a project, or
//! one service inside it.
//!
//! Wraps `cd <wd> && docker compose -p <p> logs [--tail N] [--since
//! X] [--follow] [--timestamps] [<svc>]` over the persistent ssh
//! socket. brings the verb up to feature-parity with the
//! generic `inspect logs` verb's triage surface:
//!
//! - `--match <REGEX>` / `--exclude <REGEX>` (repeatable; OR within
//!   each, AND across the two). Reuses `verbs::line_filter` so the
//!   filter compiles to a remote `grep -E` pipeline — the SSH
//!   transport never ferries lines we are about to drop.
//! - `--merged` is an assertion flag: this is a multi-service
//!   interleaved stream. Project-level form is already interleaved
//!   (compose's default), so `--merged` is documentation +
//!   discoverability + rejection of per-service selectors.
//! - `--cursor <PATH>` resumes from the ISO-8601 timestamp recorded
//!   in the cursor file. Forces `--timestamps` so the output lines
//!   carry the prefix needed for the next resume; on stream
//!   completion, the latest timestamp is written back to the
//!   cursor file (atomic write via `<file>.tmp.<pid>` → rename).
//!   Mutex with `--since` — both pin the start.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

use crate::cli::ComposeLogsArgs;
use crate::error::ExitKind;
use crate::redact::OutputRedactor;
use crate::ssh::exec::RunOpts;
use crate::verbs::line_filter::build_suffix;
use crate::verbs::quote::shquote;
use crate::verbs::runtime::{current_runner, resolve_target, RemoteRunner};

use super::resolve::{project_in_profile, Parsed};

pub fn run(args: ComposeLogsArgs) -> Result<ExitKind> {
    let parsed = match Parsed::parse(&args.selector) {
        Ok(p) => p,
        Err(e) => {
            crate::error::emit(format!("{e}"));
            return Ok(ExitKind::Error);
        }
    };
    let project_name = match parsed.project.as_deref() {
        Some(p) => p,
        None => {
            crate::error::emit(format!(
                "selector '{}' is missing the project portion — \
                 expected '<ns>/<project>[/<service>]'",
                args.selector
            ));
            return Ok(ExitKind::Error);
        }
    };
    let (_profile, project) = match project_in_profile(&parsed.namespace, project_name) {
        Ok(t) => t,
        Err(e) => {
            crate::error::emit(format!("{e}"));
            return Ok(ExitKind::NoMatches);
        }
    };

    // --merged asserts a multi-service stream. Reject
    // per-service selectors so the contract is unambiguous.
    if args.merged && parsed.service.is_some() {
        crate::error::emit(format!(
            "compose logs --merged is incompatible with the per-service \
             selector '{}'. --merged means 'every service in the \
             project, interleaved'; for one service, drop the flag and \
             pass `<ns>/<project>/<service>`.",
            args.selector
        ));
        return Ok(ExitKind::Error);
    }

    // --cursor + --follow is allowed and useful (resume
    // from the last seen timestamp and continue tailing). The cursor
    // is updated only when the verb returns control (--follow exits
    // when ssh's read-loop ends).

    let runner = current_runner();
    let (_resolved, target) = resolve_target(&parsed.namespace)?;

    // Read cursor's stored timestamp (if any) — passed to compose as
    // `--since`. Missing or empty cursor is fine; the next call will
    // populate it.
    let cursor_since = match args.cursor.as_deref() {
        Some(path) => read_cursor(path)?,
        None => None,
    };
    let force_timestamps = args.cursor.is_some();

    // Build the docker compose logs invocation.
    let mut parts: Vec<String> = vec![
        format!("cd {wd} &&", wd = shquote(&project.working_dir)),
        format!("docker compose -p {p} logs", p = shquote(&project.name)),
        // `--no-color` so the redaction pipeline's regexes don't
        // have to fight ANSI sequences. Operators who need color
        // can drop back to `inspect run -- 'docker compose logs ...'`.
        "--no-color".into(),
        // `--no-log-prefix` would strip the `[svc]` prefix; we
        // *want* it, both for human reading and for the JSON
        // envelope's `service` field, so we leave the default on.
    ];
    if force_timestamps {
        // `-t` / `--timestamps` adds an ISO-8601 prefix to each line.
        // Required for cursor-based resume.
        parts.push("--timestamps".into());
    }
    if let Some(tail) = args.tail {
        parts.push(format!("--tail {tail}"));
    }
    let effective_since = cursor_since.as_deref().or(args.since.as_deref());
    if let Some(since) = effective_since {
        parts.push(format!("--since {}", shquote(since)));
    }
    if args.follow {
        parts.push("--follow".into());
    }
    if let Some(svc) = parsed.service.as_deref() {
        parts.push(shquote(svc));
    }
    let mut cmd = parts.join(" ");
    // Line-filter pipeline pushed down to the remote so the SSH
    // transport never carries lines we're about to drop. The live
    // mode (--follow) uses `--line-buffered` so streaming round-trips
    // immediately.
    cmd.push_str(&build_suffix(&args.r#match, &args.exclude, args.follow));

    // Streaming with redaction. We pipe each line through the
    // maskers and emit in real time so `--follow` is responsive.
    let redactor = OutputRedactor::new(args.show_secrets, false);
    // Long timeout for `--follow`, normal otherwise — matches
    // `inspect logs --follow`'s 8h convention.
    let timeout = if args.follow {
        RunOpts::with_timeout(8 * 3600)
    } else {
        RunOpts::with_timeout(60)
    };
    let (exit, last_ts) = stream_with_redaction(
        runner.as_ref(),
        &parsed.namespace,
        &target,
        &cmd,
        timeout,
        &redactor,
        force_timestamps,
    )?;

    // Write the cursor when one was supplied AND we
    // captured a timestamp. Skip when the stream produced no
    // lines (the cursor stays whatever it was; an empty stream
    // shouldn't reset the operator's resume point).
    if let (Some(path), Some(ts)) = (args.cursor.as_deref(), last_ts) {
        if let Err(e) = write_cursor(path, &ts) {
            // Cursor write failures are warnings, not errors — the
            // logs were emitted correctly; the operator just can't
            // resume cleanly next time.
            eprintln!("warning: cursor write to '{}' failed: {e}", path.display());
        }
    }

    if exit == 0 {
        Ok(ExitKind::Success)
    } else {
        Ok(ExitKind::Error)
    }
}

/// Stream the remote command, mask each line, and (when
/// `force_timestamps` is set) capture the latest ISO-8601 timestamp
/// for cursor write-back. Returns `(remote_exit, last_timestamp)`.
fn stream_with_redaction(
    runner: &dyn RemoteRunner,
    ns: &str,
    target: &crate::ssh::options::SshTarget,
    cmd: &str,
    opts: RunOpts,
    redactor: &OutputRedactor,
    capture_ts: bool,
) -> Result<(i32, Option<String>)> {
    let mut last_ts: Option<String> = None;
    let exit = runner.run_streaming(ns, target, cmd, opts, &mut |line| {
        if capture_ts {
            if let Some(ts) = extract_iso8601_ts(line) {
                last_ts = Some(ts);
            }
        }
        if let Some(masked) = redactor.mask_line(line) {
            crate::transcript::emit_stdout(&masked);
        }
    })?;
    Ok((exit, last_ts))
}

/// Pull the ISO-8601 timestamp prefix out of a docker
/// compose `--timestamps` line. The prefix shape is one of:
///
///   service_name  | 2024-01-15T12:34:56.789012345Z message
///   2024-01-15T12:34:56.789012345Z message
///
/// We accept either form — the per-service narrowing strips the
/// service prefix. Returns `None` when no timestamp prefix is
/// found (e.g., a stderr line, a malformed log).
fn extract_iso8601_ts(line: &str) -> Option<String> {
    // Skip optional `<service>  | ` prefix.
    let body = match line.find("| ") {
        Some(idx) => &line[idx + 2..],
        None => line,
    };
    // ISO-8601 with optional fractional seconds + 'Z' / offset. We
    // accept any prefix shaped like `YYYY-MM-DDTHH:MM:SS` followed
    // by either `Z`, `+HH:MM`, `-HH:MM`, or `.<digits>` then one of
    // those. Rather than ship a regex (the line is hot path; we
    // want a minimal byte-walk), we parse char-by-char.
    let bytes = body.as_bytes();
    if bytes.len() < 20 {
        return None;
    }
    fn d(b: u8) -> bool {
        b.is_ascii_digit()
    }
    if !(d(bytes[0])
        && d(bytes[1])
        && d(bytes[2])
        && d(bytes[3])
        && bytes[4] == b'-'
        && d(bytes[5])
        && d(bytes[6])
        && bytes[7] == b'-'
        && d(bytes[8])
        && d(bytes[9])
        && bytes[10] == b'T'
        && d(bytes[11])
        && d(bytes[12])
        && bytes[13] == b':'
        && d(bytes[14])
        && d(bytes[15])
        && bytes[16] == b':'
        && d(bytes[17])
        && d(bytes[18]))
    {
        return None;
    }
    let mut end = 19;
    if end < bytes.len() && bytes[end] == b'.' {
        end += 1;
        while end < bytes.len() && d(bytes[end]) {
            end += 1;
        }
    }
    if end < bytes.len() {
        match bytes[end] {
            b'Z' => end += 1,
            // ±HH:MM
            b'+' | b'-'
                if end + 5 < bytes.len()
                    && d(bytes[end + 1])
                    && d(bytes[end + 2])
                    && bytes[end + 3] == b':'
                    && d(bytes[end + 4])
                    && d(bytes[end + 5]) =>
            {
                end += 6;
            }
            _ => {}
        }
    }
    Some(body[..end].to_string())
}

fn read_cursor(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(s) => {
            let trimmed = s.trim().to_string();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed))
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading cursor at '{}'", path.display())),
    }
}

fn write_cursor(path: &Path, ts: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
    }
    // Atomic write: tempfile in the same dir → rename(2).
    let mut tmp = path.to_path_buf();
    let fname = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("cursor");
    let pid = std::process::id();
    tmp.set_file_name(format!(".{fname}.tmp.{pid}"));

    {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .mode(0o600)
                .open(&tmp)
                .with_context(|| format!("creating cursor temp '{}'", tmp.display()))?;
            f.write_all(ts.as_bytes())?;
            f.write_all(b"\n")?;
            f.sync_all()?;
        }
        #[cfg(not(unix))]
        {
            let mut f = std::fs::File::create(&tmp)
                .with_context(|| format!("creating cursor temp '{}'", tmp.display()))?;
            f.write_all(ts.as_bytes())?;
            f.write_all(b"\n")?;
        }
    }
    std::fs::rename(&tmp, path)
        .with_context(|| format!("renaming cursor temp to '{}'", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l8_extract_iso8601_ts_with_service_prefix() {
        let line = "onyx-vault    | 2024-01-15T12:34:56.789Z hello world";
        assert_eq!(
            extract_iso8601_ts(line),
            Some("2024-01-15T12:34:56.789Z".to_string())
        );
    }

    #[test]
    fn l8_extract_iso8601_ts_without_service_prefix() {
        let line = "2024-01-15T12:34:56.789Z hello world";
        assert_eq!(
            extract_iso8601_ts(line),
            Some("2024-01-15T12:34:56.789Z".to_string())
        );
    }

    #[test]
    fn l8_extract_iso8601_ts_with_offset() {
        let line = "api  | 2024-01-15T12:34:56.789+02:00 hello";
        assert_eq!(
            extract_iso8601_ts(line),
            Some("2024-01-15T12:34:56.789+02:00".to_string())
        );
    }

    #[test]
    fn l8_extract_iso8601_ts_without_fractional_seconds() {
        let line = "api  | 2024-01-15T12:34:56Z hello";
        assert_eq!(
            extract_iso8601_ts(line),
            Some("2024-01-15T12:34:56Z".to_string())
        );
    }

    #[test]
    fn l8_extract_iso8601_ts_no_timestamp_returns_none() {
        let line = "api  | this line has no timestamp";
        assert_eq!(extract_iso8601_ts(line), None);
    }

    #[test]
    fn l8_extract_iso8601_ts_empty_line_returns_none() {
        assert_eq!(extract_iso8601_ts(""), None);
    }

    #[test]
    fn l8_cursor_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cur");
        // Missing file is None.
        assert_eq!(read_cursor(&path).unwrap(), None);
        // Write + read.
        write_cursor(&path, "2024-01-15T12:34:56.789Z").unwrap();
        assert_eq!(
            read_cursor(&path).unwrap(),
            Some("2024-01-15T12:34:56.789Z".to_string())
        );
        // Trim trailing newline.
        std::fs::write(&path, "2024-01-15T12:34:56Z\n\n").unwrap();
        assert_eq!(
            read_cursor(&path).unwrap(),
            Some("2024-01-15T12:34:56Z".to_string())
        );
        // Empty file is None.
        std::fs::write(&path, "").unwrap();
        assert_eq!(read_cursor(&path).unwrap(), None);
    }
}
