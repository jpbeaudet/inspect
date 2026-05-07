//! Per-line filter helper for streaming JSON output.
//!
//! Per-frame mode evaluates the cached filter against each input
//! line and emits results as the stream progresses (memory is
//! O(1) in stream length). Slurp mode buffers every line into a
//! single array and evaluates once at end-of-stream (memory is
//! O(stream)) — same trade-off jq makes between `jq` and `jq -s`.

use serde_json::Value;

use super::{
    compile, eval_compiled, eval_slurp_compiled, render_compact, render_raw, Compiled, QueryError,
};

#[derive(Debug)]
pub struct Filter {
    compiled: Compiled,
    raw: bool,
    slurp: Option<Vec<Value>>,
}

impl Filter {
    pub fn new(filter: &str, raw: bool, slurp: bool) -> Result<Self, QueryError> {
        Ok(Self {
            compiled: compile(filter)?,
            raw,
            slurp: if slurp { Some(Vec::new()) } else { None },
        })
    }

    pub fn on_line(&mut self, line: &Value) -> Result<String, QueryError> {
        if let Some(buf) = self.slurp.as_mut() {
            buf.push(line.clone());
            return Ok(String::new());
        }
        let values = eval_compiled(&self.compiled, line)?;
        if self.raw {
            render_raw(&values)
        } else {
            Ok(render_compact(&values))
        }
    }

    /// End-of-stream hook. Per-frame mode returns an empty string
    /// (nothing buffered); slurp mode evaluates the filter against
    /// the buffered array and returns the rendered output. The slurp
    /// buffer is taken so a second call after the first is a no-op
    /// (idempotent), which keeps the streaming wrappers in
    /// `JsonOut::flush_filter` from having to track "did we flush
    /// already" state.
    pub fn finish(&mut self) -> Result<String, QueryError> {
        let Some(buf) = self.slurp.take() else {
            return Ok(String::new());
        };
        let values = eval_slurp_compiled(&self.compiled, &buf)?;
        if self.raw {
            render_raw(&values)
        } else {
            Ok(render_compact(&values))
        }
    }
}
