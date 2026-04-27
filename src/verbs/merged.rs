//! `--merged` multi-container log view (P5, v0.1.1).
//!
//! When a `inspect logs` selector matches multiple services and the user
//! passes `--merged`, we render a single interleaved stream prefixed by
//! `[svc]` rather than one block per service.
//!
//! Two execution modes:
//! - **Batch** (no `--follow`): every selected step's `docker logs` (or
//!   `journalctl`) is invoked in parallel via `std::thread::scope`. We
//!   require `--timestamps` on docker logs so each line carries an
//!   RFC3339 prefix; lines are pushed into a `BinaryHeap<Reverse<...>>`
//!   keyed on the timestamp, then printed in sorted order. Lines that
//!   fail to parse a timestamp fall back to the per-step arrival index
//!   so we never drop content.
//! - **Follow**: each step streams via `runner.run_streaming` on its own
//!   scoped thread. Lines flow through a single `std::sync::mpsc` channel
//!   and the main thread prints them in observed order. Clock-skew
//!   between hosts is documented in the help body; the alternative
//!   (buffered window-merge) would cap the live feel of `--follow`.
//!
//! Field-pitfall driver: P5 in `INSPECT_v0.1.1_PATCH_SPEC.md`. Operators
//! correlating across multiple services lost mental context juggling
//! N terminal panes; the merged view restores chronology.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::ssh::exec::RunOpts;
use crate::verbs::output::{Envelope, JsonOut};
use crate::verbs::runtime::RemoteRunner;

/// One pre-tagged line ready for k-way merge.
#[derive(Debug, Clone)]
pub struct MergeLine {
    pub ts: Option<DateTime<Utc>>,
    pub svc_idx: usize,
    pub seq: u64,
    pub svc: String,
    pub line: String,
}

/// Parse the leading RFC3339 timestamp emitted by `docker logs --timestamps`.
/// Returns `(timestamp_or_none, remainder)`.
pub fn split_timestamp(raw: &str) -> (Option<DateTime<Utc>>, &str) {
    // docker logs --timestamps emits e.g.:
    //   "2024-01-01T00:00:00.123456789Z hello\n"
    // The first token (up to the first ASCII space) is the timestamp.
    let space = match raw.find(' ') {
        Some(i) => i,
        None => return (None, raw),
    };
    let (head, tail) = raw.split_at(space);
    match DateTime::parse_from_rfc3339(head) {
        Ok(dt) => (Some(dt.with_timezone(&Utc)), &tail[1..]),
        Err(_) => (None, raw),
    }
}

/// Deterministic ordering: timestamp ascending; within an identical
/// timestamp the source service index breaks ties; finally the per-source
/// sequence number. Lines without a parseable timestamp sink to the
/// bottom of the heap so they always print after every dated line — but
/// still preserve their per-source order.
fn order_key(m: &MergeLine) -> (DateTime<Utc>, usize, u64) {
    // Undated lines sort to the end by mapping `None` to MAX_UTC.
    let ts = m.ts.unwrap_or(DateTime::<Utc>::MAX_UTC);
    (ts, m.svc_idx, m.seq)
}

/// K-way merge a batch of pre-collected per-source line buffers.
/// Sources are consumed in-place (drained into the heap) and rendered
/// via `print` in order.
pub fn k_way_merge(buffers: Vec<Vec<MergeLine>>, mut print: impl FnMut(&MergeLine)) {
    let mut heap: BinaryHeap<Reverse<MergeLine>> = BinaryHeap::new();
    // Move every line into the heap.
    for buf in buffers.into_iter() {
        for m in buf {
            heap.push(Reverse(m));
        }
    }
    while let Some(Reverse(m)) = heap.pop() {
        print(&m);
    }
}

impl PartialEq for MergeLine {
    fn eq(&self, other: &Self) -> bool {
        order_key(self) == order_key(other)
    }
}
impl Eq for MergeLine {}
impl PartialOrd for MergeLine {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for MergeLine {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        order_key(self).cmp(&order_key(other))
    }
}

/// One source the merger needs: per-step namespace/svc/cmd tuple. We do
/// not borrow the [`crate::verbs::dispatch::Step`] directly so the
/// merger module stays loosely coupled and unit-testable.
pub struct MergeSource<'a> {
    pub namespace: &'a str,
    pub target: &'a crate::ssh::options::SshTarget,
    pub svc: String,
    pub cmd: String,
}

/// Run every source in parallel, collect per-source buffers, k-way
/// merge by RFC3339 timestamp, and emit each line via `emit`. Returns
/// the total number of lines produced.
pub fn batch_merged(
    runner: &(dyn RemoteRunner + Send + Sync),
    sources: &[MergeSource<'_>],
    timeout_secs: u64,
    mut emit: impl FnMut(&MergeLine),
) -> Result<usize> {
    // Collect per-source results in the same order as `sources` so the
    // svc_idx tie-break is stable.
    let bufs: Vec<Result<Vec<MergeLine>>> = std::thread::scope(|s| {
        let handles: Vec<_> = sources
            .iter()
            .enumerate()
            .map(|(idx, src)| {
                s.spawn(move || -> Result<Vec<MergeLine>> {
                    let opts = RunOpts::with_timeout(timeout_secs);
                    let out = runner.run(src.namespace, src.target, &src.cmd, opts)?;
                    let mut buf = Vec::new();
                    for (seq, raw) in (0_u64..).zip(out.stdout.lines()) {
                        let (ts, body) = split_timestamp(raw);
                        buf.push(MergeLine {
                            ts,
                            svc_idx: idx,
                            seq,
                            svc: src.svc.clone(),
                            line: body.to_string(),
                        });
                    }
                    Ok(buf)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("merger thread panicked"))
            .collect()
    });

    let mut total = 0usize;
    let mut all = Vec::with_capacity(bufs.len());
    for r in bufs {
        let v = r?;
        total += v.len();
        all.push(v);
    }
    k_way_merge(all, |m| emit(m));
    Ok(total)
}

/// Follow-mode merge: each source streams in its own thread; lines
/// flow through a single mpsc channel; the main thread prints in
/// arrival order. Returns when every source's `run_streaming` returns
/// (or on Ctrl-C cancellation).
pub fn follow_merged(
    runner: &(dyn RemoteRunner + Send + Sync),
    sources: &[MergeSource<'_>],
    timeout_secs: u64,
    mut emit: impl FnMut(&MergeLine),
) -> Result<usize> {
    let (tx, rx) = mpsc::channel::<MergeLine>();
    let total_received = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for (idx, src) in sources.iter().enumerate() {
            let tx = tx.clone();
            s.spawn(move || {
                let opts = RunOpts::with_timeout(timeout_secs);
                let mut seq: u64 = 0;
                let svc = src.svc.clone();
                let _ =
                    runner.run_streaming(src.namespace, src.target, &src.cmd, opts, &mut |line| {
                        if crate::exec::cancel::is_cancelled() {
                            return;
                        }
                        let (ts, body) = split_timestamp(line);
                        let m = MergeLine {
                            ts,
                            svc_idx: idx,
                            seq,
                            svc: svc.clone(),
                            line: body.to_string(),
                        };
                        seq += 1;
                        // Receiver gone == main thread aborted; drop.
                        let _ = tx.send(m);
                    });
            });
        }
        // Drop our own clone so the channel closes when the last
        // worker exits.
        drop(tx);

        for m in rx {
            if crate::exec::cancel::is_cancelled() {
                break;
            }
            total_received.fetch_add(1, Ordering::Relaxed);
            emit(&m);
        }
    });
    Ok(total_received.load(Ordering::Relaxed))
}

/// Print one merged line in human format with a `[svc]` prefix. Kept
/// out of the merger so callers can route to JSON instead.
pub fn print_human(prefix_svc: &str, body: &str) {
    let safe =
        crate::format::safe::safe_terminal_line(body, crate::format::safe::DEFAULT_MAX_LINE_BYTES);
    println!("[{prefix_svc}] {safe}");
}

/// JSON-out variant: emit a single envelope keyed on the namespace
/// recorded by the source plus a `svc` field for the merged stream.
pub fn print_json(namespace: &str, svc: &str, body: &str) {
    JsonOut::write(
        &Envelope::new(namespace, "logs", "logs")
            .with_service(svc)
            .put(
                "line",
                crate::format::safe::safe_machine_line(body).as_ref(),
            )
            .put("merged", true),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(ts: Option<&str>, svc_idx: usize, seq: u64, body: &str) -> MergeLine {
        MergeLine {
            ts: ts.map(|s| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)),
            svc_idx,
            seq,
            svc: format!("svc{svc_idx}"),
            line: body.to_string(),
        }
    }

    #[test]
    fn split_timestamp_strips_rfc3339_prefix() {
        let (ts, rest) = split_timestamp("2024-01-01T00:00:00.123456789Z hello world");
        assert!(ts.is_some());
        assert_eq!(rest, "hello world");
    }

    #[test]
    fn split_timestamp_returns_none_for_plain_lines() {
        let (ts, rest) = split_timestamp("plain line of output");
        assert!(ts.is_none());
        assert_eq!(rest, "plain line of output");
    }

    #[test]
    fn k_way_merge_orders_three_streams_by_timestamp() {
        let a = vec![
            line(Some("2024-01-01T00:00:00Z"), 0, 0, "a-1"),
            line(Some("2024-01-01T00:00:05Z"), 0, 1, "a-2"),
        ];
        let b = vec![
            line(Some("2024-01-01T00:00:01Z"), 1, 0, "b-1"),
            line(Some("2024-01-01T00:00:04Z"), 1, 1, "b-2"),
        ];
        let c = vec![line(Some("2024-01-01T00:00:02Z"), 2, 0, "c-1")];
        let mut got: Vec<String> = Vec::new();
        k_way_merge(vec![a, b, c], |m| got.push(m.line.clone()));
        assert_eq!(got, vec!["a-1", "b-1", "c-1", "b-2", "a-2"]);
    }

    #[test]
    fn k_way_merge_preserves_source_order_for_undated_lines() {
        let a = vec![line(None, 0, 0, "a-first"), line(None, 0, 1, "a-second")];
        let b = vec![line(None, 1, 0, "b-first")];
        let mut got: Vec<String> = Vec::new();
        k_way_merge(vec![a, b], |m| got.push(m.line.clone()));
        // No timestamps → svc_idx then seq decides order.
        assert_eq!(got, vec!["a-first", "a-second", "b-first"]);
    }

    #[test]
    fn dated_lines_print_before_undated_ones() {
        let dated = vec![line(Some("2024-01-01T00:00:00Z"), 0, 0, "dated")];
        let undated = vec![line(None, 1, 0, "undated")];
        let mut got: Vec<String> = Vec::new();
        k_way_merge(vec![dated, undated], |m| got.push(m.line.clone()));
        assert_eq!(got, vec!["dated", "undated"]);
    }
}
