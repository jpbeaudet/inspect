//! Hand-rolled progress spinner (P9, v0.1.1).
//!
//! Used to give live feedback during slow log/grep/search fetches when
//! the operator runs in a TTY. We deliberately avoid `indicatif` to
//! keep the dep tree tight; the spinner is ~30 lines of plain stdlib.
//!
//! ## Behaviour
//!
//! `with_progress(label, f)` runs `f`. If `f` is still running 700 ms
//! after entry, a side thread starts redrawing
//! `\r{label} {frame}` to **stderr** every 100 ms. When `f` returns,
//! the side thread is signalled, the line is erased with
//! `\r<spaces>\r`, and `f`'s value is returned.
//!
//! ## When the spinner is suppressed
//!
//! - stderr is not a terminal (`IsTerminal::is_terminal()` is false)
//! - `INSPECT_NO_PROGRESS=1` is set in the env (used by the acceptance
//!   tests and any CI/automation invocation)
//! - the calling verb has selected a JSON output (the caller must pass
//!   `enabled=false` in that case)
//!
//! All three checks happen inside `should_show()` so callers don't
//! have to think about it.

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const START_DELAY: Duration = Duration::from_millis(700);
const FRAME_DELAY: Duration = Duration::from_millis(100);

/// Returns true iff a spinner should be drawn for the current
/// invocation. Callers must additionally pass `enabled=false` when in
/// JSON mode.
pub fn should_show(enabled: bool) -> bool {
    if !enabled {
        return false;
    }
    if std::env::var_os("INSPECT_NO_PROGRESS").is_some() {
        return false;
    }
    std::io::stderr().is_terminal()
}

/// Run `f` while optionally drawing a spinner labelled `label` to
/// stderr. The spinner only appears after `START_DELAY` so fast
/// commands stay quiet.
pub fn with_progress<T>(label: &str, enabled: bool, f: impl FnOnce() -> T) -> T {
    if !should_show(enabled) {
        return f();
    }
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = Arc::clone(&stop);
    let label_owned = label.to_string();
    let handle = thread::spawn(move || {
        thread::sleep(START_DELAY);
        if stop_for_thread.load(Ordering::Relaxed) {
            return;
        }
        let mut frame = 0usize;
        let mut drew = false;
        while !stop_for_thread.load(Ordering::Relaxed) {
            let mut err = std::io::stderr().lock();
            let _ = write!(err, "\r{} {}", label_owned, FRAMES[frame % FRAMES.len()]);
            let _ = err.flush();
            drew = true;
            drop(err);
            frame = frame.wrapping_add(1);
            thread::sleep(FRAME_DELAY);
        }
        if drew {
            // Erase the line on exit so the next stderr write starts
            // clean.
            let mut err = std::io::stderr().lock();
            let pad = " ".repeat(label_owned.len() + 4);
            let _ = write!(err, "\r{pad}\r");
            let _ = err.flush();
        }
    });
    let out = f();
    stop.store(true, Ordering::Relaxed);
    let _ = handle.join();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_show_when_disabled() {
        assert!(!should_show(false));
    }

    #[test]
    fn no_show_when_env_var_set() {
        // Save/restore the env var around the test so we don't leak
        // state into sibling tests running on the same process.
        let prev = std::env::var_os("INSPECT_NO_PROGRESS");
        // SAFETY: tests in this module are not run in parallel with
        // any other test that mutates the same env var.
        unsafe {
            std::env::set_var("INSPECT_NO_PROGRESS", "1");
        }
        assert!(!should_show(true));
        unsafe {
            match prev {
                Some(v) => std::env::set_var("INSPECT_NO_PROGRESS", v),
                None => std::env::remove_var("INSPECT_NO_PROGRESS"),
            }
        }
    }

    #[test]
    fn with_progress_returns_inner_value_quickly() {
        let v = with_progress("scanning", false, || 42_u32);
        assert_eq!(v, 42);
    }
}
