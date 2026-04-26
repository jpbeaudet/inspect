//! LogQL parser, AST, and validators (bible §9).
//!
//! The grammar implemented here matches §9.13 verbatim:
//!
//! ```text
//! query        ::= log_query | metric_query
//! log_query    ::= selector_union (filter | stage)*
//! metric_query ::= range_aggregation | vector_aggregation
//! ```
//!
//! plus the two `inspect`-specific extensions:
//! * reserved label names (`server`, `service`, `source`)
//! * the `map { <log_query> }` stage (Splunk SPL convention)
//!
//! The parser is hand-written recursive descent over a tokenized stream
//! with explicit byte spans so error diagnostics can render code frames.
//! We deliberately avoid `chumsky` here: the grammar is small, alias
//! substitution must run before parsing, and the BNF is well-defined.
//! A hand-rolled parser yields better error spans and zero extra deps.

// Phase 6 ships parsing + validation. Phase 7 wires this into
// `inspect search`; until then the public surface is allowed dead-code.
#![allow(dead_code)]

pub mod alias_subst;
pub mod ast;
pub mod error;
pub mod lexer;
pub mod parser;
pub mod validate;

#[cfg(test)]
mod tests;

pub use ast::Query;
pub use error::ParseError;

/// Parse a query string into a typed AST. Aliases (`@name`) are
/// expanded before parsing using `alias_resolver`.
///
/// `alias_resolver` is invoked for each `@name` reference. Returning
/// `None` produces an "unknown alias" diagnostic.
pub fn parse(input: &str) -> Result<Query, ParseError> {
    parse_with_aliases(input, |_| None)
}

/// Same as [`parse`] but with a custom alias resolver.
pub fn parse_with_aliases<F>(input: &str, alias_resolver: F) -> Result<Query, ParseError>
where
    F: Fn(&str) -> Option<String>,
{
    let (expanded, expansions) = alias_subst::expand_with_map(input, &alias_resolver)?;
    let frame = |e: ParseError| frame_alias_error(e, &expansions);
    let tokens = lexer::tokenize(&expanded).map_err(&frame)?;
    let ast = parser::parse_tokens(&tokens, &expanded).map_err(&frame)?;
    validate::validate(&ast).map_err(&frame)?;
    Ok(ast)
}

/// If a downstream error span falls inside an expanded alias body,
/// re-frame the message as "in expansion of `@name`: …" and snap the
/// span back to the original `@name` reference. This keeps diagnostics
/// pointing at code the user actually typed (audit §1.7).
fn frame_alias_error(mut e: ParseError, expansions: &[alias_subst::Expansion]) -> ParseError {
    for ex in expansions {
        if e.span.start >= ex.expanded_span.start && e.span.start < ex.expanded_span.end {
            e.message = format!("in expansion of `@{}`: {}", ex.name, e.message);
            e.span = ex.original_span.clone();
            return e;
        }
    }
    e
}

/// Expand aliases in `input` without parsing. The returned string is
/// suitable to pass to [`parse`] (which won't see any `@name` tokens)
/// and to slice with AST spans.
pub fn expand_aliases<F>(input: &str, alias_resolver: F) -> Result<String, ParseError>
where
    F: Fn(&str) -> Option<String>,
{
    alias_subst::expand(input, &alias_resolver)
}
