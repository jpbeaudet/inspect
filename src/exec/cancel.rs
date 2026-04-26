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

use std::sync::atomic::{AtomicBool, Ordering};

static CANCELLED: AtomicBool = AtomicBool::new(false);

/// Has the user asked us to stop?
#[inline]
pub fn is_cancelled() -> bool {
    CANCELLED.load(Ordering::Relaxed)
}

/// Manually trip the cancel flag. Test-only.
#[cfg(test)]
pub fn cancel() {
    CANCELLED.store(true, Ordering::Relaxed);
}

/// Reset the flag. Test-only.
#[cfg(test)]
pub fn reset() {
    CANCELLED.store(false, Ordering::Relaxed);
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
    // Async-signal-safe: a relaxed atomic store on a primitive integer.
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
}
