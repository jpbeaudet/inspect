//! Phase 10.3 — output renderers.
//!
//! Two entry points cover all user-facing commands:
//!
//! * [`render_doc`] handles the command-level [`OutputDoc`] used by
//!   aggregate verbs (status, health, why, connectivity, recipe,
//!   search). It knows how to flatten arbitrary `data` shapes into a
//!   tabular projection for CSV/TSV/Markdown/Table.
//! * [`render_rows`] handles per-record streaming verbs (ps, ports,
//!   images, volumes, network, list). The caller supplies the rows and
//!   a one-line summary; this function picks the right renderer based
//!   on the resolved [`OutputFormat`].
//!
//! Both honor the bible's per-format SUMMARY/DATA/NEXT visibility rules
//! (§10.3): Human/Table/Md retain decoration, JSON/CSV/TSV/YAML/Format
//! /Raw strip it (YAML keeps it as `# comments`).

use anyhow::Result;
use serde_json::{Map, Value};
use unicode_width::UnicodeWidthStr;

use crate::error::ExitKind;
use crate::format::template::Template;
use crate::format::OutputFormat;
use crate::verbs::output::{NextStep, OutputDoc};

// -----------------------------------------------------------------------------
// Public entry points
// -----------------------------------------------------------------------------

/// Render a single [`OutputDoc`] in the chosen format. `human_lines`
/// are the pre-rendered DATA lines used in default human output; the
/// renderer falls back to a generic projection of `doc.data` for
/// table/csv/etc.
///
/// `select` is the F19 (v0.1.3) `--select` triple (filter source, raw
/// flag, slurp flag). It is meaningful only on the JSON branch; the
/// non-JSON branches return `Ok(Success)` because `FormatArgs::resolve`
/// already rejects `--select` against any non-JSON-class format with
/// a usage-class error before reaching this point.
pub fn render_doc(
    doc: &OutputDoc,
    fmt: &OutputFormat,
    human_lines: &[String],
    select: Option<(&str, bool, bool)>,
) -> Result<ExitKind> {
    match fmt {
        OutputFormat::Human => {
            doc.print_human(human_lines);
            Ok(ExitKind::Success)
        }
        OutputFormat::Json => doc.print_json(select),
        OutputFormat::Yaml => render_doc_yaml(doc).map(|_| ExitKind::Success),
        OutputFormat::Table => render_doc_table(doc, human_lines, false).map(|_| ExitKind::Success),
        OutputFormat::Md => render_doc_markdown(doc, human_lines).map(|_| ExitKind::Success),
        OutputFormat::Csv => render_doc_rows_delimited(doc, ",").map(|_| ExitKind::Success),
        OutputFormat::Tsv => render_doc_rows_delimited(doc, "\t").map(|_| ExitKind::Success),
        OutputFormat::Format(tpl) => render_doc_template(doc, tpl).map(|_| ExitKind::Success),
        OutputFormat::Raw => render_doc_raw(doc, human_lines).map(|_| ExitKind::Success),
    }
}

/// Render a list of records (per-record verbs) in the chosen format.
/// `summary` and `next` are used only by formats that retain the
/// envelope (Human/Table/Md/Yaml).
pub fn render_rows(
    rows: &[Value],
    summary: &str,
    next: &[NextStep],
    fmt: &OutputFormat,
) -> Result<()> {
    match fmt {
        OutputFormat::Human | OutputFormat::Table => {
            print_envelope_summary(summary);
            if rows.is_empty() {
                crate::tee_println!("DATA:    (none)");
            } else {
                crate::tee_println!("DATA:");
                let (headers, table_rows) = collect_table(rows);
                for line in render_ascii_table(&headers, &table_rows) {
                    crate::tee_println!("  {line}");
                }
            }
            print_envelope_next(next);
            Ok(())
        }
        OutputFormat::Md => {
            print_envelope_summary(summary);
            let (headers, table_rows) = collect_table(rows);
            if !rows.is_empty() {
                crate::tee_println!("DATA:");
                for line in render_markdown_table(&headers, &table_rows) {
                    crate::tee_println!("  {line}");
                }
            } else {
                crate::tee_println!("DATA:    (none)");
            }
            print_envelope_next(next);
            Ok(())
        }
        OutputFormat::Json => {
            for r in rows {
                crate::tee_println!("{}", serde_json::to_string(r)?);
            }
            Ok(())
        }
        OutputFormat::Yaml => render_rows_yaml(rows, summary, next),
        OutputFormat::Csv => render_rows_delimited(rows, ","),
        OutputFormat::Tsv => render_rows_delimited(rows, "\t"),
        OutputFormat::Format(tpl) => render_rows_template(rows, tpl),
        OutputFormat::Raw => render_rows_raw(rows),
    }
}

// -----------------------------------------------------------------------------
// OutputDoc helpers
// -----------------------------------------------------------------------------

fn render_doc_yaml(doc: &OutputDoc) -> Result<()> {
    // Comments retain envelope context per §10.3.
    crate::tee_println!("# summary: {}", doc.summary);
    if !doc.next.is_empty() {
        crate::tee_println!("# next:");
        for n in &doc.next {
            crate::tee_println!("#   - {}  -- {}", n.cmd, n.rationale);
        }
    }
    let yaml = serde_yaml::to_string(&doc.data)?;
    print!("{yaml}");
    Ok(())
}

fn render_doc_raw(doc: &OutputDoc, human_lines: &[String]) -> Result<()> {
    if !human_lines.is_empty() {
        for l in human_lines {
            crate::tee_println!("{l}");
        }
        return Ok(());
    }
    // Fallback: stringify the data field minimally.
    match &doc.data {
        Value::String(s) => crate::tee_println!("{s}"),
        Value::Array(arr) => {
            for item in arr {
                crate::tee_println!("{}", value_scalar(item));
            }
        }
        other => crate::tee_println!("{}", serde_json::to_string(other)?),
    }
    Ok(())
}

fn render_doc_table(doc: &OutputDoc, human_lines: &[String], _md: bool) -> Result<()> {
    // For OutputDoc, the simplest correct table is the human DATA
    // lines wrapped with envelope context. Plain ASCII; no color.
    crate::tee_println!("SUMMARY: {}", doc.summary);
    if let Some(rows) = extract_doc_rows(&doc.data) {
        let (headers, table_rows) = collect_table(&rows);
        crate::tee_println!("DATA:");
        for line in render_ascii_table(&headers, &table_rows) {
            crate::tee_println!("  {line}");
        }
    } else if human_lines.is_empty() {
        crate::tee_println!("DATA:    (none)");
    } else {
        crate::tee_println!("DATA:");
        for l in human_lines {
            crate::tee_println!("  {l}");
        }
    }
    print_envelope_next(&doc.next);
    Ok(())
}

fn render_doc_markdown(doc: &OutputDoc, human_lines: &[String]) -> Result<()> {
    crate::tee_println!("SUMMARY: {}", doc.summary);
    if let Some(rows) = extract_doc_rows(&doc.data) {
        let (headers, table_rows) = collect_table(&rows);
        crate::tee_println!("DATA:");
        for line in render_markdown_table(&headers, &table_rows) {
            crate::tee_println!("  {line}");
        }
    } else if human_lines.is_empty() {
        crate::tee_println!("DATA:    (none)");
    } else {
        crate::tee_println!("DATA:");
        for l in human_lines {
            crate::tee_println!("  {l}");
        }
    }
    print_envelope_next(&doc.next);
    Ok(())
}

fn render_doc_rows_delimited(doc: &OutputDoc, sep: &str) -> Result<()> {
    let rows = extract_doc_rows(&doc.data).unwrap_or_default();
    render_rows_delimited(&rows, sep)
}

fn render_doc_template(doc: &OutputDoc, tpl: &str) -> Result<()> {
    let template = Template::parse(tpl)?;
    if let Some(rows) = extract_doc_rows(&doc.data) {
        for r in &rows {
            crate::tee_println!("{}", template.render(r)?);
        }
    } else {
        // Apply against the data value itself (or empty object).
        let v = if doc.data.is_object() {
            doc.data.clone()
        } else {
            Value::Object(Map::new())
        };
        crate::tee_println!("{}", template.render(&v)?);
    }
    Ok(())
}

/// Best-effort extraction of a table-friendly row list out of the
/// `data` field. We look for the first array of objects under any
/// top-level key (`services`, `records`, `steps`, ...) so the renderer
/// works across commands without each one having to register a schema.
fn extract_doc_rows(data: &Value) -> Option<Vec<Value>> {
    if let Value::Array(arr) = data {
        if arr.iter().all(|v| v.is_object()) {
            return Some(arr.clone());
        }
    }
    if let Value::Object(map) = data {
        for (_, v) in map.iter() {
            if let Value::Array(arr) = v {
                if !arr.is_empty() && arr.iter().all(|x| x.is_object()) {
                    return Some(arr.clone());
                }
            }
        }
    }
    None
}

// -----------------------------------------------------------------------------
// Row helpers
// -----------------------------------------------------------------------------

fn render_rows_yaml(rows: &[Value], summary: &str, next: &[NextStep]) -> Result<()> {
    crate::tee_println!("# summary: {summary}");
    if !next.is_empty() {
        crate::tee_println!("# next:");
        for n in next {
            crate::tee_println!("#   - {}  -- {}", n.cmd, n.rationale);
        }
    }
    let yaml = serde_yaml::to_string(&rows)?;
    print!("{yaml}");
    Ok(())
}

fn render_rows_delimited(rows: &[Value], sep: &str) -> Result<()> {
    let (headers, table_rows) = collect_table(rows);
    if headers.is_empty() {
        return Ok(());
    }
    let escape = |s: &str| {
        if sep == "," {
            csv_escape(s)
        } else {
            tsv_escape(s)
        }
    };
    crate::tee_println!(
        "{}",
        headers
            .iter()
            .map(|h| escape(h))
            .collect::<Vec<_>>()
            .join(sep)
    );
    for row in &table_rows {
        let line = row.iter().map(|c| escape(c)).collect::<Vec<_>>().join(sep);
        crate::tee_println!("{line}");
    }
    Ok(())
}

fn render_rows_template(rows: &[Value], tpl: &str) -> Result<()> {
    let template = Template::parse(tpl)?;
    for r in rows {
        crate::tee_println!("{}", template.render(r)?);
    }
    Ok(())
}

fn render_rows_raw(rows: &[Value]) -> Result<()> {
    for r in rows {
        match r {
            Value::String(s) => crate::tee_println!("{s}"),
            Value::Object(map) => {
                // Pick the most meaningful scalar: prefer `name`,
                // `id`, `value`, then first scalar key.
                let pick = ["name", "id", "value", "service", "server"]
                    .iter()
                    .find_map(|k| map.get(*k))
                    .or_else(|| map.values().find(|v| !v.is_object() && !v.is_array()));
                match pick {
                    Some(v) => crate::tee_println!("{}", value_scalar(v)),
                    None => crate::tee_println!("{}", serde_json::to_string(r)?),
                }
            }
            other => crate::tee_println!("{}", value_scalar(other)),
        }
    }
    Ok(())
}

// -----------------------------------------------------------------------------
// Tabulation primitives
// -----------------------------------------------------------------------------

/// Walk all rows once to gather the union of keys (preserving first-
/// seen order), then build a string table. The first columns are the
/// reserved envelope fields when present (`_source`, `_medium`,
/// `server`, `service`) so users get stable column ordering.
fn collect_table(rows: &[Value]) -> (Vec<String>, Vec<Vec<String>>) {
    const PRELUDE: &[&str] = &["_source", "_medium", "server", "service"];
    let mut headers: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Reserved prelude (if any row has them).
    for k in PRELUDE {
        if rows.iter().any(|r| r.get(*k).is_some()) {
            headers.push((*k).to_string());
            seen.insert((*k).to_string());
        }
    }
    for r in rows {
        if let Some(map) = r.as_object() {
            for k in map.keys() {
                if !seen.contains(k) {
                    seen.insert(k.clone());
                    headers.push(k.clone());
                }
            }
        }
    }
    let table: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            headers
                .iter()
                .map(|h| match r.get(h) {
                    Some(v) => value_scalar(v),
                    None => String::new(),
                })
                .collect()
        })
        .collect();
    (headers, table)
}

fn value_scalar(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(value_scalar).collect::<Vec<_>>().join(","),
        Value::Object(_) => serde_json::to_string(v).unwrap_or_default(),
    }
}

fn render_ascii_table(headers: &[String], rows: &[Vec<String>]) -> Vec<String> {
    if headers.is_empty() {
        return Vec::new();
    }
    let mut widths: Vec<usize> = headers.iter().map(|h| display_width(h)).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(display_width(cell));
            }
        }
    }
    let mut out = Vec::with_capacity(rows.len() + 1);
    let header_line = headers
        .iter()
        .enumerate()
        .map(|(i, h)| pad_right(h, widths[i]))
        .collect::<Vec<_>>()
        .join("  ");
    out.push(header_line);
    for row in rows {
        let line = row
            .iter()
            .enumerate()
            .map(|(i, c)| pad_right(c, widths[i]))
            .collect::<Vec<_>>()
            .join("  ");
        out.push(line);
    }
    out
}

fn render_markdown_table(headers: &[String], rows: &[Vec<String>]) -> Vec<String> {
    if headers.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(rows.len() + 2);
    out.push(format!(
        "| {} |",
        headers
            .iter()
            .map(|h| md_escape(h))
            .collect::<Vec<_>>()
            .join(" | ")
    ));
    out.push(format!(
        "| {} |",
        headers
            .iter()
            .map(|_| "---".to_string())
            .collect::<Vec<_>>()
            .join(" | ")
    ));
    for row in rows {
        out.push(format!(
            "| {} |",
            row.iter()
                .map(|c| md_escape(c))
                .collect::<Vec<_>>()
                .join(" | ")
        ));
    }
    out
}

fn pad_right(s: &str, width: usize) -> String {
    let n = display_width(s);
    if n >= width {
        s.to_string()
    } else {
        let mut o = String::with_capacity(s.len() + (width - n));
        o.push_str(s);
        for _ in 0..(width - n) {
            o.push(' ');
        }
        o
    }
}

/// Display width in terminal cells, honoring CJK / emoji widths
/// (audit §5.3). Falls back to char count for control chars.
fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Returns true if `s` starts with a character spreadsheet engines
/// (Excel, LibreOffice, Google Sheets) interpret as the start of a
/// formula. Per OWASP CSV Injection guidance: `=`, `+`, `-`, `@`,
/// TAB (`\t`), and CR (`\r`) are all dangerous.
fn is_formula_prefix(s: &str) -> bool {
    matches!(
        s.as_bytes().first(),
        Some(b'=' | b'+' | b'-' | b'@' | b'\t' | b'\r')
    )
}

fn csv_escape(s: &str) -> String {
    // Defuse spreadsheet formula injection by prefixing a literal `'`
    // (audit §5.1). The single quote is the documented OWASP
    // mitigation and is invisible in cells.
    let needs_defuse = is_formula_prefix(s);
    let needs_quote =
        needs_defuse || s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r');
    if needs_quote {
        let mut body = String::with_capacity(s.len() + 3);
        if needs_defuse {
            body.push('\'');
        }
        body.push_str(&s.replace('"', "\"\""));
        format!("\"{body}\"")
    } else {
        s.to_string()
    }
}

fn tsv_escape(s: &str) -> String {
    // Strip tabs/newlines first so column alignment survives.
    let cleaned = s.replace(['\t', '\n', '\r'], " ");
    if is_formula_prefix(&cleaned) {
        // Same defusing rationale as CSV.
        format!("'{cleaned}")
    } else {
        cleaned
    }
}

fn md_escape(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn print_envelope_summary(summary: &str) {
    crate::tee_println!("SUMMARY: {summary}");
}

fn print_envelope_next(next: &[NextStep]) {
    if next.is_empty() {
        crate::tee_println!("NEXT:    (none)");
    } else {
        crate::tee_println!("NEXT:");
        for n in next {
            crate::tee_println!("  {}  -- {}", n.cmd, n.rationale);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rows() -> Vec<Value> {
        vec![
            json!({"_source": "docker", "_medium": "ssh", "server": "h1", "service": "api", "state": "up"}),
            json!({"_source": "docker", "_medium": "ssh", "server": "h2", "service": "api", "state": "down"}),
        ]
    }

    #[test]
    fn collect_table_orders_prelude_first() {
        let (h, t) = collect_table(&rows());
        assert_eq!(h[0], "_source");
        assert_eq!(h[1], "_medium");
        assert_eq!(h[2], "server");
        assert_eq!(h[3], "service");
        assert!(h.contains(&"state".to_string()));
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn csv_escape_quotes_when_needed() {
        assert_eq!(csv_escape("plain"), "plain");
        assert_eq!(csv_escape("a,b"), "\"a,b\"");
        assert_eq!(csv_escape("she said \"hi\""), "\"she said \"\"hi\"\"\"");
    }

    #[test]
    fn csv_escape_defuses_formula_prefixes() {
        // OWASP CSV injection mitigation: spreadsheet apps treat a
        // leading `=`, `+`, `-`, `@`, TAB, or CR as a formula. We
        // wrap the cell and prefix it with a literal apostrophe.
        for prefix in ["=", "+", "-", "@", "\t", "\r"] {
            let cell = format!("{prefix}cmd|calc");
            let out = csv_escape(&cell);
            assert!(
                out.starts_with("\"'"),
                "expected formula defusing for {prefix:?}, got {out:?}"
            );
        }
        // safe leading char: not quoted, not prefixed
        assert_eq!(csv_escape("0=ok"), "0=ok");
    }

    #[test]
    fn tsv_escape_strips_tabs() {
        assert_eq!(tsv_escape("a\tb"), "a b");
    }

    #[test]
    fn tsv_escape_defuses_formula_prefix() {
        assert_eq!(tsv_escape("=SUM(A1)"), "'=SUM(A1)");
        assert_eq!(tsv_escape("+1"), "'+1");
        assert_eq!(tsv_escape("safe"), "safe");
    }

    #[test]
    fn ascii_table_aligns_with_unicode_width() {
        // CJK chars are width-2; emoji is width-2 in most fonts.
        // chars().count() would give 1, mis-aligning the column.
        let headers = vec!["name".to_string(), "v".to_string()];
        let rows = vec![
            vec!["日本語".to_string(), "1".to_string()],
            vec!["en".to_string(), "22".to_string()],
        ];
        let out = render_ascii_table(&headers, &rows);
        // header padded to max(width("name"), width("日本語")=6, width("en")=2) = 6
        assert!(out[0].starts_with("name  "));
        assert!(out[1].starts_with("日本語"));
    }

    #[test]
    fn markdown_pipes_escaped() {
        assert_eq!(md_escape("a|b"), "a\\|b");
    }

    #[test]
    fn extract_doc_rows_finds_nested_array() {
        let v = json!({"services": [{"x": 1}, {"x": 2}]});
        let rows = extract_doc_rows(&v).unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn extract_doc_rows_returns_none_for_scalar_data() {
        let v = json!({"summary": "ok"});
        assert!(extract_doc_rows(&v).is_none());
    }

    #[test]
    fn ascii_table_aligns_columns() {
        let headers = vec!["a".to_string(), "bb".to_string()];
        let rows = vec![vec!["1".to_string(), "22".to_string()]];
        let out = render_ascii_table(&headers, &rows);
        assert_eq!(out[0], "a  bb");
        assert_eq!(out[1], "1  22");
    }
}
