//! Pre-parse alias substitution.
//!
//! Aliases are referenced in queries via `@name`. The reference is
//! replaced with the literal alias body before the parser runs, so the
//! parser never sees a raw `@name`. (Bible §6.7, §9.3.)
//!
//! We also forbid alias chaining at this layer: an alias body may not
//! itself contain another `@name`.

use super::error::ParseError;

/// Records one alias substitution so parse errors that fall inside an
/// expanded region can be re-framed in terms of the original source
/// (audit §1.7).
#[derive(Debug, Clone)]
pub struct Expansion {
    /// Alias name (without the leading `@`).
    pub name: String,
    /// Byte span of the `@name` reference in the **original** input.
    pub original_span: std::ops::Range<usize>,
    /// Byte span the body occupies in the **expanded** output.
    pub expanded_span: std::ops::Range<usize>,
}

/// Expand `@name` references inside a LogQL query string.
///
/// Returns the substituted text. The returned string preserves the
/// total byte length where possible (we don't pad/align — spans are
/// recomputed from the substituted source by the lexer).
pub fn expand<F>(input: &str, resolve: &F) -> Result<String, ParseError>
where
    F: Fn(&str) -> Option<String>,
{
    expand_with_map(input, resolve).map(|(s, _)| s)
}

/// Same as [`expand`] but also returns the list of expansions performed,
/// so callers can re-frame downstream parse errors that point into an
/// expanded region (audit §1.7).
pub fn expand_with_map<F>(input: &str, resolve: &F) -> Result<(String, Vec<Expansion>), ParseError>
where
    F: Fn(&str) -> Option<String>,
{
    let mut out = String::with_capacity(input.len());
    let mut expansions: Vec<Expansion> = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut in_string = false;
    while i < bytes.len() {
        let c = bytes[i];
        // Track quoted-string regions so we don't expand inside them.
        if c == b'"' {
            in_string = !in_string;
            out.push('"');
            i += 1;
            continue;
        }
        if in_string {
            if c == b'\\' && i + 1 < bytes.len() {
                out.push(c as char);
                out.push(bytes[i + 1] as char);
                i += 2;
                continue;
            }
            out.push(c as char);
            i += 1;
            continue;
        }
        if c == b'@' {
            let mut j = i + 1;
            while j < bytes.len()
                && (bytes[j].is_ascii_alphanumeric()
                    || bytes[j] == b'_'
                    || bytes[j] == b'-'
                    || bytes[j] == b'.')
            {
                j += 1;
            }
            if j == i + 1 {
                // bare `@` — leave for the lexer to error on
                out.push('@');
                i += 1;
                continue;
            }
            let name = &input[i + 1..j];
            let Some(body) = resolve(name) else {
                return Err(ParseError::new(format!("unknown alias `@{name}`"), i..j)
                    .with_hint("define it via `inspect alias add` or check the name"));
            };
            if body.contains('@') && contains_alias_ref_outside_strings(&body) {
                return Err(ParseError::new(
                    format!("alias `@{name}` references another alias (chaining is not supported in v1)"),
                    i..j,
                ));
            }
            // Insert the body verbatim. Aliases are always selectors
            // (`{...}`) or selector unions, so they slot in at the
            // selector position without grouping parens.
            let exp_start = out.len();
            out.push_str(&body);
            let exp_end = out.len();
            expansions.push(Expansion {
                name: name.to_string(),
                original_span: i..j,
                expanded_span: exp_start..exp_end,
            });
            i = j;
            continue;
        }
        out.push(c as char);
        i += 1;
    }
    Ok((out, expansions))
}

fn contains_alias_ref_outside_strings(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'"' {
            in_string = !in_string;
            i += 1;
            continue;
        }
        if !in_string && c == b'@' {
            // followed by an alias-name char?
            if let Some(&n) = bytes.get(i + 1) {
                if n.is_ascii_alphanumeric() || n == b'_' {
                    return true;
                }
            }
        }
        if in_string && c == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_alias_verbatim() {
        let out = expand("@plogs |= \"x\"", &|n| {
            (n == "plogs").then(|| "{server=\"arte\", source=\"logs\"}".to_string())
        })
        .unwrap();
        assert!(out.starts_with("{server=\"arte\""));
        assert!(out.contains("} |= \"x\""));
    }

    #[test]
    fn does_not_expand_inside_string() {
        let out = expand(r#"{a="@x"}"#, &|_| Some("BOOM".into())).unwrap();
        assert_eq!(out, r#"{a="@x"}"#);
    }

    #[test]
    fn unknown_alias_errors() {
        let e = expand("@nope", &|_| None).unwrap_err();
        assert!(e.message.contains("unknown alias"));
    }

    #[test]
    fn rejects_chained_alias() {
        let e = expand("@a", &|_| Some("@b".into())).unwrap_err();
        assert!(e.message.contains("chaining"));
    }

    #[test]
    fn passes_through_when_no_alias() {
        let s = "{server=\"arte\"} |= \"x\"";
        assert_eq!(expand(s, &|_| None).unwrap(), s);
    }

    #[test]
    fn map_records_original_and_expanded_spans() {
        let (out, exps) = expand_with_map("@a or @b", &|n| match n {
            "a" => Some("AAAA".into()),
            "b" => Some("BBBBBB".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(out, "AAAA or BBBBBB");
        assert_eq!(exps.len(), 2);
        assert_eq!(exps[0].name, "a");
        assert_eq!(exps[0].original_span, 0..2);
        assert_eq!(&out[exps[0].expanded_span.clone()], "AAAA");
        assert_eq!(exps[1].name, "b");
        assert_eq!(exps[1].original_span, 6..8);
        assert_eq!(&out[exps[1].expanded_span.clone()], "BBBBBB");
    }
}
