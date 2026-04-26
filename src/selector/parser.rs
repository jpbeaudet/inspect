//! Selector parser. Hand-written, char-driven, no DSL dependency.
//!
//! The grammar is small enough (server + service + path) that a
//! straight-line parser is more readable, gives better error spans, and
//! avoids pulling in `chumsky` (reserved for Phase 6 LogQL).

use thiserror::Error;

use super::ast::{PathSpec, Selector, ServerAtom, ServerSpec, ServiceAtom, ServiceSpec};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SelectorParseError {
    #[error("selector is empty")]
    Empty,

    #[error(
        "selector '{0}' looks like a LogQL selector ({{server=...}}); use 'inspect search' for LogQL queries, \
         or rewrite the selector in verb form (e.g. 'arte/pulse')"
    )]
    LookLikeLogQl(String),

    #[error("alias '@{0}' is not defined; run 'inspect alias list' to see available aliases")]
    UnknownAlias(String),

    #[error(
        "alias '@{name}' expansion would loop or chain (alias '{target}' references another alias); \
         alias chaining is not supported in v1"
    )]
    #[allow(dead_code)] // v2: parameterized-aliases — variant is constructed once chained alias resolution lands.
    AliasChain { name: String, target: String },

    #[error("server portion of selector '{0}' is empty")]
    EmptyServer(String),

    #[error("service portion of selector '{0}' is empty")]
    EmptyService(String),

    #[error("path portion of selector '{0}' is empty (write 'arte/foo' if you don't need a path)")]
    EmptyPath(String),

    #[error("regex '{0}' is missing a closing '/'")]
    UnterminatedRegex(String),

    #[error("invalid selector character '{ch}' in '{src}' at position {pos}")]
    InvalidChar { src: String, ch: char, pos: usize },
}

/// Parse a textual selector into an [`Selector`].
///
/// **Aliases** are resolved by [`super::resolve::resolve`] before this is
/// called; if a leading `@` reaches this function it's an error.
pub fn parse_selector(input: &str) -> Result<Selector, SelectorParseError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(SelectorParseError::Empty);
    }
    if trimmed.starts_with('{') {
        return Err(SelectorParseError::LookLikeLogQl(trimmed.to_string()));
    }
    if let Some(rest) = trimmed.strip_prefix('@') {
        return Err(SelectorParseError::UnknownAlias(rest.to_string()));
    }

    // Three forms:
    //   bare server         → "arte"
    //   server/service      → "arte/pulse"
    //   server[/service]:path → "arte/_:/var/log/x" or "arte:/var/log/x"
    //
    // Tricky bit: a regex service atom uses `/` as its delimiter, so we
    // must split on the FIRST top-level slash for server/service, and on
    // the FIRST colon that is OUTSIDE any regex `/.../` for service/path.

    // 1) server/service split: first slash from the start.
    let (server_str, after_slash): (&str, Option<&str>) =
        match trimmed.find('/') {
            Some(i) => (&trimmed[..i], Some(&trimmed[i + 1..])),
            None => (trimmed, None),
        };
    if server_str.is_empty() {
        return Err(SelectorParseError::EmptyServer(trimmed.to_string()));
    }

    // 2) service/path split: first colon outside a regex.
    let (service_str, path_str): (Option<&str>, Option<&str>) = match after_slash {
        Some(rest) => match split_outside_regex(rest, ':') {
            Some((svc, p)) => (Some(svc), Some(p)),
            None => (Some(rest), None),
        },
        None => {
            // Bare-server form: allow `arte:/var/log/syslog` as shorthand
            // for `arte/_:/var/log/syslog`. There is no service portion to
            // worry about here, so a top-level colon is unambiguous.
            match server_str.find(':') {
                Some(i) => {
                    // Re-split: server is everything before the first ':',
                    // path is everything after, service is implicit `_`.
                    let (s, p) = (&server_str[..i], &server_str[i + 1..]);
                    return Ok(Selector {
                        server: parse_server(s)?,
                        service: Some(ServiceSpec::Host),
                        path: if p.is_empty() {
                            return Err(SelectorParseError::EmptyPath(trimmed.to_string()));
                        } else {
                            Some(PathSpec(p.to_string()))
                        },
                        source: trimmed.to_string(),
                    });
                }
                None => (None, None),
            }
        }
    };

    if let Some(svc) = service_str {
        if svc.is_empty() {
            return Err(SelectorParseError::EmptyService(trimmed.to_string()));
        }
    }
    if let Some(p) = path_str {
        if p.is_empty() {
            return Err(SelectorParseError::EmptyPath(trimmed.to_string()));
        }
    }

    Ok(Selector {
        server: parse_server(server_str)?,
        service: service_str.map(parse_service).transpose()?,
        path: path_str.map(|p| PathSpec(p.to_string())),
        source: trimmed.to_string(),
    })
}

/// Split on the first occurrence of `sep` that lies OUTSIDE a `/.../` regex
/// pair. Used for the service/path boundary, where the service may itself
/// contain a regex like `/milvus-\d+/`.
fn split_outside_regex(s: &str, sep: char) -> Option<(&str, &str)> {
    let mut in_regex = false;
    let mut prev = '\0';
    for (i, ch) in s.char_indices() {
        if ch == '/' && prev != '\\' {
            in_regex = !in_regex;
        }
        if !in_regex && ch == sep {
            return Some((&s[..i], &s[i + ch.len_utf8()..]));
        }
        prev = ch;
    }
    None
}

fn parse_server(s: &str) -> Result<ServerSpec, SelectorParseError> {
    if s == "all" {
        return Ok(ServerSpec::All);
    }
    let mut atoms = Vec::new();
    for raw in s.split(',') {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(SelectorParseError::EmptyServer(s.to_string()));
        }
        validate_pattern_chars(raw, /*allow_regex=*/ false)?;
        atoms.push(if let Some(rest) = raw.strip_prefix('~') {
            if rest.is_empty() {
                return Err(SelectorParseError::EmptyServer(s.to_string()));
            }
            ServerAtom::Exclude(rest.to_string())
        } else {
            ServerAtom::Pattern(raw.to_string())
        });
    }
    if atoms.is_empty() {
        return Err(SelectorParseError::EmptyServer(s.to_string()));
    }
    Ok(ServerSpec::Atoms(atoms))
}

fn parse_service(s: &str) -> Result<ServiceSpec, SelectorParseError> {
    if s == "_" {
        return Ok(ServiceSpec::Host);
    }
    if s == "*" {
        return Ok(ServiceSpec::All);
    }
    // A regex atom must be the WHOLE service-spec; we don't allow mixing
    // regex atoms with comma-separated others (rare and ambiguous).
    if let Some(rest) = s.strip_prefix('/') {
        let body = rest
            .strip_suffix('/')
            .ok_or_else(|| SelectorParseError::UnterminatedRegex(s.to_string()))?;
        return Ok(ServiceSpec::Atoms(vec![ServiceAtom::Regex(body.to_string())]));
    }
    let mut atoms = Vec::new();
    for raw in s.split(',') {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(SelectorParseError::EmptyService(s.to_string()));
        }
        validate_pattern_chars(raw, /*allow_regex=*/ false)?;
        atoms.push(if let Some(rest) = raw.strip_prefix('~') {
            if rest.is_empty() {
                return Err(SelectorParseError::EmptyService(s.to_string()));
            }
            ServiceAtom::Exclude(rest.to_string())
        } else {
            ServiceAtom::Pattern(raw.to_string())
        });
    }
    if atoms.is_empty() {
        return Err(SelectorParseError::EmptyService(s.to_string()));
    }
    Ok(ServiceSpec::Atoms(atoms))
}

/// Pattern atoms allow alphanum, `_`, `-`, `.`, glob `*?[]`. Forbidden
/// characters: whitespace, control chars, etc.
fn validate_pattern_chars(s: &str, allow_regex: bool) -> Result<(), SelectorParseError> {
    for (i, ch) in s.char_indices() {
        let ok = ch.is_ascii_alphanumeric()
            || matches!(ch, '_' | '-' | '.' | '*' | '?' | '[' | ']' | '~')
            || (allow_regex && ch != '/');
        if !ok {
            return Err(SelectorParseError::InvalidChar {
                src: s.to_string(),
                ch,
                pos: i,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Selector {
        parse_selector(s).unwrap()
    }

    #[test]
    fn bare_server() {
        let p = parse("arte");
        assert!(matches!(p.server, ServerSpec::Atoms(ref a) if a.len() == 1));
        assert!(p.service.is_none());
        assert!(p.path.is_none());
    }

    #[test]
    fn server_slash_service() {
        let p = parse("arte/pulse");
        match p.service.unwrap() {
            ServiceSpec::Atoms(a) => assert_eq!(a.len(), 1),
            _ => panic!(),
        }
    }

    #[test]
    fn comma_lists() {
        let p = parse("arte,prod/pulse,atlas");
        match (&p.server, p.service.as_ref().unwrap()) {
            (ServerSpec::Atoms(s), ServiceSpec::Atoms(svc)) => {
                assert_eq!(s.len(), 2);
                assert_eq!(svc.len(), 2);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn glob_and_regex() {
        let p = parse("prod-*/storage");
        let _ = p;
        let p = parse("arte//milvus-\\d+/");
        match p.service.unwrap() {
            ServiceSpec::Atoms(a) => assert!(matches!(a[0], ServiceAtom::Regex(_))),
            _ => panic!(),
        }
    }

    #[test]
    fn host_level_and_path() {
        let p = parse("arte/_:/var/log/syslog");
        assert!(matches!(p.service, Some(ServiceSpec::Host)));
        assert_eq!(p.path.unwrap().0, "/var/log/syslog");
    }

    #[test]
    fn star_service() {
        let p = parse("arte/*");
        assert!(matches!(p.service, Some(ServiceSpec::All)));
    }

    #[test]
    fn all_keyword() {
        let p = parse("all");
        assert!(matches!(p.server, ServerSpec::All));
    }

    #[test]
    fn subtractive() {
        let p = parse("~staging");
        match p.server {
            ServerSpec::Atoms(a) => assert!(matches!(a[0], ServerAtom::Exclude(_))),
            _ => panic!(),
        }
    }

    #[test]
    fn rejects_logql() {
        assert!(matches!(
            parse_selector("{server=\"arte\"}"),
            Err(SelectorParseError::LookLikeLogQl(_))
        ));
    }

    #[test]
    fn rejects_alias_marker() {
        assert!(matches!(
            parse_selector("@plogs"),
            Err(SelectorParseError::UnknownAlias(_))
        ));
    }

    #[test]
    fn rejects_empty_pieces() {
        assert!(parse_selector("").is_err());
        assert!(parse_selector("arte/").is_err());
        assert!(parse_selector("arte/pulse:").is_err());
        assert!(parse_selector("/pulse").is_err());
    }
}
