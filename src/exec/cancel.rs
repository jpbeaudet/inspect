//! Process-wide cancellation flag for SIGINT / SIGTERM (audit §2.2, §5.4).
//!
//! `inspect` is a synchronous batch tool today (no Tokio runtime), so
//! the right shape for cancellation is a single global `AtomicBool`
//! that:
//!
//! * is set from a real `sigaction` handler on Unix (async-signal-safe
//!   — `AtomicBool::store(Relaxed)` is on every reasonable target);
//! * is polled by every long-running loop in the engine, the SSH
//!   timeout poller, and the parallel branch worker;
//! * lets the engine return *partial* results so the renderer can
//!   still emit a SUMMARY/DATA/NEXT envelope marked `cancelled`,
//!   instead of the script-consumer seeing a truncated stream with
//!   no terminator.
//!
//! `SA_RESTART` is intentionally **not** set: we want syscalls (notably
//! `wait`/`read` on child SSH processes) to return `EINTR` so the
//! polling loops notice the cancel quickly and reap children.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

static CANCELLED: AtomicBool = AtomicBool::new(false);
/// F16-followup (v0.1.3): monotonically increasing count of SIGINT /
/// SIGTERM signals received. The streaming SSH executor uses this to
/// distinguish "first Ctrl-C" (forward through PTY → SIGINT to remote
/// process group) from "second Ctrl-C within 1s" (escalate to SSH
/// channel close → SIGHUP on remote process group). Plain
/// `is_cancelled()` is a one-way trip and cannot answer "did a NEW
/// signal arrive since I last checked" — the counter does.
static SIGNAL_COUNT: AtomicU32 = AtomicU32::new(0);

/// Has the user asked us to stop?
#[inline]
pub fn is_cancelled() -> bool {
    CANCELLED.load(Ordering::Relaxed)
}

/// F16-followup (v0.1.3): monotonically increasing count of cancel
/// signals received. Wraps at `u32::MAX` (~ 4 billion Ctrl-Cs — not a
/// realistic concern). Used by the streaming SSH executor to detect
/// "a new SIGINT arrived since my last poll" without losing the trip
/// to `is_cancelled()` race conditions.
#[inline]
pub fn signal_count() -> u32 {
    SIGNAL_COUNT.load(Ordering::Relaxed)
}

/// Manually trip the cancel flag (and bump the counter). Test-only.
/// Production code never calls this; the SIGINT/SIGTERM handler does
/// the same work via the `extern "C"` path.
#[cfg(test)]
pub fn cancel() {
    CANCELLED.store(true, Ordering::Relaxed);
    SIGNAL_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// F16-followup (v0.1.3): clear the cancellation flag without resetting
/// the signal counter. Used by the streaming SSH executor after it has
/// successfully forwarded a first Ctrl-C through the remote PTY — the
/// remote process is now responsible for terminating, and the local
/// `inspect` should resume reading the stream until either the remote
/// exits cleanly (in which case we want to surface the remote's exit
/// code, NOT exit 130 from the cancellation flag) or a second Ctrl-C
/// arrives (which the counter still detects via the unchanged
/// SIGNAL_COUNT). Test code uses this too via `reset_for_test`.
pub fn reset_cancel_flag() {
    CANCELLED.store(false, Ordering::Relaxed);
}

/// Reset both the flag AND the counter. Test-only — production code
/// must not zero the counter mid-run because doing so would lose the
/// "I've already forwarded one Ctrl-C, the next one escalates"
/// progress in the streaming executor.
#[cfg(test)]
pub fn reset() {
    CANCELLED.store(false, Ordering::Relaxed);
    SIGNAL_COUNT.store(0, Ordering::Relaxed);
}

/// Convenience: return `Err(anyhow!("cancelled by signal"))` if the
/// flag is set. Intended for sprinkling at safe checkpoints.
pub fn check() -> anyhow::Result<()> {
    if is_cancelled() {
        Err(anyhow::anyhow!("cancelled by signal"))
    } else {
        Ok(())
    }
}

/// Install SIGINT/SIGTERM handlers. Safe to call repeatedly — only the
/// first call wins. On non-Unix platforms this is a no-op.
pub fn install_handlers() {
    install_unix();
}

#[cfg(unix)]
fn install_unix() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // SAFETY: `sigaction` is the documented way to install a
        // signal handler. `handler` is `extern "C"` and only stores to
        // a static `AtomicBool`, which is async-signal-safe.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = handler as *const () as usize;
            // Empty mask: don't block other signals during handler.
            libc::sigemptyset(&mut sa.sa_mask);
            // No SA_RESTART: we *want* EINTR so polling loops wake up.
            sa.sa_flags = 0;
            libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
            libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut());
        }
    });
}

#[cfg(not(unix))]
fn install_unix() {}

#[cfg(unix)]
extern "C" fn handler(_sig: libc::c_int) {
    // Async-signal-safe: relaxed atomic ops on primitive integers are
    // explicitly permitted from signal handlers. The counter feeds the
    // streaming executor's first-vs-second-Ctrl-C escalation
    // (F16-followup v0.1.3); `CANCELLED` feeds the conventional
    // "have we been signaled at all" polling.
    SIGNAL_COUNT.fetch_add(1, Ordering::Relaxed);
    CANCELLED.store(true, Ordering::Relaxed);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// All tests in the codebase that mutate the process-global cancel
    /// flag must take this mutex, including the ones in
    /// `crate::ssh::concurrency::tests`. Without it, parallel test
    /// runs trip each other's wait loops.
    pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, OnceLock};
        static M: OnceLock<Mutex<()>> = OnceLock::new();
        M.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    #[test]
    fn flag_round_trip() {
        let _g = test_lock();
        reset();
        assert!(!is_cancelled());
        cancel();
        assert!(is_cancelled());
        reset();
        assert!(!is_cancelled());
    }

    #[test]
    fn check_returns_err_when_cancelled() {
        let _g = test_lock();
        reset();
        assert!(check().is_ok());
        cancel();
        assert!(check().is_err());
        reset();
    }

    /// F16-followup (v0.1.3): the SIGINT counter increments on every
    /// trip — distinguishes "first Ctrl-C" from "second Ctrl-C" so the
    /// streaming executor can escalate from PTY-forwarded SIGINT to
    /// channel-close SIGHUP on the second hit.
    #[test]
    fn signal_count_increments_per_cancel() {
        let _g = test_lock();
        reset();
        let baseline = signal_count();
        assert_eq!(baseline, 0);
        cancel();
        assert_eq!(signal_count(), 1);
        cancel();
        assert_eq!(signal_count(), 2);
        // reset_cancel_flag() must NOT zero the counter — the
        // streaming executor relies on `signal_count() > prev` to
        // detect a *new* signal between polls.
        reset_cancel_flag();
        assert_eq!(signal_count(), 2);
        assert!(!is_cancelled());
        cancel();
        assert_eq!(signal_count(), 3);
        assert!(is_cancelled());
        reset();
    }

    #[test]
    fn reset_cancel_flag_clears_only_the_flag() {
        let _g = test_lock();
        reset();
        cancel();
        assert!(is_cancelled());
        let count_before = signal_count();
        assert_eq!(count_before, 1);
        reset_cancel_flag();
        // Flag cleared, counter preserved.
        assert!(!is_cancelled());
        assert_eq!(signal_count(), count_before);
        reset();
    }
}
