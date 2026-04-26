//! Parsed-field filter evaluator (`| level == "error" and status >= 500`).

use regex::Regex;

use crate::exec::record::{value_as_f64, Record};
use crate::logql::ast::{FieldExpr, FieldOp, FieldValue};

pub fn eval(expr: &FieldExpr, rec: &Record) -> bool {
    match expr {
        FieldExpr::And(a, b) => eval(a, rec) && eval(b, rec),
        FieldExpr::Or(a, b) => eval(a, rec) || eval(b, rec),
        FieldExpr::Not(e) => !eval(e, rec),
        FieldExpr::Cmp { field, op, value } => eval_cmp(field, *op, value, rec),
    }
}

fn eval_cmp(field: &str, op: FieldOp, value: &FieldValue, rec: &Record) -> bool {
    let actual = rec.lookup(field);
    // Regex ops are always string comparisons.
    if matches!(op, FieldOp::Re | FieldOp::Nre) {
        let pat = match value {
            FieldValue::String(s) => s.as_str(),
            FieldValue::Number(n) => return false_if_not_string(n, op, &actual),
        };
        let re = match Regex::new(pat) {
            Ok(re) => re,
            Err(_) => return false,
        };
        let hay = actual.unwrap_or_default();
        let m = re.is_match(&hay);
        return if matches!(op, FieldOp::Re) { m } else { !m };
    }
    // For ==, !=, >, >=, <, <=: prefer numeric compare when both sides parse.
    let actual_str = actual.clone();
    let actual_num = actual.as_deref().and_then(|s| s.parse::<f64>().ok());
    match value {
        FieldValue::Number(want) => {
            let Some(got) = actual_num else {
                return matches!(op, FieldOp::Ne);
            };
            cmp_num(got, *want, op)
        }
        FieldValue::String(want) => {
            let got = actual_str.unwrap_or_default();
            cmp_str(&got, want, op)
        }
    }
}

fn cmp_num(got: f64, want: f64, op: FieldOp) -> bool {
    match op {
        FieldOp::Eq => (got - want).abs() < f64::EPSILON,
        FieldOp::Ne => (got - want).abs() >= f64::EPSILON,
        FieldOp::Gt => got > want,
        FieldOp::Ge => got >= want,
        FieldOp::Lt => got < want,
        FieldOp::Le => got <= want,
        FieldOp::Re | FieldOp::Nre => false,
    }
}

fn cmp_str(got: &str, want: &str, op: FieldOp) -> bool {
    match op {
        FieldOp::Eq => got == want,
        FieldOp::Ne => got != want,
        FieldOp::Gt => got > want,
        FieldOp::Ge => got >= want,
        FieldOp::Lt => got < want,
        FieldOp::Le => got <= want,
        FieldOp::Re | FieldOp::Nre => false,
    }
}

fn false_if_not_string(_n: &f64, op: FieldOp, _actual: &Option<String>) -> bool {
    // Regex against a numeric literal — undefined; treat as no-match.
    matches!(op, FieldOp::Nre)
}

// keep clippy happy for unused parser import in a small file
#[allow(dead_code)]
fn _silence_unused(v: &serde_json::Value) -> Option<f64> {
    value_as_f64(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec_with(field: &str, v: serde_json::Value) -> Record {
        let mut r = Record::new();
        r.fields.insert(field.into(), v);
        r
    }

    #[test]
    fn numeric_ge() {
        let r = rec_with("status", json!(500));
        let e = FieldExpr::Cmp {
            field: "status".into(),
            op: FieldOp::Ge,
            value: FieldValue::Number(500.0),
        };
        assert!(eval(&e, &r));
        let e2 = FieldExpr::Cmp {
            field: "status".into(),
            op: FieldOp::Gt,
            value: FieldValue::Number(500.0),
        };
        assert!(!eval(&e2, &r));
    }
    #[test]
    fn string_eq() {
        let r = rec_with("level", json!("error"));
        let e = FieldExpr::Cmp {
            field: "level".into(),
            op: FieldOp::Eq,
            value: FieldValue::String("error".into()),
        };
        assert!(eval(&e, &r));
    }
    #[test]
    fn regex_match() {
        let r = rec_with("path", json!("/api/v1/users"));
        let e = FieldExpr::Cmp {
            field: "path".into(),
            op: FieldOp::Re,
            value: FieldValue::String("^/api".into()),
        };
        assert!(eval(&e, &r));
    }
    #[test]
    fn boolean_and_or_not() {
        let mut r = Record::new();
        r.fields.insert("a".into(), json!(1));
        r.fields.insert("b".into(), json!(2));
        let a_eq_1 = FieldExpr::Cmp {
            field: "a".into(),
            op: FieldOp::Eq,
            value: FieldValue::Number(1.0),
        };
        let b_eq_3 = FieldExpr::Cmp {
            field: "b".into(),
            op: FieldOp::Eq,
            value: FieldValue::Number(3.0),
        };
        assert!(eval(
            &FieldExpr::Or(Box::new(a_eq_1.clone()), Box::new(b_eq_3.clone())),
            &r
        ));
        assert!(!eval(
            &FieldExpr::And(Box::new(a_eq_1.clone()), Box::new(b_eq_3.clone())),
            &r
        ));
        assert!(eval(&FieldExpr::Not(Box::new(b_eq_3)), &r));
    }
}
