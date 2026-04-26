//! AST types for parsed LogQL queries (bible §9.13).

use std::ops::Range;

/// Byte span into the original (post-alias-substitution) input.
pub type Span = Range<usize>;

/// Top-level: a query is either a streaming log query or a metric
/// (aggregation) query, never both. Bible §9.7.
#[derive(Debug, Clone, PartialEq)]
pub enum Query {
    Log(LogQuery),
    Metric(MetricQuery),
}

/// `selector_union (filter | stage)*`
#[derive(Debug, Clone, PartialEq)]
pub struct LogQuery {
    pub selector: SelectorUnion,
    pub pipeline: Vec<PipelineOp>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PipelineOp {
    Filter(Filter),
    Stage(Stage),
}

/// One or more `selector` joined by `or`.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectorUnion {
    pub branches: Vec<Selector>,
    pub span: Span,
}

/// `{label="x"}` or `@alias`. Aliases get expanded before parsing,
/// so by the time we hit the AST a `Selector` is always a label set.
#[derive(Debug, Clone, PartialEq)]
pub struct Selector {
    pub matchers: Vec<LabelMatcher>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LabelMatcher {
    pub name: String,
    pub op: MatchOp,
    pub value: String,
    pub name_span: Span,
    pub value_span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchOp {
    /// `=`
    Eq,
    /// `!=`
    Ne,
    /// `=~`
    Re,
    /// `!~`
    Nre,
}

/// Line filter: `|=`, `!=`, `|~`, `!~`.
#[derive(Debug, Clone, PartialEq)]
pub struct Filter {
    pub op: FilterOp,
    pub pattern: String,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOp {
    /// `|=`
    Contains,
    /// `!=` (line-level "doesn't contain")
    NotContains,
    /// `|~`
    Re,
    /// `!~`
    Nre,
}
impl FilterOp {
    pub fn as_str(self) -> &'static str {
        match self {
            FilterOp::Contains => "|=",
            FilterOp::NotContains => "!=",
            FilterOp::Re => "|~",
            FilterOp::Nre => "!~",
        }
    }
}

/// Pipeline stages after `|`. Bible §9.6.
#[derive(Debug, Clone, PartialEq)]
pub enum Stage {
    Json,
    Logfmt,
    Pattern {
        template: String,
        span: Span,
    },
    Regexp {
        pattern: String,
        span: Span,
    },
    LineFormat {
        template: String,
        span: Span,
    },
    LabelFormat {
        assignments: Vec<LabelAssign>,
        span: Span,
    },
    Drop {
        labels: Vec<String>,
        span: Span,
    },
    Keep {
        labels: Vec<String>,
        span: Span,
    },
    /// `| <field> <op> <value>` parsed-field filter, may be a boolean tree.
    FieldFilter {
        expr: FieldExpr,
        span: Span,
    },
    /// `| map { <log_query> }`
    Map {
        sub: Box<LogQuery>,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct LabelAssign {
    pub name: String,
    pub template: String,
}

/// Parsed-field filter expression — boolean tree of comparisons.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldExpr {
    Cmp {
        field: String,
        op: FieldOp,
        value: FieldValue,
    },
    And(Box<FieldExpr>, Box<FieldExpr>),
    Or(Box<FieldExpr>, Box<FieldExpr>),
    Not(Box<FieldExpr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldOp {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    Re,
    Nre,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    String(String),
    Number(f64),
}

// ---------------------------------------------------------------- metric

#[derive(Debug, Clone, PartialEq)]
pub enum MetricQuery {
    Range(RangeAggregation),
    Vector(VectorAggregation),
}

/// `range_fn ( log_query [ duration ] )`
#[derive(Debug, Clone, PartialEq)]
pub struct RangeAggregation {
    pub func: RangeFn,
    pub inner: Box<LogQuery>,
    pub range_ms: u64,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeFn {
    CountOverTime,
    Rate,
    BytesOverTime,
    BytesRate,
    AbsentOverTime,
}
impl RangeFn {
    pub fn from_ident(s: &str) -> Option<Self> {
        Some(match s {
            "count_over_time" => RangeFn::CountOverTime,
            "rate" => RangeFn::Rate,
            "bytes_over_time" => RangeFn::BytesOverTime,
            "bytes_rate" => RangeFn::BytesRate,
            "absent_over_time" => RangeFn::AbsentOverTime,
            _ => return None,
        })
    }
}

/// `agg_fn (by/without (...))? ( inner )`
/// `inner` can be another vector aggregation (nesting), or a range aggregation.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorAggregation {
    pub func: AggFn,
    pub grouping: Option<Grouping>,
    /// `topk(K, ...)` / `bottomk(K, ...)` carry an integer parameter.
    pub param: Option<i64>,
    pub inner: Box<MetricQuery>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Grouping {
    pub mode: GroupingMode,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupingMode {
    By,
    Without,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggFn {
    Sum,
    Avg,
    Min,
    Max,
    Stddev,
    Stdvar,
    Count,
    Topk,
    Bottomk,
}
impl AggFn {
    pub fn from_ident(s: &str) -> Option<Self> {
        Some(match s {
            "sum" => AggFn::Sum,
            "avg" => AggFn::Avg,
            "min" => AggFn::Min,
            "max" => AggFn::Max,
            "stddev" => AggFn::Stddev,
            "stdvar" => AggFn::Stdvar,
            "count" => AggFn::Count,
            "topk" => AggFn::Topk,
            "bottomk" => AggFn::Bottomk,
            _ => return None,
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            AggFn::Sum => "sum",
            AggFn::Avg => "avg",
            AggFn::Min => "min",
            AggFn::Max => "max",
            AggFn::Stddev => "stddev",
            AggFn::Stdvar => "stdvar",
            AggFn::Count => "count",
            AggFn::Topk => "topk",
            AggFn::Bottomk => "bottomk",
        }
    }
    pub fn requires_param(self) -> bool {
        matches!(self, AggFn::Topk | AggFn::Bottomk)
    }
}
