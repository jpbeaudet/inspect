//! jq filter engine kept behind a single abstraction so the
//! underlying library can be swapped (jaq → libjq → handwritten
//! subset → other) without touching call sites — only the four
//! files in this module name `jaq_*` types directly.

use std::fmt;

mod jaq;
pub mod ndjson;
mod raw;

#[cfg(test)]
mod tests;

pub use jaq::{compile, eval, eval_compiled, eval_slurp, eval_slurp_compiled, Compiled};
pub use raw::{render_compact, render_raw};

/// `kind` determines exit-code mapping at the verb-output layer:
/// `Parse` → 2 (clap-class usage error), `Runtime` and
/// `RawNonString` → 1 (no-match class).
#[derive(Debug)]
pub struct QueryError {
    pub kind: QueryErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryErrorKind {
    Parse,
    Runtime,
    RawNonString,
}

impl QueryError {
    pub(crate) fn parse(message: impl Into<String>) -> Self {
        Self {
            kind: QueryErrorKind::Parse,
            message: message.into(),
        }
    }

    pub(crate) fn runtime(message: impl Into<String>) -> Self {
        Self {
            kind: QueryErrorKind::Runtime,
            message: message.into(),
        }
    }

    pub(crate) fn raw_non_string(index: usize, kind_name: &str) -> Self {
        Self {
            kind: QueryErrorKind::RawNonString,
            message: format!(
                "filter yielded non-string at result {index} (got {kind_name}); \
                 remove --raw / --select-raw or wrap with `tostring`"
            ),
        }
    }
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for QueryError {}
