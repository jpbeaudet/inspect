//! Help-output renderer.
//!
//! Responsibilities:
//! * Detect terminal width (env override, `terminal_size`, fallback 80).
//! * Honor `NO_COLOR` / `INSPECT_NO_COLOR` (HP-0 emits no color anyway,
//!   but the contract is set so HP-6 can add highlighting safely).
//! * Pipe through a pager when stdout is a tty and pager use isn't
//!   suppressed; otherwise write directly.
//!
//! The body content is rendered **verbatim** — the markdown source is
//! authored to read well as plain text (see HELP-BIBLE §9 style guide).

use std::io::Write;
use std::process::{Command, Stdio};

/// Returns the rendering width in columns. Resolution order:
/// 1. `INSPECT_HELP_WIDTH` (numeric, clamped to `[40, 200]`).
/// 2. `COLUMNS` (numeric, same clamp).
/// 3. Detected terminal width via `tput cols` (best-effort, never blocks long).
/// 4. Fallback `80`.
//
// HP-0 ships verbatim topic content (no wrapping), but the renderer
// publishes the width contract now so HP-6 can adopt it without a
// signature change. The width is also surfaced via `--json` (HP-4).
#[allow(dead_code)]
pub fn render_width() -> usize {
    if let Some(w) = env_usize("INSPECT_HELP_WIDTH") {
        return w.clamp(40, 200);
    }
    if let Some(w) = env_usize("COLUMNS") {
        return w.clamp(40, 200);
    }
    if let Some(w) = tput_cols() {
        return w.clamp(40, 200);
    }
    80
}

#[allow(dead_code)]
fn env_usize(name: &str) -> Option<usize> {
    std::env::var(name).ok().and_then(|s| s.trim().parse().ok())
}

#[allow(dead_code)]
fn tput_cols() -> Option<usize> {
    // Spawn `tput cols` only when there's a tty on stderr; without a
    // controlling terminal `tput` returns "no value for $TERM".
    use std::io::IsTerminal;
    if !std::io::stderr().is_terminal() {
        return None;
    }
    let out = Command::new("tput")
        .arg("cols")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    std::str::from_utf8(&out.stdout).ok()?.trim().parse().ok()
}

/// Returns true when ANSI color output is allowed.
#[allow(dead_code)] // HP-6 will consume this when adding highlighting.
pub fn color_enabled() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if std::env::var_os("INSPECT_NO_COLOR").is_some() {
        return false;
    }
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}

/// Returns true when the renderer should attempt to spawn a pager.
/// Disabled when stdout is not a tty, when `INSPECT_HELP_NO_PAGER` is
/// set, when `PAGER` is the empty string, or when running in CI
/// (`CI=true` is a near-universal convention).
pub fn pager_enabled() -> bool {
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        return false;
    }
    if std::env::var_os("INSPECT_HELP_NO_PAGER").is_some() {
        return false;
    }
    if matches!(std::env::var("PAGER"), Ok(ref s) if s.is_empty()) {
        return false;
    }
    if matches!(std::env::var("CI").as_deref(), Ok("true" | "1")) {
        return false;
    }
    true
}

/// Resolves the pager command line. Honors `PAGER`; falls back to
/// `less -FRX` (quit-if-one-screen, raw color, no init) then `more`.
fn resolve_pager() -> Option<(String, Vec<String>)> {
    if let Ok(raw) = std::env::var("PAGER") {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        // Naive split on whitespace is sufficient for `PAGER` values in
        // practice (`less -R`, `most -s`); shells that need quoting
        // already wrap the value in their own invocation.
        let mut parts = raw.split_whitespace();
        let bin = parts.next()?.to_string();
        let args: Vec<String> = parts.map(String::from).collect();
        return Some((bin, args));
    }
    if which("less").is_some() {
        return Some((
            "less".into(),
            vec!["-F".into(), "-R".into(), "-X".into()],
        ));
    }
    if which("more").is_some() {
        return Some(("more".into(), vec![]));
    }
    None
}

fn which(bin: &str) -> Option<()> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if dir.join(bin).is_file() {
            return Some(());
        }
    }
    None
}

/// Write `text` to stdout, optionally through a pager.
///
/// Always returns `Ok(())` for the trailing-newline flush so a closed
/// pager pipe (user pressed `q`) is not surfaced as a CLI error.
pub fn write_paged(text: &str) -> std::io::Result<()> {
    if pager_enabled() {
        if let Some((bin, args)) = resolve_pager() {
            return spawn_pager(&bin, &args, text);
        }
    }
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    // EPIPE on `inspect help quickstart | head` is benign — swallow it.
    if let Err(e) = lock.write_all(text.as_bytes()) {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            return Ok(());
        }
        return Err(e);
    }
    if !text.ends_with('\n') {
        let _ = lock.write_all(b"\n");
    }
    Ok(())
}

fn spawn_pager(bin: &str, args: &[String], text: &str) -> std::io::Result<()> {
    let mut child = match Command::new(bin)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => {
            // Pager binary disappeared between `which` and `spawn`;
            // gracefully degrade to direct stdout.
            return write_direct(text);
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(text.as_bytes()) {
            if e.kind() != std::io::ErrorKind::BrokenPipe {
                let _ = child.wait();
                return Err(e);
            }
        }
        // Drop stdin to signal EOF before we wait on the pager.
        drop(stdin);
    }
    let _ = child.wait();
    Ok(())
}

fn write_direct(text: &str) -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    match lock.write_all(text.as_bytes()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_width_respects_env_override() {
        // Use a value far outside the default to make the assertion robust
        // even if some other test in the same process set COLUMNS.
        let prev = std::env::var("INSPECT_HELP_WIDTH").ok();
        std::env::set_var("INSPECT_HELP_WIDTH", "73");
        assert_eq!(render_width(), 73);
        match prev {
            Some(v) => std::env::set_var("INSPECT_HELP_WIDTH", v),
            None => std::env::remove_var("INSPECT_HELP_WIDTH"),
        }
    }

    #[test]
    fn render_width_clamps_extremes() {
        let prev = std::env::var("INSPECT_HELP_WIDTH").ok();
        std::env::set_var("INSPECT_HELP_WIDTH", "5");
        assert_eq!(render_width(), 40);
        std::env::set_var("INSPECT_HELP_WIDTH", "9999");
        assert_eq!(render_width(), 200);
        match prev {
            Some(v) => std::env::set_var("INSPECT_HELP_WIDTH", v),
            None => std::env::remove_var("INSPECT_HELP_WIDTH"),
        }
    }

    #[test]
    fn pager_disabled_in_ci() {
        let prev = std::env::var("CI").ok();
        std::env::set_var("CI", "true");
        assert!(!pager_enabled());
        match prev {
            Some(v) => std::env::set_var("CI", v),
            None => std::env::remove_var("CI"),
        }
    }
}
