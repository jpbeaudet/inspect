//! Field pitfall §4.2 — `RLIMIT_NOFILE` aware concurrency cap.
//!
//! Each remote SSH invocation opens roughly:
//!
//! * one control socket (shared with siblings, but the child still
//!   counts the connect),
//! * one stdin (`/dev/null`),
//! * one stdout pipe,
//! * one stderr pipe.
//!
//! Plus a handful of fds the runtime keeps for itself (logs, audit
//! file, tty, etc.). The math below is intentionally conservative: we
//! reserve a fixed safety margin and divide what's left by the
//! per-step fd budget. The result is a **hard upper bound** for any
//! fan-out parameter (engine `--max-parallel`, fleet `--concurrency`).
//!
//! When `getrlimit` fails (non-Unix host, sandbox, etc.) we return
//! `usize::MAX` so the caller's nominal value wins. This module is
//! deliberately silent at the API level — callers are responsible for
//! warning on stderr when they decide to clamp.

#[cfg(unix)]
use std::mem::MaybeUninit;

/// Estimated open-file descriptors consumed per concurrent remote
/// step (stdin, stdout, stderr, plus a small slack for control-socket
/// re-attach and any reader that opens a temp file).
pub const FDS_PER_STEP: u64 = 4;

/// Fixed reserve held back from `RLIMIT_NOFILE` for the runtime
/// itself (log files, audit log, terminal, threadpool wakers, …).
pub const FD_RUNTIME_RESERVE: u64 = 100;

/// Floor below which we refuse to clamp — even on a tiny ulimit we
/// must let the user run *some* fan-out, otherwise a `ulimit -n 64`
/// shell would silently disable concurrency.
pub const MIN_SAFE_CONCURRENCY: usize = 2;

/// Read the current soft `RLIMIT_NOFILE`. Returns `None` on platforms
/// where the syscall is unavailable or fails.
pub fn nofile_soft_limit() -> Option<u64> {
    #[cfg(unix)]
    unsafe {
        let mut rlim = MaybeUninit::<libc::rlimit>::uninit();
        if libc::getrlimit(libc::RLIMIT_NOFILE, rlim.as_mut_ptr()) != 0 {
            return None;
        }
        Some(rlim.assume_init().rlim_cur as u64)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Compute the safe concurrency upper bound implied by the current
/// `RLIMIT_NOFILE`. Returns `usize::MAX` when the limit cannot be
/// determined (treats "unknown" as "unlimited" — the caller's nominal
/// value wins).
pub fn safe_concurrency_cap() -> usize {
    cap_from_soft_limit(nofile_soft_limit())
}

/// Pure helper for unit tests.
pub fn cap_from_soft_limit(soft: Option<u64>) -> usize {
    let soft = match soft {
        Some(n) => n,
        None => return usize::MAX,
    };
    if soft <= FD_RUNTIME_RESERVE {
        return MIN_SAFE_CONCURRENCY;
    }
    let usable = soft - FD_RUNTIME_RESERVE;
    let cap = usable / FDS_PER_STEP;
    let cap_usize = if cap > usize::MAX as u64 {
        usize::MAX
    } else {
        cap as usize
    };
    cap_usize.max(MIN_SAFE_CONCURRENCY)
}

/// Apply [`safe_concurrency_cap`] to `requested`, writing a single
/// stderr warning when we clamp so the operator can either raise their
/// `ulimit -n` or accept the lower fan-out.
///
/// `label` is a human-readable name for the source of `requested`
/// (e.g. `"INSPECT_FLEET_CONCURRENCY"`, `"--max-parallel"`).
pub fn clamp_with_warning(requested: usize, label: &str) -> usize {
    let cap = safe_concurrency_cap();
    if requested <= cap {
        return requested;
    }
    let soft = nofile_soft_limit().unwrap_or(0);
    eprintln!(
        "warning: {label} ({requested}) exceeds the per-process file-descriptor budget \
         (RLIMIT_NOFILE soft={soft}, ~{FDS_PER_STEP} fds per step, {FD_RUNTIME_RESERVE} reserved); \
         clamped to {cap}. Raise the limit with `ulimit -n <N>` to lift the cap."
    );
    cap
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_limit_is_unlimited() {
        assert_eq!(cap_from_soft_limit(None), usize::MAX);
    }

    #[test]
    fn tiny_limit_yields_floor() {
        // 64 is below the runtime reserve; we still allow a tiny bit
        // of concurrency rather than disabling it entirely.
        assert_eq!(cap_from_soft_limit(Some(64)), MIN_SAFE_CONCURRENCY);
    }

    #[test]
    fn typical_developer_limit() {
        // macOS default of 256: (256 - 100) / 4 = 39.
        assert_eq!(cap_from_soft_limit(Some(256)), 39);
    }

    #[test]
    fn linux_default_limit() {
        // Linux default of 1024: (1024 - 100) / 4 = 231.
        assert_eq!(cap_from_soft_limit(Some(1024)), 231);
    }

    #[test]
    fn very_large_limit() {
        // hardened server with 1M fds: still bounded by usize on the
        // platform but never overflows.
        let cap = cap_from_soft_limit(Some(1_048_576));
        assert!(cap > 100_000);
        assert!(cap < usize::MAX);
    }
}
