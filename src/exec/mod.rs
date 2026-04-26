//! Phase 7: query execution engine.
//!
//! Bible §9.6 / §9.7 / §9.8: this module turns a parsed `Query` (from
//! [`crate::logql`]) into either a stream of records (log query) or a
//! set of metric samples (metric query). Source readers per medium
//! live under [`reader`], pipeline stages under [`pipeline`], the
//! `map` extension under [`map_stage`], and metric aggregations under
//! [`metric`].
//!
//! Streaming is approximated as eager fan-out + concatenate today.
//! Truly incremental streaming with `--follow` is documented as a
//! Phase 8 enhancement (no protocol break — readers will gain
//! `read_stream`).

#![allow(dead_code)]

pub mod cancel;
pub mod engine;
pub mod field_filter;
pub mod format;
pub mod map_stage;
pub mod medium;
pub mod metric;
pub mod parsers;
pub mod pipeline;
pub mod reader;
pub mod record;

pub use engine::{execute, ExecOutput};
#[allow(unused_imports)]
pub use engine::LogResult;
pub use record::Record;

use crate::verbs::runtime::RemoteRunner;

/// User-tunable execution knobs.
#[derive(Debug, Clone)]
pub struct ExecOpts {
    pub since: Option<String>,
    pub until: Option<String>,
    pub tail: Option<usize>,
    pub follow: bool,
    /// Hard cap on records that survive the pipeline (0 = unlimited).
    pub record_limit: usize,
    /// Hard cap on `map` fanout — a runaway sub-query shouldn't fork
    /// thousands of remote calls.
    pub map_max_fanout: usize,
    /// Maximum parallel reader invocations (across selector branches
    /// and per-branch namespace/service steps). Bible §14.3 — needed
    /// to hit the "<2s first results across 5 servers" target.
    pub max_parallel: usize,
}

impl Default for ExecOpts {
    fn default() -> Self {
        Self {
            since: None,
            until: None,
            tail: None,
            follow: false,
            record_limit: 0,
            map_max_fanout: 256,
            max_parallel: default_max_parallel(),
        }
    }
}

fn default_max_parallel() -> usize {
    // Operator override always wins; otherwise we cap at 8 which is
    // safe across SSH ControlMaster fanout without saturating sockets.
    if let Ok(v) = std::env::var("INSPECT_MAX_PARALLEL") {
        if let Ok(n) = v.parse::<usize>() {
            if n > 0 {
                return n;
            }
        }
    }
    8
}

/// Execution context shared by all stages and sub-queries.
pub struct ExecCtx<'a> {
    /// Post-alias-substitution source string.
    pub source: &'a str,
    pub opts: &'a ExecOpts,
    pub runner: &'a dyn RemoteRunner,
}
