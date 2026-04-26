//! Lexer for LogQL. Produces a stream of (`Token`, span) pairs.

use std::ops::Range;

use super::error::ParseError;

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // structural
    LBrace,    // {
    RBrace,    // }
    LParen,    // (
    RParen,    // )
    LBracket,  // [
    RBracket,  // ]
    Comma,
    Pipe,      // bare `|` (introduces a stage)
    // label/field comparison ops (also reused inside selectors)
    Eq,        // =
    Ne,        // !=
    Re,        // =~
    Nre,       // !~
    EqEq,      // ==
    Gt,        // >
    Ge,        // >=
    Lt,        // <
    Le,        // <=
    // line filter ops
    PipeEq,    // |=
    PipeRe,    // |~
    // values
    String(String),
    Ident(String),
    Number(f64),
    Integer(i64),
    Duration(u64), // milliseconds
    AliasRef(String),
    // keywords (also valid as idents in some positions; lexer doesn't know context)
    KwOr,
    KwAnd,
    KwNot,
    KwBy,
    KwWithout,
}

impl Token {
    /// User-friendly rendering of a token for diagnostics.
    /// e.g. `LBrace` -> "`{`", `Ident("foo")` -> "`foo`", `String(_)` -> "a string".
    pub fn display(&self) -> String {
        match self {
            Token::LBrace => "`{`".into(),
            Token::RBrace => "`}`".into(),
            Token::LParen => "`(`".into(),
            Token::RParen => "`)`".into(),
            Token::LBracket => "`[`".into(),
            Token::RBracket => "`]`".into(),
            Token::Comma => "`,`".into(),
            Token::Pipe => "`|`".into(),
            Token::Eq => "`=`".into(),
            Token::Ne => "`!=`".into(),
            Token::Re => "`=~`".into(),
            Token::Nre => "`!~`".into(),
            Token::EqEq => "`==`".into(),
            Token::Gt => "`>`".into(),
            Token::Ge => "`>=`".into(),
            Token::Lt => "`<`".into(),
            Token::Le => "`<=`".into(),
            Token::PipeEq => "`|=`".into(),
            Token::PipeRe => "`|~`".into(),
            Token::String(s) => {
                // Truncate long strings in diagnostics.
                let preview: String = s.chars().take(24).collect();
                let trail = if s.chars().count() > 24 { "…" } else { "" };
                format!("a string (\"{preview}{trail}\")")
            }
            Token::Ident(s) => format!("`{s}`"),
            Token::Number(n) => format!("the number `{n}`"),
            Token::Integer(n) => format!("the number `{n}`"),
            Token::Duration(_) => "a duration".into(),
            Token::AliasRef(n) => format!("alias reference `@{n}`"),
            Token::KwOr => "`or`".into(),
            Token::KwAnd => "`and`".into(),
            Token::KwNot => "`not`".into(),
            Token::KwBy => "`by`".into(),
            Token::KwWithout => "`without`".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Spanned {
    pub token: Token,
    pub span: Range<usize>,
}

pub fn tokenize(src: &str) -> Result<Vec<Spanned>, ParseError> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        // whitespace
        if c == b' ' || c == b'\t' || c == b'\r' || c == b'\n' {
            i += 1;
            continue;
        }
        let start = i;
        // single + multi-char punctuation
        match c {
            b'{' => { out.push(spanned(Token::LBrace, start..start + 1)); i += 1; continue; }
            b'}' => { out.push(spanned(Token::RBrace, start..start + 1)); i += 1; continue; }
            b'(' => { out.push(spanned(Token::LParen, start..start + 1)); i += 1; continue; }
            b')' => { out.push(spanned(Token::RParen, start..start + 1)); i += 1; continue; }
            b'[' => { out.push(spanned(Token::LBracket, start..start + 1)); i += 1; continue; }
            b']' => { out.push(spanned(Token::RBracket, start..start + 1)); i += 1; continue; }
            b',' => { out.push(spanned(Token::Comma, start..start + 1)); i += 1; continue; }
            b'=' => {
                if peek(bytes, i + 1) == Some(b'~') {
                    out.push(spanned(Token::Re, start..start + 2));
                    i += 2;
                } else if peek(bytes, i + 1) == Some(b'=') {
                    out.push(spanned(Token::EqEq, start..start + 2));
                    i += 2;
                } else {
                    out.push(spanned(Token::Eq, start..start + 1));
                    i += 1;
                }
                continue;
            }
            b'!' => {
                if peek(bytes, i + 1) == Some(b'=') {
                    out.push(spanned(Token::Ne, start..start + 2));
                    i += 2;
                } else if peek(bytes, i + 1) == Some(b'~') {
                    out.push(spanned(Token::Nre, start..start + 2));
                    i += 2;
                } else {
                    return Err(ParseError::new(
                        "unexpected `!`; expected `!=` or `!~`",
                        start..start + 1,
                    ));
                }
                continue;
            }
            b'|' => {
                match peek(bytes, i + 1) {
                    Some(b'=') => {
                        out.push(spanned(Token::PipeEq, start..start + 2));
                        i += 2;
                    }
                    Some(b'~') => {
                        out.push(spanned(Token::PipeRe, start..start + 2));
                        i += 2;
                    }
                    _ => {
                        out.push(spanned(Token::Pipe, start..start + 1));
                        i += 1;
                    }
                }
                continue;
            }
            b'>' => {
                if peek(bytes, i + 1) == Some(b'=') {
                    out.push(spanned(Token::Ge, start..start + 2));
                    i += 2;
                } else {
                    out.push(spanned(Token::Gt, start..start + 1));
                    i += 1;
                }
                continue;
            }
            b'<' => {
                if peek(bytes, i + 1) == Some(b'=') {
                    out.push(spanned(Token::Le, start..start + 2));
                    i += 2;
                } else {
                    out.push(spanned(Token::Lt, start..start + 1));
                    i += 1;
                }
                continue;
            }
            b'"' => {
                let (s, end) = lex_string(bytes, i)?;
                out.push(spanned(Token::String(s), start..end));
                i = end;
                continue;
            }
            b'@' => {
                // alias ref `@name`
                let mut j = i + 1;
                while j < bytes.len() && is_alias_char(bytes[j]) {
                    j += 1;
                }
                if j == i + 1 {
                    return Err(ParseError::new(
                        "expected alias name after `@`",
                        start..start + 1,
                    ));
                }
                let name = std::str::from_utf8(&bytes[i + 1..j])
                    .expect("ascii alias")
                    .to_string();
                out.push(spanned(Token::AliasRef(name), start..j));
                i = j;
                continue;
            }
            b'-' | b'0'..=b'9' => {
                let (tok, end) = lex_number_or_duration(bytes, i)?;
                out.push(spanned(tok, start..end));
                i = end;
                continue;
            }
            _ if is_ident_start(c) => {
                let mut j = i + 1;
                while j < bytes.len() && is_ident_continue(bytes[j]) {
                    j += 1;
                }
                let word = std::str::from_utf8(&bytes[i..j])
                    .map_err(|_| ParseError::new("invalid utf-8 in identifier", i..j))?;
                let tok = match word {
                    "or" => Token::KwOr,
                    "and" => Token::KwAnd,
                    "not" => Token::KwNot,
                    "by" => Token::KwBy,
                    "without" => Token::KwWithout,
                    _ => Token::Ident(word.to_string()),
                };
                out.push(spanned(tok, start..j));
                i = j;
                continue;
            }
            _ => {
                return Err(ParseError::new(
                    format!("unexpected character `{}`", c as char),
                    start..start + 1,
                ));
            }
        }
    }
    Ok(out)
}

fn spanned(token: Token, span: Range<usize>) -> Spanned {
    Spanned { token, span }
}

fn peek(b: &[u8], i: usize) -> Option<u8> {
    b.get(i).copied()
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}
fn is_ident_continue(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}
fn is_alias_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-' || c == b'.'
}

fn lex_string(b: &[u8], start: usize) -> Result<(String, usize), ParseError> {
    debug_assert_eq!(b[start], b'"');
    let mut i = start + 1;
    let mut out = String::new();
    while i < b.len() {
        match b[i] {
            b'"' => return Ok((out, i + 1)),
            b'\\' => {
                if i + 1 >= b.len() {
                    return Err(ParseError::new(
                        "unterminated escape sequence",
                        i..i + 1,
                    ));
                }
                match b[i + 1] {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'n' => out.push('\n'),
                    b't' => out.push('\t'),
                    b'r' => out.push('\r'),
                    other => {
                        return Err(ParseError::new(
                            format!("unknown escape `\\{}`", other as char),
                            i..i + 2,
                        ));
                    }
                }
                i += 2;
            }
            c => {
                out.push(c as char);
                i += 1;
            }
        }
    }
    Err(ParseError::new(
        "unterminated string literal",
        start..b.len(),
    ))
}

fn lex_number_or_duration(b: &[u8], start: usize) -> Result<(Token, usize), ParseError> {
    let mut i = start;
    if b[i] == b'-' {
        i += 1;
    }
    let digits_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == digits_start {
        return Err(ParseError::new("expected digit", start..i + 1));
    }
    let int_part = &b[start..i];
    // duration suffix: s|m|h|d|w (no decimal in durations)
    if i < b.len() && matches!(b[i], b's' | b'm' | b'h' | b'd' | b'w') {
        // Don't treat it as a duration if the next char would continue an
        // identifier (e.g. `5min` is not a duration; just bail to number+ident).
        let unit = b[i];
        let after = b.get(i + 1).copied();
        let unit_ends = after.is_none() || !is_ident_continue(after.unwrap());
        if unit_ends && b[start] != b'-' {
            let n: u64 = std::str::from_utf8(int_part)
                .ok()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| ParseError::new("invalid integer", start..i))?;
            let ms = match unit {
                b's' => n.saturating_mul(1_000),
                b'm' => n.saturating_mul(60_000),
                b'h' => n.saturating_mul(3_600_000),
                b'd' => n.saturating_mul(86_400_000),
                b'w' => n.saturating_mul(7 * 86_400_000),
                _ => unreachable!(),
            };
            return Ok((Token::Duration(ms), i + 1));
        }
    }
    // float?
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        let s = std::str::from_utf8(&b[start..i])
            .map_err(|_| ParseError::new("invalid number", start..i))?;
        let f: f64 = s
            .parse()
            .map_err(|_| ParseError::new("invalid number", start..i))?;
        return Ok((Token::Number(f), i));
    }
    let s = std::str::from_utf8(&b[start..i])
        .map_err(|_| ParseError::new("invalid number", start..i))?;
    let n: i64 = s
        .parse()
        .map_err(|_| ParseError::new("invalid integer", start..i))?;
    Ok((Token::Integer(n), i))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Token> {
        tokenize(s).unwrap().into_iter().map(|t| t.token).collect()
    }

    #[test]
    fn punctuation_and_strings() {
        let t = toks(r#"{server="arte"} |= "x""#);
        assert_eq!(
            t,
            vec![
                Token::LBrace,
                Token::Ident("server".into()),
                Token::Eq,
                Token::String("arte".into()),
                Token::RBrace,
                Token::PipeEq,
                Token::String("x".into()),
            ]
        );
    }

    #[test]
    fn duration_and_keywords() {
        let t = toks("count_over_time({server=\"a\"} [5m])");
        assert!(matches!(t[0], Token::Ident(ref s) if s == "count_over_time"));
        assert!(t.iter().any(|x| matches!(x, Token::Duration(300_000))));
    }

    #[test]
    fn alias_ref() {
        let t = toks("@plogs |= \"err\"");
        assert!(matches!(t[0], Token::AliasRef(ref s) if s == "plogs"));
    }

    #[test]
    fn regex_ops() {
        let t = toks("{a=~\"x\", b!~\"y\"}");
        assert!(t.contains(&Token::Re));
        assert!(t.contains(&Token::Nre));
    }

    #[test]
    fn float_and_int() {
        let t = toks("3.14 42");
        assert!(matches!(t[0], Token::Number(_)));
        assert!(matches!(t[1], Token::Integer(42)));
    }

    #[test]
    fn unterminated_string_errors() {
        let e = tokenize(r#""abc"#).unwrap_err();
        assert!(e.message.contains("unterminated"));
    }

    #[test]
    fn bare_bang_errors() {
        let e = tokenize("a ! b").unwrap_err();
        assert!(e.message.contains("!"));
    }
}
