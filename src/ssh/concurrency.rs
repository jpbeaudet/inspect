//! Per-host SSH session concurrency limiter (audit §3.3).
//!
//! OpenSSH's default `MaxSessions` is **10** open multiplexed sessions
//! per ControlMaster connection. Inspect's fan-out can easily exceed
//! that on a single host (e.g. `inspect logs '*/_:logs'` against a box
//! running 12 containers). When the limit is reached, OpenSSH replies
//!
//!     channel N: open failed: administratively prohibited: open failed
//!
//! and the sub-command exits non-zero. There is no graceful retry from
//! the server; the client must self-throttle.
//!
//! This module wraps every outbound `ssh` invocation in a per-host
//! semaphore. Default limit is **8** (one short of the OpenSSH default
//! to leave headroom for the user's own ssh session and for inspect's
//! own `-O check` health pings). It is overridable per-process via
//! `INSPECT_MAX_SESSIONS_PER_HOST=<n>`.
//!
//! ## Why a semaphore and not a thread pool?
//!
//! The engine already uses a thread pool (`engine::parallel_map`) for
//! cross-target fan-out. The semaphore is a *cross-cutting* limit that
//! must apply across every code path that opens an SSH channel:
//! pipeline reads, write verbs, health probes, recipes. A per-host
//! semaphore sized below `MaxSessions` is the simplest way to make the
//! limit composable.
//!
//! ## Implementation
//!
//! `std` doesn't ship a counting semaphore, so we use the canonical
//! `Mutex<u32> + Condvar` pattern. The acquire path is:
//!
//! 1. Look up (or create) the per-host slot in the registry.
//! 2. Lock the slot, wait on its condvar while `available == 0`.
//! 3. Decrement `available`, return a `SessionGuard`.
//! 4. Drop releases by re-locking, incrementing, and notifying one waiter.
//!
//! Cancellation: the wait loop checks
//! [`crate::exec::cancel::is_cancelled`] each time it wakes, so SIGINT
//! never deadlocks behind a saturated host.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, OnceLock};

/// Per-process registry of host → slot.
fn registry() -> &'static Mutex<HashMap<String, Arc<HostSlot>>> {
    static REG: OnceLock<Mutex<HashMap<String, Arc<HostSlot>>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Effective limit for a host. Default 8; overridable globally via
/// `INSPECT_MAX_SESSIONS_PER_HOST`. Must be ≥ 1.
fn host_limit() -> u32 {
    if let Ok(s) = std::env::var("INSPECT_MAX_SESSIONS_PER_HOST") {
        if let Ok(n) = s.parse::<u32>() {
            if n >= 1 {
                return n;
            }
        }
    }
    8
}

struct HostSlot {
    state: Mutex<HostState>,
    cv: Condvar,
}

struct HostState {
    /// Remaining capacity; starts at `host_limit()`.
    available: u32,
    /// Cached limit so the registry is constructed once per host.
    limit: u32,
}

/// RAII guard returned by [`acquire`]. Releases the slot on drop.
pub struct SessionGuard {
    slot: Arc<HostSlot>,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        let mut st = match self.slot.state.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        st.available = st.available.saturating_add(1).min(st.limit);
        // Wake exactly one waiter — multi-wake would just trigger
        // spurious wakeups + re-blocking on the inner Mutex.
        self.slot.cv.notify_one();
    }
}

/// Acquire one session slot for `host`, blocking until one is free.
///
/// Returns `Err` only if the global cancel flag was set while waiting,
/// so callers can short-circuit instead of starting an `ssh` child
/// that would immediately be killed.
pub fn acquire(host: &str) -> Result<SessionGuard, anyhow::Error> {
    let slot = get_or_create_slot(host);
    let mut st = match slot.state.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    loop {
        if crate::exec::cancel::is_cancelled() {
            return Err(anyhow::anyhow!("cancelled by signal"));
        }
        if st.available > 0 {
            st.available -= 1;
            drop(st);
            return Ok(SessionGuard { slot });
        }
        // Wait at most 100 ms so we re-check the cancel flag promptly
        // even if no other thread releases a slot.
        let res = match slot
            .cv
            .wait_timeout(st, std::time::Duration::from_millis(100))
        {
            Ok((g, _)) => g,
            Err(p) => p.into_inner().0,
        };
        st = res;
    }
}

fn get_or_create_slot(host: &str) -> Arc<HostSlot> {
    let mut reg = match registry().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if let Some(s) = reg.get(host) {
        return Arc::clone(s);
    }
    let limit = host_limit();
    let slot = Arc::new(HostSlot {
        state: Mutex::new(HostState {
            available: limit,
            limit,
        }),
        cv: Condvar::new(),
    });
    reg.insert(host.to_string(), Arc::clone(&slot));
    slot
}

/// Heuristic: did this stderr come from OpenSSH telling us we hit
/// `MaxSessions`? Used by `run_remote` to upgrade an opaque non-zero
/// exit into an actionable error message.
pub fn looks_like_max_sessions(stderr: &str) -> bool {
    let s = stderr;
    // Three observed phrasings across OpenSSH 7.x – 9.x:
    s.contains("administratively prohibited: open failed")
        || s.contains("MaxSessions")
        || s.contains("open failed: administratively prohibited")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    fn with_limit<F: FnOnce()>(limit: u32, f: F) {
        std::env::set_var("INSPECT_MAX_SESSIONS_PER_HOST", limit.to_string());
        f();
        std::env::remove_var("INSPECT_MAX_SESSIONS_PER_HOST");
    }

    #[test]
    fn semaphore_caps_concurrency() {
        let _g = crate::exec::cancel::tests::test_lock();
        crate::exec::cancel::reset();
        // Use a fresh host name per test so the static registry doesn't
        // see a slot left over from another run with a different limit.
        with_limit(2, || {
            let host = "test-host-cap";
            let in_flight = Arc::new(AtomicUsize::new(0));
            let max = Arc::new(AtomicUsize::new(0));
            let mut handles = Vec::new();
            for _ in 0..8 {
                let in_flight = Arc::clone(&in_flight);
                let max = Arc::clone(&max);
                handles.push(thread::spawn(move || {
                    let _g = acquire(host).expect("acquire");
                    let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    max.fetch_max(now, Ordering::SeqCst);
                    thread::sleep(Duration::from_millis(20));
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                }));
            }
            for h in handles {
                h.join().unwrap();
            }
            // Hard upper bound: never exceed the configured limit.
            assert!(
                max.load(Ordering::SeqCst) <= 2,
                "max concurrency was {}",
                max.load(Ordering::SeqCst)
            );
        });
    }

    #[test]
    fn cancellation_unblocks_waiters() {
        let _g = crate::exec::cancel::tests::test_lock();
        crate::exec::cancel::reset();
        with_limit(1, || {
            let host = "test-host-cancel";
            // Hold the only slot in the foreground so the second
            // acquire is forced to wait.
            let _held = acquire(host).expect("first acquire");
            let h = thread::spawn(move || {
                let start = Instant::now();
                let r = acquire(host);
                (r.is_err(), start.elapsed())
            });
            // Give the waiter time to park.
            thread::sleep(Duration::from_millis(50));
            crate::exec::cancel::cancel();
            let (was_err, elapsed) = h.join().unwrap();
            crate::exec::cancel::reset();
            assert!(was_err, "waiter should return Err on cancel");
            assert!(
                elapsed < Duration::from_millis(500),
                "waiter took {elapsed:?} — cancel didn't propagate"
            );
        });
    }

    #[test]
    fn looks_like_max_sessions_matches_real_phrasings() {
        assert!(looks_like_max_sessions(
            "channel 0: open failed: administratively prohibited: open failed"
        ));
        assert!(looks_like_max_sessions("MaxSessions reached"));
        assert!(!looks_like_max_sessions("connection refused"));
    }
}
