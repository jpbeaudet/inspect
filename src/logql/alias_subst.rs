//! Pre-parse alias substitution.
//!
//! Aliases are referenced in queries via `@name` (parameterless,
//! earlier) or `@name(k=v,...)` (parameterized). The reference is
//! replaced with the literal alias body before the parser runs, so the
//! parser never sees a raw `@name`. (Bible §6.7, §9.3.)
//!
//! Chaining is *allowed* in parameterized form but is the resolver's responsibility:
//! this module asks `resolve` for a fully-substituted body. The
//! resolver must walk any nested `@other(...)` references and surface
//! cycle / depth-cap errors. (`alias::expand_recursive` is the
//! production resolver; see `src/logql/mod.rs`.)

use std::collections::BTreeMap;

use super::error::ParseError;
use crate::alias;

/// Records one alias substitution so parse errors that fall inside an
/// expanded region can be re-framed in terms of the original source
/// (audit §1.7).
#[derive(Debug, Clone)]
pub struct Expansion {
    /// Alias name (without the leading `@`).
    pub name: String,
    /// Byte span of the `@name` (or `@name(...)`) reference in the
    /// **original** input.
    pub original_span: std::ops::Range<usize>,
    /// Byte span the substituted body occupies in the **expanded**
    /// output.
    pub expanded_span: std::ops::Range<usize>,
}

/// Resolver outcome. `Ok(Some(body))` is the substituted body;
/// `Ok(None)` means "no such alias" (the scanner emits an unknown-
/// alias error); `Err` is propagated as a `ParseError` (so a
/// `MissingParam` from the alias layer surfaces as a query parse
/// error attached to the original `@name(...)` span).
pub type ResolverResult = Result<Option<String>, ParseError>;

/// Expand `@name[(...)]` references inside a LogQL query string.
pub fn expand<F>(input: &str, resolve: &F) -> Result<String, ParseError>
where
    F: Fn(&str, &BTreeMap<String, String>) -> ResolverResult,
{
    expand_with_map(input, resolve).map(|(s, _)| s)
}

/// Same as [`expand`] but also returns the list of expansions performed,
/// so callers can re-frame downstream parse errors that point into an
/// expanded region (audit §1.7).
pub fn expand_with_map<F>(input: &str, resolve: &F) -> Result<(String, Vec<Expansion>), ParseError>
where
    F: Fn(&str, &BTreeMap<String, String>) -> ResolverResult,
{
    let mut out = String::with_capacity(input.len());
    let mut expansions: Vec<Expansion> = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut in_string = false;
    while i < bytes.len() {
        let c = bytes[i];
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
            let rest = &input[i..];
            let cs_opt = alias::try_parse_call_site_prefix(rest).map_err(|e| {
                let end = i
                    + 1
                    + rest
                        .as_bytes()
                        .iter()
                        .skip(1)
                        .position(|&b| b == b')' || b == b' ' || b == b'\n')
                        .map(|p| p + 1)
                        .unwrap_or(rest.len() - 1);
                ParseError::new(format!("alias call site error: {e}"), i..end)
            })?;
            let Some(cs) = cs_opt else {
                // Bare `@` or non-name — leave for the lexer to error on.
                out.push('@');
                i += 1;
                continue;
            };
            let original_end = i + cs.span_len;
            let body = resolve(&cs.name, &cs.params).map_err(|mut e| {
                e.span = i..original_end;
                e
            })?;
            let Some(body) = body else {
                return Err(ParseError::new(
                    format!("unknown alias `@{}`", cs.name),
                    i..original_end,
                )
                .with_hint("define it via `inspect alias add` or check the name"));
            };
            let exp_start = out.len();
            out.push_str(&body);
            let exp_end = out.len();
            expansions.push(Expansion {
                name: cs.name.clone(),
                original_span: i..original_end,
                expanded_span: exp_start..exp_end,
            });
            i = original_end;
            continue;
        }
        out.push(c as char);
        i += 1;
    }
    Ok((out, expansions))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nop_resolver_static(name: &str, _: &BTreeMap<String, String>) -> ResolverResult {
        Ok((name == "plogs").then(|| "{server=\"arte\", source=\"logs\"}".to_string()))
    }

    #[test]
    fn substitutes_alias_verbatim() {
        let out = expand("@plogs |= \"x\"", &nop_resolver_static).unwrap();
        assert!(out.starts_with("{server=\"arte\""));
        assert!(out.contains("} |= \"x\""));
    }

    #[test]
    fn does_not_expand_inside_string() {
        let out = expand(r#"{a="@x"}"#, &|_, _| Ok(Some("BOOM".into()))).unwrap();
        assert_eq!(out, r#"{a="@x"}"#);
    }

    #[test]
    fn unknown_alias_errors() {
        let e = expand("@nope", &|_, _| Ok(None)).unwrap_err();
        assert!(e.message.contains("unknown alias"));
    }

    #[test]
    fn passes_through_when_no_alias() {
        let s = "{server=\"arte\"} |= \"x\"";
        assert_eq!(expand(s, &|_, _| Ok(None)).unwrap(), s);
    }

    #[test]
    fn map_records_original_and_expanded_spans() {
        let (out, exps) = expand_with_map("@a or @b", &|n, _| match n {
            "a" => Ok(Some("AAAA".into())),
            "b" => Ok(Some("BBBBBB".into())),
            _ => Ok(None),
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

    #[test]
    fn parameterized_call_site_passes_params_to_resolver() {
        let out = expand("@svc(svc=pulse,env=prod) |= \"x\"", &|n, p| {
            assert_eq!(n, "svc");
            assert_eq!(p.get("svc").map(String::as_str), Some("pulse"));
            assert_eq!(p.get("env").map(String::as_str), Some("prod"));
            Ok(Some(format!(
                "{{server=\"arte\", svc=\"{}\", env=\"{}\"}}",
                p["svc"], p["env"]
            )))
        })
        .unwrap();
        assert!(out.contains("svc=\"pulse\""));
        assert!(out.contains("env=\"prod\""));
        assert!(out.contains("|= \"x\""));
    }

    #[test]
    fn parameterized_call_site_records_full_original_span() {
        let (_out, exps) =
            expand_with_map("@svc(svc=pulse) |= \"x\"", &|_, _| Ok(Some("BODY".into()))).unwrap();
        assert_eq!(exps.len(), 1);
        // span should cover the whole `@svc(svc=pulse)` token (15 chars)
        assert_eq!(exps[0].original_span, 0..15);
    }

    #[test]
    fn resolver_error_is_attached_to_call_site_span() {
        let err = expand("@svc(svc=pulse)", &|_, _| {
            Err(ParseError::new("missing param `pat`", 0..0))
        })
        .unwrap_err();
        assert!(err.message.contains("missing param"));
        assert_eq!(err.span, 0..15);
    }
}
