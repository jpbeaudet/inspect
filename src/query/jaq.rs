//! The only file that names `jaq_*` types directly.
//!
//! Round-trip strategy: `serde_json::Value` ↔ `jaq_json::Val` via
//! JSON text. jaq-json does not implement `Serialize` (only
//! `Deserialize` behind a feature flag), so a JSON-text bridge is
//! the most robust path that doesn't require us to walk jaq's
//! internal representation. Performance is fine for our workload —
//! envelopes are small and per-line streaming filters parse in
//! microseconds.

use jaq_core::data::JustLut;
use jaq_core::load::{Arena, File, Loader};
use jaq_core::{Compiler, Ctx, Vars};
use jaq_json::Val;
use serde_json::Value;

use super::QueryError;

/// Pre-compiled filter, parsed exactly once.
///
/// `jaq_core::Compiler::compile` returns `Filter<F>` with no
/// lifetime parameter, so the compiled value is owned and can
/// outlive the parse-time `Arena`. We `Box::leak` the `Arena` to
/// satisfy the `'a` bound during compilation; the leak is bounded
/// by one small `Arena` (a few KB of interned strings) per
/// `compile()` call, and `inspect` is a one-shot CLI binary, so
/// the leaked memory is reclaimed by the OS on process exit.
pub struct Compiled {
    inner: jaq_core::compile::Filter<jaq_core::Native<JustLut<Val>>>,
}

impl std::fmt::Debug for Compiled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Compiled").finish_non_exhaustive()
    }
}

pub fn compile(filter: &str) -> Result<Compiled, QueryError> {
    let arena: &'static Arena = Box::leak(Box::new(Arena::default()));
    let inner = parse_and_compile(filter, arena)?;
    Ok(Compiled { inner })
}

pub fn eval(filter: &str, input: &Value) -> Result<Vec<Value>, QueryError> {
    let arena = Arena::default();
    let compiled = parse_and_compile(filter, &arena)?;
    let val = json_value_to_val(input)?;
    run_filter(&compiled, val)
}

pub fn eval_slurp(filter: &str, inputs: &[Value]) -> Result<Vec<Value>, QueryError> {
    let arr = Value::Array(inputs.to_vec());
    eval(filter, &arr)
}

pub fn eval_compiled(c: &Compiled, input: &Value) -> Result<Vec<Value>, QueryError> {
    let val = json_value_to_val(input)?;
    run_filter(&c.inner, val)
}

pub fn eval_slurp_compiled(c: &Compiled, inputs: &[Value]) -> Result<Vec<Value>, QueryError> {
    let arr = Value::Array(inputs.to_vec());
    eval_compiled(c, &arr)
}

fn parse_and_compile<'a>(
    filter: &'a str,
    arena: &'a Arena,
) -> Result<jaq_core::compile::Filter<jaq_core::Native<JustLut<Val>>>, QueryError> {
    let defs = jaq_core::defs()
        .chain(jaq_std::defs())
        .chain(jaq_json::defs());
    let funs = jaq_core::funs::<JustLut<Val>>()
        .chain(jaq_std::funs::<JustLut<Val>>())
        .chain(jaq_json::funs::<JustLut<Val>>());

    let loader = Loader::new(defs);
    let program = File {
        code: filter,
        path: (),
    };

    let modules = loader
        .load(arena, program)
        .map_err(|errs| QueryError::parse(format_load_errors(&errs)))?;

    Compiler::default()
        .with_funs(funs)
        .compile(modules)
        .map_err(|errs| QueryError::parse(format_compile_errors(&errs)))
}

fn run_filter(
    compiled: &jaq_core::compile::Filter<jaq_core::Native<JustLut<Val>>>,
    input: Val,
) -> Result<Vec<Value>, QueryError> {
    let ctx = Ctx::<JustLut<Val>>::new(&compiled.lut, Vars::new([]));
    let outs = compiled.id.run((ctx, input));

    let mut results = Vec::new();
    for v in outs.map(jaq_core::unwrap_valr) {
        match v {
            Ok(val) => results.push(val_to_json_value(&val)?),
            Err(e) => return Err(QueryError::runtime(format_runtime_error(&e))),
        }
    }
    Ok(results)
}

fn json_value_to_val(input: &Value) -> Result<Val, QueryError> {
    let bytes = serde_json::to_vec(input)
        .map_err(|e| QueryError::runtime(format!("internal: input serialization failed: {e}")))?;
    jaq_json::read::parse_single(&bytes)
        .map_err(|e| QueryError::runtime(format!("internal: jaq input parse failed: {e}")))
}

/// jaq-json's `Val` Display emits a JSON-superset format that
/// includes a few non-JSON shapes (raw byte strings, non-string
/// object keys). For filters that operate on standard JSON inputs
/// the output is plain JSON; we surface the parse failure as a
/// runtime error if a filter introduces something we can't round-
/// trip back through `serde_json::from_str`.
fn val_to_json_value(val: &Val) -> Result<Value, QueryError> {
    let text = format!("{}", val);
    serde_json::from_str::<Value>(&text).map_err(|e| {
        QueryError::runtime(format!(
            "filter result is not representable as JSON ({e}); \
             jaq supersets like raw byte strings are unsupported here"
        ))
    })
}

fn format_load_errors<P>(errs: &jaq_core::load::Errors<&str, P>) -> String {
    let mut out = String::new();
    for (_file, err) in errs {
        if !out.is_empty() {
            out.push_str("; ");
        }
        match err {
            jaq_core::load::Error::Io(items) => {
                for (s, msg) in items {
                    out.push_str(&format!("io error near '{s}': {msg}"));
                }
            }
            jaq_core::load::Error::Lex(items) => {
                for (expect, near) in items {
                    out.push_str(&format!(
                        "lex error: expected {} near '{}'",
                        expect.as_str(),
                        truncate(near, 24)
                    ));
                }
            }
            jaq_core::load::Error::Parse(items) => {
                for (expect, near) in items {
                    let near_str = if near.is_empty() {
                        "<eof>"
                    } else {
                        truncate(near, 24)
                    };
                    out.push_str(&format!(
                        "parse error: expected {} near '{near_str}'",
                        expect.as_str()
                    ));
                }
            }
        }
    }
    if out.is_empty() {
        "filter failed to load".to_string()
    } else {
        out
    }
}

fn format_compile_errors<P>(errs: &jaq_core::compile::Errors<&str, P>) -> String {
    let mut out = String::new();
    for (_file, items) in errs {
        for err in items {
            if !out.is_empty() {
                out.push_str("; ");
            }
            out.push_str(&format!("compile error: {err:?}"));
        }
    }
    if out.is_empty() {
        "filter failed to compile".to_string()
    } else {
        out
    }
}

fn format_runtime_error(err: &jaq_core::Error<Val>) -> String {
    format!("{err}")
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        let mut idx = max;
        while idx > 0 && !s.is_char_boundary(idx) {
            idx -= 1;
        }
        &s[..idx]
    }
}
