//! Phase 10.3 — Go-style template engine.
//!
//! Implements a small but production-grade subset of Go's
//! `text/template` syntax that matches what Docker, kubectl, and gh
//! users (and LLM agents) already expect:
//!
//! * Field access: `{{.name}}` (resolves against the current record).
//! * Pipes: `{{.name | upper}}`, `{{.field | default "x"}}`.
//! * Functions: `upper`, `lower`, `len`, `default`, `truncate`,
//!   `pad`, `join`, `json`, `ago`.
//! * Conditionals: `{{if eq .x "y"}}...{{else}}...{{end}}` and
//!   `{{if ne .x "y"}}...{{end}}`.
//! * Escape sequences in literal text: `\t` and `\n`.
//!
//! Out-of-scope (intentional): user-defined functions, ranges, nested
//! field paths, parentheses, comments. These are easy follow-ups but
//! aren't needed by the bible's documented examples.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Result};
use serde_json::Value;

/// Parsed template ready for repeated application.
#[derive(Debug, Clone)]
pub struct Template {
    nodes: Vec<Node>,
}

#[derive(Debug, Clone)]
enum Node {
    Text(String),
    /// `{{.field | fn1 | fn2 "arg"}}`
    Expr(Expr),
    /// `{{if eq .x "y"}}...{{else}}...{{end}}`
    If {
        cond: Cond,
        then: Vec<Node>,
        otherwise: Vec<Node>,
    },
}

#[derive(Debug, Clone)]
struct Expr {
    /// `.field` or empty for `len`/`json`-style top-level pipes.
    field: Option<String>,
    pipes: Vec<Pipe>,
}

#[derive(Debug, Clone)]
struct Pipe {
    name: String,
    args: Vec<Arg>,
}

#[derive(Debug, Clone)]
enum Arg {
    Str(String),
    Num(i64),
}

#[derive(Debug, Clone)]
enum Cond {
    Eq(CondSide, CondSide),
    Ne(CondSide, CondSide),
}

#[derive(Debug, Clone)]
enum CondSide {
    Field(String),
    Lit(String),
}

impl Template {
    pub fn parse(src: &str) -> Result<Self> {
        let mut p = Parser::new(src);
        let nodes = p.parse_nodes(false)?;
        Ok(Template { nodes })
    }

    /// Render against a single record. Missing fields render as
    /// `<none>` (kubectl convention), per the bible.
    pub fn render(&self, record: &Value) -> Result<String> {
        let mut out = String::new();
        render_nodes(&self.nodes, record, &mut out)?;
        Ok(out)
    }
}

fn render_nodes(nodes: &[Node], rec: &Value, out: &mut String) -> Result<()> {
    for n in nodes {
        match n {
            Node::Text(s) => out.push_str(s),
            Node::Expr(e) => out.push_str(&eval_expr(e, rec)?),
            Node::If { cond, then, otherwise } => {
                if eval_cond(cond, rec) {
                    render_nodes(then, rec, out)?;
                } else {
                    render_nodes(otherwise, rec, out)?;
                }
            }
        }
    }
    Ok(())
}

fn eval_expr(e: &Expr, rec: &Value) -> Result<String> {
    let mut current: Value = match &e.field {
        Some(name) => lookup(rec, name),
        None => Value::Null,
    };
    for p in &e.pipes {
        current = apply_fn(&p.name, &p.args, current)?;
    }
    Ok(value_to_str(&current))
}

fn eval_cond(c: &Cond, rec: &Value) -> bool {
    let resolve = |s: &CondSide| -> String {
        match s {
            CondSide::Field(name) => value_to_str(&lookup(rec, name)),
            CondSide::Lit(s) => s.clone(),
        }
    };
    match c {
        Cond::Eq(a, b) => resolve(a) == resolve(b),
        Cond::Ne(a, b) => resolve(a) != resolve(b),
    }
}

fn lookup(rec: &Value, name: &str) -> Value {
    rec.get(name).cloned().unwrap_or(Value::Null)
}

fn value_to_str(v: &Value) -> String {
    match v {
        Value::Null => "<none>".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(a) => {
            // Default render: comma-joined for short arrays; users who
            // want a custom separator should pipe through `join "..."`.
            a.iter().map(value_to_str).collect::<Vec<_>>().join(",")
        }
        Value::Object(_) => serde_json::to_string(v).unwrap_or_default(),
    }
}

fn apply_fn(name: &str, args: &[Arg], v: Value) -> Result<Value> {
    let s = || value_to_str(&v);
    match name {
        "upper" => Ok(Value::String(s().to_uppercase())),
        "lower" => Ok(Value::String(s().to_lowercase())),
        "len" => match &v {
            Value::Array(a) => Ok(Value::Number(serde_json::Number::from(a.len() as u64))),
            Value::String(st) => Ok(Value::Number(serde_json::Number::from(st.chars().count() as u64))),
            Value::Null => Ok(Value::Number(serde_json::Number::from(0u64))),
            _ => Ok(Value::Number(serde_json::Number::from(0u64))),
        },
        "default" => {
            let fallback = match args.first() {
                Some(Arg::Str(s)) => s.clone(),
                Some(Arg::Num(n)) => n.to_string(),
                None => bail!("`default` requires one argument"),
            };
            if matches!(v, Value::Null) || matches!(&v, Value::String(s) if s.is_empty()) {
                Ok(Value::String(fallback))
            } else {
                Ok(v)
            }
        }
        "truncate" => {
            let n = match args.first() {
                Some(Arg::Num(n)) => *n as usize,
                _ => bail!("`truncate` requires an integer argument"),
            };
            let st = s();
            let truncated: String = st.chars().take(n).collect();
            Ok(Value::String(truncated))
        }
        "pad" => {
            let n = match args.first() {
                Some(Arg::Num(n)) => *n as usize,
                _ => bail!("`pad` requires an integer argument"),
            };
            let st = s();
            let pad = n.saturating_sub(st.chars().count());
            Ok(Value::String(format!("{st}{}", " ".repeat(pad))))
        }
        "join" => {
            let sep = match args.first() {
                Some(Arg::Str(s)) => s.clone(),
                _ => bail!("`join` requires a string argument"),
            };
            match &v {
                Value::Array(a) => Ok(Value::String(
                    a.iter().map(value_to_str).collect::<Vec<_>>().join(&sep),
                )),
                _ => Ok(v),
            }
        }
        "json" => Ok(Value::String(serde_json::to_string(&v).unwrap_or_default())),
        "ago" => {
            // Best-effort: accepts either a unix-seconds number or an
            // RFC3339 string and renders a coarse "Nh Mm" string. Falls
            // back to the original string for unparseable input so the
            // template still works when the source isn't a timestamp.
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let secs = match &v {
                Value::Number(n) => n.as_i64(),
                Value::String(st) => parse_rfc3339_secs(st),
                _ => None,
            };
            match secs {
                Some(then) if now >= then => {
                    Ok(Value::String(human_duration(now - then)))
                }
                _ => Ok(Value::String(s())),
            }
        }
        other => Err(anyhow!("unknown template function `{other}`")),
    }
}

fn parse_rfc3339_secs(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.timestamp())
}

fn human_duration(mut secs: i64) -> String {
    if secs < 60 {
        return format!("{secs}s");
    }
    let mut out = String::new();
    let days = secs / 86_400;
    secs %= 86_400;
    let hours = secs / 3_600;
    secs %= 3_600;
    let mins = secs / 60;
    if days > 0 {
        out.push_str(&format!("{days}d "));
    }
    if hours > 0 || days > 0 {
        out.push_str(&format!("{hours}h "));
    }
    out.push_str(&format!("{mins}m"));
    out
}

// -----------------------------------------------------------------------------
// Parser
// -----------------------------------------------------------------------------

struct Parser<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }

    fn parse_nodes(&mut self, in_branch: bool) -> Result<Vec<Node>> {
        let mut out = Vec::new();
        while self.pos < self.src.len() {
            // Look for `{{`.
            if self.starts_with("{{") {
                // Peek for `else`/`end` to terminate a branch.
                let save = self.pos;
                self.pos += 2;
                self.skip_ws();
                if in_branch && (self.starts_with("else") || self.starts_with("end")) {
                    self.pos = save;
                    return Ok(out);
                }
                self.pos = save;
                let action = self.parse_action()?;
                match action {
                    Action::Expr(e) => out.push(Node::Expr(e)),
                    Action::If(cond) => {
                        let then = self.parse_nodes(true)?;
                        let otherwise = if self.consume_action_kw("else") {
                            self.parse_nodes(true)?
                        } else {
                            Vec::new()
                        };
                        if !self.consume_action_kw("end") {
                            bail!("template: missing `{{end}}`");
                        }
                        out.push(Node::If { cond, then, otherwise });
                    }
                }
            } else {
                let mut buf = String::new();
                while self.pos < self.src.len() && !self.starts_with("{{") {
                    let c = self.src.as_bytes()[self.pos] as char;
                    if c == '\\' && self.pos + 1 < self.src.len() {
                        let nxt = self.src.as_bytes()[self.pos + 1] as char;
                        match nxt {
                            'n' => { buf.push('\n'); self.pos += 2; continue; }
                            't' => { buf.push('\t'); self.pos += 2; continue; }
                            '\\' => { buf.push('\\'); self.pos += 2; continue; }
                            _ => {}
                        }
                    }
                    buf.push(c);
                    self.pos += 1;
                }
                if !buf.is_empty() {
                    out.push(Node::Text(buf));
                }
            }
        }
        Ok(out)
    }

    fn parse_action(&mut self) -> Result<Action> {
        // Currently at `{{`.
        self.pos += 2;
        self.skip_ws();
        if self.starts_with("if") && self.peek_after("if").map(|c| c.is_whitespace()).unwrap_or(false) {
            self.pos += 2;
            self.skip_ws();
            let cond = self.parse_cond()?;
            self.skip_ws();
            if !self.consume("}}") {
                bail!("template: expected `}}}}` after `if` action");
            }
            return Ok(Action::If(cond));
        }
        // Otherwise: an expression with optional pipes.
        let field = if self.starts_with(".") {
            self.pos += 1;
            Some(self.read_ident())
        } else {
            None
        };
        let mut pipes = Vec::new();
        loop {
            self.skip_ws();
            if self.consume("}}") {
                break;
            }
            if !self.consume("|") {
                // Could also be a function call without a leading field
                // (e.g. `{{ len .x }}` is technically valid Go syntax,
                // but we've already chosen pipe-style for simplicity).
                bail!("template: expected `|` or `}}}}`");
            }
            self.skip_ws();
            let name = self.read_ident();
            if name.is_empty() {
                bail!("template: expected pipe function name");
            }
            let mut args = Vec::new();
            loop {
                self.skip_ws();
                if self.starts_with("|") || self.starts_with("}}") {
                    break;
                }
                args.push(self.read_arg()?);
            }
            pipes.push(Pipe { name, args });
        }
        Ok(Action::Expr(Expr { field, pipes }))
    }

    fn parse_cond(&mut self) -> Result<Cond> {
        let kw = self.read_ident();
        self.skip_ws();
        let lhs = self.read_cond_side()?;
        self.skip_ws();
        let rhs = self.read_cond_side()?;
        match kw.as_str() {
            "eq" => Ok(Cond::Eq(lhs, rhs)),
            "ne" => Ok(Cond::Ne(lhs, rhs)),
            other => Err(anyhow!(
                "template: unknown comparison `{other}` (expected `eq` or `ne`)"
            )),
        }
    }

    fn read_cond_side(&mut self) -> Result<CondSide> {
        if self.starts_with(".") {
            self.pos += 1;
            return Ok(CondSide::Field(self.read_ident()));
        }
        if self.starts_with("\"") {
            return Ok(CondSide::Lit(self.read_quoted_string()?));
        }
        bail!("template: expected `.field` or quoted literal in condition")
    }

    fn read_arg(&mut self) -> Result<Arg> {
        if self.starts_with("\"") {
            return Ok(Arg::Str(self.read_quoted_string()?));
        }
        // Number literal.
        let start = self.pos;
        let mut neg = false;
        if self.starts_with("-") {
            neg = true;
            self.pos += 1;
        }
        while self.pos < self.src.len() && self.src.as_bytes()[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        if self.pos == start || (neg && self.pos == start + 1) {
            bail!("template: expected argument literal");
        }
        let lit: i64 = self.src[start..self.pos]
            .parse()
            .map_err(|_| anyhow!("template: bad number literal"))?;
        Ok(Arg::Num(lit))
    }

    fn read_quoted_string(&mut self) -> Result<String> {
        // Currently at `"`.
        self.pos += 1;
        let mut out = String::new();
        while self.pos < self.src.len() {
            let c = self.src.as_bytes()[self.pos] as char;
            if c == '\\' && self.pos + 1 < self.src.len() {
                let nxt = self.src.as_bytes()[self.pos + 1] as char;
                match nxt {
                    '"' => { out.push('"'); self.pos += 2; continue; }
                    '\\' => { out.push('\\'); self.pos += 2; continue; }
                    'n' => { out.push('\n'); self.pos += 2; continue; }
                    't' => { out.push('\t'); self.pos += 2; continue; }
                    _ => { out.push(nxt); self.pos += 2; continue; }
                }
            }
            if c == '"' {
                self.pos += 1;
                return Ok(out);
            }
            out.push(c);
            self.pos += 1;
        }
        bail!("template: unterminated string literal")
    }

    fn read_ident(&mut self) -> String {
        let start = self.pos;
        while self.pos < self.src.len() {
            let b = self.src.as_bytes()[self.pos];
            if b.is_ascii_alphanumeric() || b == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        self.src[start..self.pos].to_string()
    }

    fn skip_ws(&mut self) {
        while self.pos < self.src.len()
            && self.src.as_bytes()[self.pos].is_ascii_whitespace()
        {
            self.pos += 1;
        }
    }

    fn starts_with(&self, s: &str) -> bool {
        self.src[self.pos..].starts_with(s)
    }

    fn peek_after(&self, s: &str) -> Option<char> {
        let i = self.pos + s.len();
        self.src[i..].chars().next()
    }

    fn consume(&mut self, s: &str) -> bool {
        if self.starts_with(s) {
            self.pos += s.len();
            true
        } else {
            false
        }
    }

    /// Consume `{{ <kw> }}` if present.
    fn consume_action_kw(&mut self, kw: &str) -> bool {
        let save = self.pos;
        if !self.consume("{{") {
            return false;
        }
        self.skip_ws();
        if !self.consume(kw) {
            self.pos = save;
            return false;
        }
        self.skip_ws();
        if !self.consume("}}") {
            self.pos = save;
            return false;
        }
        true
    }
}

enum Action {
    Expr(Expr),
    If(Cond),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn render(t: &str, rec: Value) -> String {
        Template::parse(t).unwrap().render(&rec).unwrap()
    }

    #[test]
    fn field_access() {
        assert_eq!(render("{{.name}}", json!({"name": "pulse"})), "pulse");
    }

    #[test]
    fn missing_field_renders_none() {
        assert_eq!(render("{{.missing}}", json!({})), "<none>");
    }

    #[test]
    fn pipe_upper_lower() {
        assert_eq!(render("{{.x | upper}}", json!({"x": "ab"})), "AB");
        assert_eq!(render("{{.x | lower}}", json!({"x": "AB"})), "ab");
    }

    #[test]
    fn pipe_default() {
        assert_eq!(render(r#"{{.x | default "z"}}"#, json!({"x": null})), "z");
        assert_eq!(render(r#"{{.x | default "z"}}"#, json!({"x": "y"})), "y");
    }

    #[test]
    fn pipe_truncate_pad() {
        assert_eq!(render("{{.x | truncate 3}}", json!({"x": "hello"})), "hel");
        assert_eq!(render("{{.x | pad 5}}", json!({"x": "hi"})), "hi   ");
    }

    #[test]
    fn pipe_join() {
        assert_eq!(
            render(r#"{{.ports | join ","}}"#, json!({"ports": ["a", "b", "c"]})),
            "a,b,c"
        );
    }

    #[test]
    fn pipe_len() {
        assert_eq!(render("{{.x | len}}", json!({"x": [1, 2, 3]})), "3");
        assert_eq!(render("{{.x | len}}", json!({"x": "abcd"})), "4");
    }

    #[test]
    fn if_eq_branch() {
        let tpl = r#"{{if eq .h "down"}}ALERT: {{.s}}{{end}}"#;
        assert_eq!(render(tpl, json!({"h": "down", "s": "pulse"})), "ALERT: pulse");
        assert_eq!(render(tpl, json!({"h": "ok", "s": "pulse"})), "");
    }

    #[test]
    fn if_else() {
        let tpl = r#"{{if eq .h "ok"}}OK{{else}}BAD{{end}}"#;
        assert_eq!(render(tpl, json!({"h": "ok"})), "OK");
        assert_eq!(render(tpl, json!({"h": "down"})), "BAD");
    }

    #[test]
    fn text_with_escape_sequences() {
        assert_eq!(
            render(r"{{.a}}\t{{.b}}", json!({"a": "x", "b": "y"})),
            "x\ty"
        );
    }

    #[test]
    fn unterminated_string_errors() {
        assert!(Template::parse(r#"{{.x | default "oops}}"#).is_err());
    }

    #[test]
    fn array_default_render_is_comma_joined() {
        assert_eq!(render("{{.p}}", json!({"p": [1, 2]})), "1,2");
    }
}
