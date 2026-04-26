//! `| map { <sub-query> }` — Splunk-style cross-medium chaining.
//!
//! For each unique combination of `$field$` keys referenced by the
//! sub-query, we substitute the values into the sub-query text and run
//! it. The merged output is the stage result.
//!
//! Bible §9.8: "$field$ is not a shell variable. ... `inspect search`
//! queries are always single-quoted."

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};

use crate::exec::record::Record;
use crate::exec::ExecCtx;
use crate::logql::ast::LogQuery;

/// Execute the `map` stage by:
///   1. Re-rendering the sub-query AST back to text (so `$field$`
///      tokens originally captured in label values survive).
///   2. For each unique tuple of referenced fields in the parent
///      stream, substitute values and parse + execute the sub-query.
///   3. Concatenate the outputs (order matches parent stream order
///      of unique tuples).
pub fn execute(
    ctx: &ExecCtx<'_>,
    sub: &LogQuery,
    parent: Vec<Record>,
) -> Result<Vec<Record>> {
    // We work off the original sub-query text rather than rebuilding
    // it from the AST: the parser preserves the byte span, and the
    // post-alias-substitution source is on `ctx.source`.
    let sub_src = &ctx.source[sub.span.clone()];

    let referenced = collect_field_refs(sub_src);
    if referenced.is_empty() {
        // No interpolation: run the sub-query exactly once.
        return crate::exec::engine::execute_log(ctx, sub_src).map(|r| r.records);
    }

    // Build distinct tuples of (field_name -> value) for the parent stream.
    let mut seen: BTreeSet<Vec<(String, String)>> = BTreeSet::new();
    let mut tuples: Vec<BTreeMap<String, String>> = Vec::new();
    for r in &parent {
        let mut t = BTreeMap::new();
        for f in &referenced {
            if let Some(v) = r.lookup(f) {
                t.insert(f.clone(), v);
            }
        }
        if t.len() != referenced.len() {
            continue; // record didn't carry every required field
        }
        let key: Vec<(String, String)> = t.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        if seen.insert(key) {
            tuples.push(t);
        }
        if tuples.len() >= ctx.opts.map_max_fanout {
            break;
        }
    }

    let mut out = Vec::new();
    for t in tuples {
        let rendered = interpolate(sub_src, &t);
        let result = crate::exec::engine::execute_log(ctx, &rendered)
            .with_context(|| format!("`map` sub-query failed for {t:?}"))?;
        out.extend(result.records);
    }
    Ok(out)
}

/// Collect `$name$` references present in the (single-quoted) sub-query
/// source. Skips anything inside `"..."` string literals so values that
/// happen to contain `$word$` aren't treated as interpolations.
pub fn collect_field_refs(src: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let bytes = src.as_bytes();
    let mut i = 0;
    let mut in_string = false;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'"' {
            in_string = !in_string;
            i += 1;
            continue;
        }
        if in_string && c == b'\\' && i + 1 < bytes.len() {
            // We DO scan for `$name$` inside strings — that's exactly
            // where it matters: `service="$service"`. But escape
            // sequences should be skipped.
            i += 2;
            continue;
        }
        if c == b'$' {
            let mut j = i + 1;
            while j < bytes.len()
                && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_')
            {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'$' && j > i + 1 {
                let name = std::str::from_utf8(&bytes[i + 1..j])
                    .unwrap_or("")
                    .to_string();
                if seen.insert(name.clone()) {
                    out.push(name);
                }
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Substitute `$name$` placeholders with values, leaving everything
/// else untouched. Skips inside strings is *not* applied here: we want
/// to interpolate inside `"..."` quoted values.
pub fn interpolate(src: &str, vals: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(src.len());
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' {
            let mut j = i + 1;
            while j < bytes.len()
                && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_')
            {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'$' && j > i + 1 {
                let name = &src[i + 1..j];
                if let Some(v) = vals.get(name) {
                    // Escape `"` and `\` so the substitution stays
                    // safe inside a quoted string literal.
                    for c in v.chars() {
                        if c == '"' || c == '\\' {
                            out.push('\\');
                        }
                        out.push(c);
                    }
                } // unknown -> empty
                i = j + 1;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_unique_field_refs() {
        let src = r#"{server="arte", service="$service$", source=~"file:.*"} |~ "$service$""#;
        let refs = collect_field_refs(src);
        assert_eq!(refs, vec!["service"]);
    }
    #[test]
    fn interpolates_with_escaping() {
        let mut m = BTreeMap::new();
        m.insert("svc".into(), r#"weird"name"#.into());
        let out = interpolate(r#"x="$svc$""#, &m);
        assert_eq!(out, r#"x="weird\"name""#);
    }
    #[test]
    fn ignores_lone_dollar() {
        let src = "{a=\"$\"}";
        assert!(collect_field_refs(src).is_empty());
    }
}
