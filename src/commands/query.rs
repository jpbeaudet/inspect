//! `inspect query <FILTER>` — apply a jq-language filter to JSON
//! or NDJSON read from stdin. Lets agents pipe arbitrary JSON
//! through a filter without an external `jq` install.

use std::io::Read;

use anyhow::{anyhow, Result};

use crate::cli::QueryArgs;
use crate::error::ExitKind;
use crate::query::{self, QueryError, QueryErrorKind};
use crate::transcript;

const STDIN_MAX_DEFAULT: usize = 16 * 1024 * 1024;

pub fn run(args: QueryArgs) -> Result<ExitKind> {
    let max = stdin_max();
    let mut buf = String::new();
    read_stdin_capped(&mut buf, max)?;
    if buf.trim().is_empty() {
        return Err(anyhow!(
            "no JSON on stdin\nhint: pipe a `--json` envelope or any JSON document into `inspect query <FILTER>`"
        ));
    }

    let mode = pick_mode(&buf, args.ndjson, args.slurp);
    let result = match mode {
        Mode::Single => run_single(&args.filter, &buf, args.raw),
        Mode::Slurp => run_slurp(&args.filter, &buf, args.raw),
        Mode::Stream => run_stream(&args.filter, &buf, args.raw),
    };

    match result {
        Ok(rendered) => {
            if rendered.is_empty() {
                Ok(ExitKind::NoMatches)
            } else {
                emit(&rendered);
                Ok(ExitKind::Success)
            }
        }
        // Parse errors are usage errors → exit 2 via anyhow.
        Err(e) if e.kind == QueryErrorKind::Parse => Err(anyhow!(
            "filter parse error: {}\nhint: see jq syntax, e.g. `.data.entries[0].id`",
            e.message
        )),
        // Runtime + RawNonString are no-match-class → exit 1; we
        // print the message to stderr and return `NoMatches` rather
        // than bubble through anyhow (which would force exit 2).
        Err(e) => {
            let label = match e.kind {
                QueryErrorKind::Runtime => "filter runtime error",
                QueryErrorKind::RawNonString => "filter --raw",
                QueryErrorKind::Parse => unreachable!("handled above"),
            };
            let line = format!("{}: {}", label, e.message);
            eprintln!("{}", line);
            transcript::tee_stderr(&line);
            Ok(ExitKind::NoMatches)
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Mode {
    Single,
    Slurp,
    Stream,
}

fn pick_mode(stdin: &str, force_ndjson: bool, slurp: bool) -> Mode {
    if slurp {
        return Mode::Slurp;
    }
    if force_ndjson {
        return Mode::Stream;
    }
    if serde_json::from_str::<serde_json::Value>(stdin).is_ok() {
        Mode::Single
    } else {
        Mode::Stream
    }
}

fn run_single(filter: &str, stdin: &str, raw: bool) -> Result<String, QueryError> {
    let value: serde_json::Value = serde_json::from_str(stdin).map_err(|e| {
        QueryError::parse(format!(
            "stdin is not valid JSON ({}); use --ndjson if the input is one value per line",
            e
        ))
    })?;
    let values = query::eval(filter, &value)?;
    if raw {
        query::render_raw(&values)
    } else {
        Ok(query::render_compact(&values))
    }
}

fn run_slurp(filter: &str, stdin: &str, raw: bool) -> Result<String, QueryError> {
    let inputs = parse_ndjson(stdin)?;
    let values = query::eval_slurp(filter, &inputs)?;
    if raw {
        query::render_raw(&values)
    } else {
        Ok(query::render_compact(&values))
    }
}

fn run_stream(filter: &str, stdin: &str, raw: bool) -> Result<String, QueryError> {
    let mut filter = query::ndjson::Filter::new(filter, raw, false)?;
    let mut out = String::new();
    for line in stdin.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(line)
            .map_err(|e| QueryError::parse(format!("ndjson line is not valid JSON ({})", e)))?;
        out.push_str(&filter.on_line(&value)?);
    }
    out.push_str(&filter.finish()?);
    Ok(out)
}

fn parse_ndjson(stdin: &str) -> Result<Vec<serde_json::Value>, QueryError> {
    let mut out = Vec::new();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(stdin) {
        out.push(v);
        return Ok(out);
    }
    for line in stdin.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(line)
            .map_err(|e| QueryError::parse(format!("ndjson line is not valid JSON ({})", e)))?;
        out.push(v);
    }
    Ok(out)
}

fn read_stdin_capped(buf: &mut String, max: usize) -> Result<()> {
    let mut handle = std::io::stdin().lock().take((max as u64) + 1);
    handle
        .read_to_string(buf)
        .map_err(|e| anyhow!("failed to read stdin: {e}"))?;
    if buf.len() > max {
        return Err(anyhow!(
            "stdin exceeded {max} bytes; raise `INSPECT_QUERY_STDIN_MAX` to override"
        ));
    }
    Ok(())
}

fn stdin_max() -> usize {
    match std::env::var("INSPECT_QUERY_STDIN_MAX") {
        Ok(v) => v.parse::<usize>().unwrap_or(STDIN_MAX_DEFAULT),
        Err(_) => STDIN_MAX_DEFAULT,
    }
}

fn emit(rendered: &str) {
    for line in rendered.lines() {
        transcript::emit_stdout(line);
    }
}
