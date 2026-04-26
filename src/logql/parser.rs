//! Recursive-descent parser for LogQL (bible §9.13).

use std::ops::Range;

use super::ast::*;
use super::error::ParseError;
use super::lexer::{Spanned, Token};

pub fn parse_tokens(tokens: &[Spanned], src: &str) -> Result<Query, ParseError> {
    let mut p = Parser {
        toks: tokens,
        idx: 0,
        src_len: src.len(),
    };
    let q = p.parse_query()?;
    if !p.at_end() {
        let span = p.cur_span();
        return Err(ParseError::new(
            format!("unexpected trailing input: {}", p.cur_repr()),
            span,
        )
        .with_hint("a query is one selector union, optional pipeline stages, optionally wrapped in a metric function"));
    }
    Ok(q)
}

struct Parser<'a> {
    toks: &'a [Spanned],
    idx: usize,
    src_len: usize,
}

impl<'a> Parser<'a> {
    // ----- token helpers ---------------------------------------------------
    fn at_end(&self) -> bool {
        self.idx >= self.toks.len()
    }
    fn peek(&self) -> Option<&'a Token> {
        self.toks.get(self.idx).map(|t| &t.token)
    }
    fn peek_n(&self, n: usize) -> Option<&'a Token> {
        self.toks.get(self.idx + n).map(|t| &t.token)
    }
    fn bump(&mut self) -> Option<&'a Spanned> {
        let t = self.toks.get(self.idx);
        if t.is_some() {
            self.idx += 1;
        }
        t
    }
    fn cur_span(&self) -> Range<usize> {
        match self.toks.get(self.idx) {
            Some(t) => t.span.clone(),
            None => self.src_len..self.src_len,
        }
    }
    fn cur_repr(&self) -> String {
        match self.peek() {
            None => "end of input".into(),
            Some(t) => t.display(),
        }
    }
    fn expect(&mut self, want: &Token, what: &str) -> Result<Range<usize>, ParseError> {
        match self.peek() {
            Some(t) if std::mem::discriminant(t) == std::mem::discriminant(want) => {
                // Safe: peek() just returned Some matching `want`, so
                // bump() will return the same token.
                Ok(self
                    .bump()
                    .expect("BUG: peek matched but bump returned None")
                    .span
                    .clone())
            }
            _ => Err(ParseError::new(
                format!("expected {what}, found {}", self.cur_repr()),
                self.cur_span(),
            )),
        }
    }
    fn eat(&mut self, want: &Token) -> bool {
        match self.peek() {
            Some(t) if std::mem::discriminant(t) == std::mem::discriminant(want) => {
                self.bump();
                true
            }
            _ => false,
        }
    }

    // ----- entry point -----------------------------------------------------
    fn parse_query(&mut self) -> Result<Query, ParseError> {
        // Decide log vs metric by top-level lookahead. A metric query
        // always begins with an aggregation function identifier followed
        // by `(` or `by`/`without`. Anything else is a log query.
        if self.looks_like_metric() {
            Ok(Query::Metric(self.parse_metric_query()?))
        } else {
            Ok(Query::Log(self.parse_log_query()?))
        }
    }

    fn looks_like_metric(&self) -> bool {
        let Some(Token::Ident(s)) = self.peek() else {
            return false;
        };
        if AggFn::from_ident(s).is_some() {
            // followed by `(` or `by/without (`
            return matches!(
                self.peek_n(1),
                Some(Token::LParen) | Some(Token::KwBy) | Some(Token::KwWithout)
            );
        }
        if RangeFn::from_ident(s).is_some() {
            return matches!(self.peek_n(1), Some(Token::LParen));
        }
        false
    }

    // ----- log query -------------------------------------------------------
    fn parse_log_query(&mut self) -> Result<LogQuery, ParseError> {
        let start = self.cur_span().start;
        let selector = self.parse_selector_union()?;
        let mut pipeline = Vec::new();
        loop {
            match self.peek() {
                Some(Token::PipeEq) | Some(Token::PipeRe) => {
                    pipeline.push(PipelineOp::Filter(self.parse_filter()?));
                }
                // `!= "x"` (line filter) vs `!= number` (field cmp): only valid as
                // line filter at top of pipeline when followed by a string.
                Some(Token::Ne) if matches!(self.peek_n(1), Some(Token::String(_))) => {
                    pipeline.push(PipelineOp::Filter(self.parse_filter()?));
                }
                Some(Token::Nre) if matches!(self.peek_n(1), Some(Token::String(_))) => {
                    pipeline.push(PipelineOp::Filter(self.parse_filter()?));
                }
                Some(Token::Pipe) => {
                    pipeline.push(PipelineOp::Stage(self.parse_stage()?));
                }
                _ => break,
            }
        }
        let end = self
            .toks
            .get(self.idx.saturating_sub(1))
            .map(|t| t.span.end)
            .unwrap_or(start);
        Ok(LogQuery {
            selector,
            pipeline,
            span: start..end,
        })
    }

    fn parse_selector_union(&mut self) -> Result<SelectorUnion, ParseError> {
        let start = self.cur_span().start;
        let first = self.parse_selector()?;
        let mut branches = vec![first];
        while matches!(self.peek(), Some(Token::KwOr))
            && matches!(
                self.peek_n(1),
                Some(Token::LBrace) | Some(Token::AliasRef(_))
            )
        {
            self.bump(); // consume `or`
            branches.push(self.parse_selector()?);
        }
        let end = branches
            .last()
            .expect("BUG: parse_selector_union always pushes at least one branch")
            .span
            .end;
        Ok(SelectorUnion {
            branches,
            span: start..end,
        })
    }

    fn parse_selector(&mut self) -> Result<Selector, ParseError> {
        // alias references are expanded before parsing, so a stray `@`
        // here means the alias resolver returned None.
        if let Some(Token::AliasRef(name)) = self.peek() {
            let span = self.cur_span();
            return Err(ParseError::new(format!("unknown alias `@{name}`"), span)
                .with_hint("define it via `inspect alias add` or check the name"));
        }
        let lbrace = self.expect(&Token::LBrace, "`{` to begin a selector").map_err(|e| {
            e.with_hint("a query starts with a selector, e.g. `{server=\"arte\", source=\"logs\"} |= \"error\"`")
        })?;
        let mut matchers = Vec::new();
        if !matches!(self.peek(), Some(Token::RBrace)) {
            loop {
                matchers.push(self.parse_label_matcher()?);
                if self.eat(&Token::Comma) {
                    continue;
                }
                break;
            }
        }
        let rbrace_span = self
            .expect(&Token::RBrace, "`}` to close the selector")
            .map_err(|e| {
                e.with_hint(
                    "label matchers are comma-separated, e.g. `{server=\"arte\", source=\"logs\"}`",
                )
            })?;
        Ok(Selector {
            matchers,
            span: lbrace.start..rbrace_span.end,
        })
    }

    fn parse_label_matcher(&mut self) -> Result<LabelMatcher, ParseError> {
        let name_tok = self
            .bump()
            .ok_or_else(|| ParseError::new("expected label name", self.cur_span()))?;
        let name = match &name_tok.token {
            Token::Ident(s) => s.clone(),
            Token::KwOr => "or".into(),
            Token::KwAnd => "and".into(),
            Token::KwBy => "by".into(),
            Token::KwWithout => "without".into(),
            Token::KwNot => "not".into(),
            _ => {
                return Err(ParseError::new(
                    format!("expected label name, found {}", name_tok.token.display()),
                    name_tok.span.clone(),
                )
                .with_hint("label names look like `server`, `service`, `source`, `path`, …"));
            }
        };
        let name_span = name_tok.span.clone();
        let op = match self.peek() {
            Some(Token::Eq) => MatchOp::Eq,
            Some(Token::Ne) => MatchOp::Ne,
            Some(Token::Re) => MatchOp::Re,
            Some(Token::Nre) => MatchOp::Nre,
            _ => {
                return Err(ParseError::new(
                    format!(
                        "expected one of `=`, `!=`, `=~`, `!~` after label name, found {}",
                        self.cur_repr()
                    ),
                    self.cur_span(),
                )
                .with_hint("label matchers look like `server=\"arte\"` or `path=~\".*\\.log\"`"));
            }
        };
        self.bump();
        let val_tok = self.bump().ok_or_else(|| {
            ParseError::new(
                "expected a quoted string value after the match operator",
                self.cur_span(),
            )
            .with_hint("e.g. `server=\"arte\"`")
        })?;
        let value = match &val_tok.token {
            Token::String(s) => s.clone(),
            _ => {
                return Err(ParseError::new(
                    format!(
                        "expected a quoted string value, found {}",
                        val_tok.token.display()
                    ),
                    val_tok.span.clone(),
                )
                .with_hint("label values must be double-quoted, e.g. `service=\"api\"`"));
            }
        };
        Ok(LabelMatcher {
            name,
            op,
            value,
            name_span,
            value_span: val_tok.span.clone(),
        })
    }

    fn parse_filter(&mut self) -> Result<Filter, ParseError> {
        // Safe: callers only enter parse_filter after peeking one of
        // PipeEq/Ne/PipeRe/Nre, so bump() must succeed.
        let tok = self
            .bump()
            .expect("BUG: parse_filter entered without a filter token");
        let op = match tok.token {
            Token::PipeEq => FilterOp::Contains,
            Token::Ne => FilterOp::NotContains,
            Token::PipeRe => FilterOp::Re,
            Token::Nre => FilterOp::Nre,
            _ => unreachable!("parse_filter precondition"),
        };
        let pat_tok = self.bump().ok_or_else(|| {
            ParseError::new(
                format!("expected a quoted string after `{}`", op.as_str()),
                self.cur_span(),
            )
            .with_hint("line filters look like `|= \"error\"` or `|~ \"5\\d\\d\"`")
        })?;
        let pattern = match &pat_tok.token {
            Token::String(s) => s.clone(),
            _ => {
                return Err(ParseError::new(
                    format!(
                        "expected a quoted string after `{}`, found {}",
                        op.as_str(),
                        pat_tok.token.display()
                    ),
                    pat_tok.span.clone(),
                )
                .with_hint("line filters look like `|= \"error\"` or `|~ \"5\\d\\d\"`"));
            }
        };
        Ok(Filter {
            op,
            pattern,
            span: tok.span.start..pat_tok.span.end,
        })
    }

    fn parse_stage(&mut self) -> Result<Stage, ParseError> {
        let pipe = self.expect(&Token::Pipe, "`|`")?;
        let name_tok = self.bump().ok_or_else(|| {
            ParseError::new("expected stage name after `|`", pipe.clone())
                .with_hint("stages: `json`, `logfmt`, `pattern`, `regexp`, `line_format`, `label_format`, `drop`, `keep`, `map`, or a parsed-field filter like `| status >= 500`")
        })?;
        let name = match &name_tok.token {
            Token::Ident(s) => s.clone(),
            _ => {
                return Err(ParseError::new(
                    format!("expected stage name after `|`, found {}", name_tok.token.display()),
                    name_tok.span.clone(),
                )
                .with_hint("stages: `json`, `logfmt`, `pattern`, `regexp`, `line_format`, `label_format`, `drop`, `keep`, `map`, or a parsed-field filter"));
            }
        };
        let span_start = pipe.start;
        match name.as_str() {
            "json" => Ok(Stage::Json),
            "logfmt" => Ok(Stage::Logfmt),
            "pattern" => {
                let s = self.expect_string("pattern template")?;
                let end = self
                    .toks
                    .get(self.idx.saturating_sub(1))
                    .map(|t| t.span.end)
                    .unwrap_or(span_start);
                Ok(Stage::Pattern {
                    template: s,
                    span: span_start..end,
                })
            }
            "regexp" => {
                let s = self.expect_string("regexp pattern")?;
                let end = self.last_end(span_start);
                Ok(Stage::Regexp {
                    pattern: s,
                    span: span_start..end,
                })
            }
            "line_format" => {
                let s = self.expect_string("line_format template")?;
                let end = self.last_end(span_start);
                Ok(Stage::LineFormat {
                    template: s,
                    span: span_start..end,
                })
            }
            "label_format" => {
                let mut assigns = Vec::new();
                loop {
                    let n_tok = self.bump().ok_or_else(|| {
                        ParseError::new(
                            "expected `name=\"template\"` after label_format",
                            self.cur_span(),
                        )
                    })?;
                    let n = match &n_tok.token {
                        Token::Ident(s) => s.clone(),
                        _ => {
                            return Err(ParseError::new(
                                "expected label name in label_format",
                                n_tok.span.clone(),
                            ));
                        }
                    };
                    self.expect(&Token::Eq, "`=`")?;
                    let tpl = self.expect_string("template string")?;
                    assigns.push(LabelAssign {
                        name: n,
                        template: tpl,
                    });
                    if !self.eat(&Token::Comma) {
                        break;
                    }
                }
                let end = self.last_end(span_start);
                Ok(Stage::LabelFormat {
                    assignments: assigns,
                    span: span_start..end,
                })
            }
            "drop" => {
                let labels = self.parse_label_list()?;
                let end = self.last_end(span_start);
                Ok(Stage::Drop {
                    labels,
                    span: span_start..end,
                })
            }
            "keep" => {
                let labels = self.parse_label_list()?;
                let end = self.last_end(span_start);
                Ok(Stage::Keep {
                    labels,
                    span: span_start..end,
                })
            }
            "map" => {
                self.expect(&Token::LBrace, "`{` after `map`")?;
                // Sub-query: a log_query expression terminated by `}`.
                let sub = self.parse_log_query()?;
                self.expect(&Token::RBrace, "`}` to close `map { ... }`")?;
                let end = self.last_end(span_start);
                Ok(Stage::Map {
                    sub: Box::new(sub),
                    span: span_start..end,
                })
            }
            // Anything else: a parsed-field filter `| <field> <op> <value> [and|or ...]`
            // with the field consumed as `name`.
            other => {
                let expr =
                    self.parse_field_filter_starting(other.to_string(), name_tok.span.clone())?;
                let end = self.last_end(span_start);
                Ok(Stage::FieldFilter {
                    expr,
                    span: span_start..end,
                })
            }
        }
    }

    fn last_end(&self, fallback: usize) -> usize {
        self.toks
            .get(self.idx.saturating_sub(1))
            .map(|t| t.span.end)
            .unwrap_or(fallback)
    }

    fn expect_string(&mut self, what: &str) -> Result<String, ParseError> {
        let tok = self
            .bump()
            .ok_or_else(|| ParseError::new(format!("expected {what}"), self.cur_span()))?;
        match &tok.token {
            Token::String(s) => Ok(s.clone()),
            _ => Err(ParseError::new(
                format!("expected {what}, found {}", tok.token.display()),
                tok.span.clone(),
            )),
        }
    }

    fn parse_label_list(&mut self) -> Result<Vec<String>, ParseError> {
        let mut out = Vec::new();
        loop {
            let tok = self
                .bump()
                .ok_or_else(|| ParseError::new("expected label name", self.cur_span()))?;
            match &tok.token {
                Token::Ident(s) => out.push(s.clone()),
                _ => {
                    return Err(ParseError::new(
                        format!("expected label name, found {}", tok.token.display()),
                        tok.span.clone(),
                    ));
                }
            }
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        Ok(out)
    }

    // ----- parsed-field filter expression ---------------------------------
    //
    // Grammar (precedence): or > and > not > primary
    fn parse_field_filter_starting(
        &mut self,
        first_field: String,
        _first_span: Range<usize>,
    ) -> Result<FieldExpr, ParseError> {
        let head = self.parse_field_cmp_with_field(first_field)?;
        self.continue_field_or(head)
    }
    fn continue_field_or(&mut self, mut left: FieldExpr) -> Result<FieldExpr, ParseError> {
        loop {
            // and binds tighter than or
            left = self.continue_field_and(left)?;
            if self.eat(&Token::KwOr) {
                let right_first = self.parse_field_primary()?;
                let right = self.continue_field_and(right_first)?;
                left = FieldExpr::Or(Box::new(left), Box::new(right));
            } else {
                return Ok(left);
            }
        }
    }
    fn continue_field_and(&mut self, mut left: FieldExpr) -> Result<FieldExpr, ParseError> {
        while self.eat(&Token::KwAnd) {
            let right = self.parse_field_primary()?;
            left = FieldExpr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }
    fn parse_field_primary(&mut self) -> Result<FieldExpr, ParseError> {
        if self.eat(&Token::KwNot) {
            let inner = self.parse_field_primary()?;
            return Ok(FieldExpr::Not(Box::new(inner)));
        }
        if self.eat(&Token::LParen) {
            let inner = {
                let head = self.parse_field_primary()?;
                self.continue_field_or(head)?
            };
            self.expect(&Token::RParen, "`)`")?;
            return Ok(inner);
        }
        // expect ident as field name
        let tok = self
            .bump()
            .ok_or_else(|| ParseError::new("expected field name", self.cur_span()))?;
        let field = match &tok.token {
            Token::Ident(s) => s.clone(),
            _ => {
                return Err(ParseError::new(
                    format!("expected field name, found {}", tok.token.display()),
                    tok.span.clone(),
                )
                .with_hint(
                    "parsed-field filters look like `| status >= 500` or `| level == \"error\"`",
                ));
            }
        };
        self.parse_field_cmp_with_field(field)
    }
    fn parse_field_cmp_with_field(&mut self, field: String) -> Result<FieldExpr, ParseError> {
        let op = match self.peek() {
            Some(Token::EqEq) => FieldOp::Eq,
            Some(Token::Ne) => FieldOp::Ne,
            Some(Token::Gt) => FieldOp::Gt,
            Some(Token::Ge) => FieldOp::Ge,
            Some(Token::Lt) => FieldOp::Lt,
            Some(Token::Le) => FieldOp::Le,
            Some(Token::Re) => FieldOp::Re,
            Some(Token::Nre) => FieldOp::Nre,
            _ => {
                return Err(ParseError::new(
                    format!(
                        "expected comparison operator after `{field}`, found {}",
                        self.cur_repr()
                    ),
                    self.cur_span(),
                )
                .with_hint("comparison operators: `==`, `!=`, `>`, `>=`, `<`, `<=`, `=~`, `!~`"));
            }
        };
        self.bump();
        let val_tok = self
            .bump()
            .ok_or_else(|| ParseError::new("expected value", self.cur_span()))?;
        let value = match &val_tok.token {
            Token::String(s) => FieldValue::String(s.clone()),
            Token::Number(n) => FieldValue::Number(*n),
            Token::Integer(n) => FieldValue::Number(*n as f64),
            _ => {
                return Err(ParseError::new(
                    format!(
                        "expected a string or number value, found {}",
                        val_tok.token.display()
                    ),
                    val_tok.span.clone(),
                )
                .with_hint("e.g. `status >= 500` or `level == \"error\"`"));
            }
        };
        Ok(FieldExpr::Cmp { field, op, value })
    }

    // ----- metric query ----------------------------------------------------
    fn parse_metric_query(&mut self) -> Result<MetricQuery, ParseError> {
        // Look at the head ident.
        let Some(Token::Ident(name)) = self.peek().cloned() else {
            return Err(ParseError::new(
                "expected metric function name",
                self.cur_span(),
            ));
        };
        if let Some(rfn) = RangeFn::from_ident(&name) {
            Ok(MetricQuery::Range(self.parse_range_aggregation(rfn)?))
        } else if let Some(afn) = AggFn::from_ident(&name) {
            Ok(MetricQuery::Vector(self.parse_vector_aggregation(afn)?))
        } else {
            Err(ParseError::new(
                format!("unknown metric function `{name}`"),
                self.cur_span(),
            )
            .with_hint("range functions: `count_over_time`, `rate`, `bytes_over_time`, `bytes_rate`, `absent_over_time`; vector functions: `sum`, `avg`, `min`, `max`, `count`, `stddev`, `stdvar`, `topk`, `bottomk`"))
        }
    }

    fn parse_range_aggregation(&mut self, func: RangeFn) -> Result<RangeAggregation, ParseError> {
        // Safe: parse_metric_query peeked a RangeFn ident before calling.
        let head = self
            .bump()
            .expect("BUG: parse_range_aggregation entered without an ident token")
            .span
            .start;
        self.expect(&Token::LParen, "`(`")?;
        let inner = self.parse_log_query()?;
        self.expect(&Token::LBracket, "`[duration]`")?;
        let dur_tok = self.bump().ok_or_else(|| {
            ParseError::new("expected a duration inside `[...]`", self.cur_span())
                .with_hint("durations look like `30s`, `5m`, `1h`, `2d`, `1w`")
        })?;
        let range_ms = match dur_tok.token {
            Token::Duration(ms) => ms,
            _ => {
                return Err(ParseError::new(
                    format!(
                        "expected a duration like `5m`, found {}",
                        dur_tok.token.display()
                    ),
                    dur_tok.span.clone(),
                )
                .with_hint("durations look like `30s`, `5m`, `1h`, `2d`, `1w`"));
            }
        };
        if range_ms == 0 {
            // Audit §1.4: a zero range collapses every range function
            // to a divide-by-zero (`rate`) or an empty bucket
            // (`count_over_time`). Reject up front with a clear hint
            // rather than producing NaNs at exec time.
            return Err(ParseError::new(
                "range duration must be greater than zero",
                dur_tok.span.clone(),
            )
            .with_hint("use a positive duration like `1s`, `5m`, `1h`"));
        }
        self.expect(&Token::RBracket, "`]`")?;
        let close = self.expect(&Token::RParen, "`)`")?;
        Ok(RangeAggregation {
            func,
            inner: Box::new(inner),
            range_ms,
            span: head..close.end,
        })
    }

    fn parse_vector_aggregation(&mut self, func: AggFn) -> Result<VectorAggregation, ParseError> {
        // Safe: parse_metric_query peeked an AggFn ident before calling.
        let head = self
            .bump()
            .expect("BUG: parse_vector_aggregation entered without an ident token")
            .span
            .start;
        // Optional `by (...)` / `without (...)` *before* the `(`.
        let grouping = self.try_parse_grouping()?;
        self.expect(&Token::LParen, "`(`")?;
        // topk/bottomk: integer first, then `,` then expr
        let mut param: Option<i64> = None;
        if func.requires_param() {
            let tok = self.bump().ok_or_else(|| {
                ParseError::new(
                    format!("`{}` requires an integer parameter", func.as_str()),
                    self.cur_span(),
                )
                .with_hint("e.g. `topk(5, sum by (service) (rate(...[5m])))`")
            })?;
            match tok.token {
                Token::Integer(n) => param = Some(n),
                _ => {
                    return Err(ParseError::new(
                        format!(
                            "`{}` requires an integer parameter, found {}",
                            func.as_str(),
                            tok.token.display()
                        ),
                        tok.span.clone(),
                    )
                    .with_hint(
                        "e.g. `topk(5, ...)` — the parameter is the number of series to keep",
                    ));
                }
            }
            self.expect(&Token::Comma, "`,`")?;
        }
        // The inner of an aggregation must itself be a metric query
        // (range agg or another vector agg).
        let inner = self.parse_metric_query()?;
        let close = self.expect(&Token::RParen, "`)`")?;
        Ok(VectorAggregation {
            func,
            grouping,
            param,
            inner: Box::new(inner),
            span: head..close.end,
        })
    }

    fn try_parse_grouping(&mut self) -> Result<Option<Grouping>, ParseError> {
        let mode = match self.peek() {
            Some(Token::KwBy) => GroupingMode::By,
            Some(Token::KwWithout) => GroupingMode::Without,
            _ => return Ok(None),
        };
        self.bump();
        self.expect(&Token::LParen, "`(`")?;
        let mut labels = Vec::new();
        if !matches!(self.peek(), Some(Token::RParen)) {
            loop {
                let tok = self
                    .bump()
                    .ok_or_else(|| ParseError::new("expected label name", self.cur_span()))?;
                match &tok.token {
                    Token::Ident(s) => labels.push(s.clone()),
                    _ => {
                        return Err(ParseError::new(
                            format!("expected label name, found {}", tok.token.display()),
                            tok.span.clone(),
                        )
                        .with_hint("e.g. `by (server, service)` or `without (instance)`"));
                    }
                }
                if !self.eat(&Token::Comma) {
                    break;
                }
            }
        }
        self.expect(&Token::RParen, "`)`")?;
        Ok(Some(Grouping { mode, labels }))
    }
}
