//! Mini template engine for `| line_format "<tpl>"` and
//! `| label_format <name>="<tpl>"`. Supports `{{.name}}` substitution
//! against the record's labels + parsed fields. Names not found expand
//! to the empty string. Other `{{ }}` syntax is currently passed
//! through verbatim — we intentionally do not implement Go's full
//! text/template until a concrete user need lands.

use crate::exec::record::Record;

pub fn render(template: &str, rec: &Record) -> String {
    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'{' && bytes[i + 1] == b'{' {
            // find closing `}}`
            let close_rel = template[i + 2..].find("}}");
            if let Some(end) = close_rel {
                let inner = &template[i + 2..i + 2 + end];
                let key = inner.trim().trim_start_matches('.');
                if let Some(v) = rec.lookup(key) {
                    out.push_str(&v);
                }
                i += 2 + end + 2;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_labels_and_fields() {
        let mut r = Record::new()
            .with_label("service", "api")
            .with_line("hi");
        r.fields
            .insert("status".into(), serde_json::json!(500));
        let s = render("{{.service}}: status={{.status}}", &r);
        assert_eq!(s, "api: status=500");
    }
    #[test]
    fn unknown_keys_expand_to_empty() {
        let r = Record::new();
        assert_eq!(render("[{{.missing}}]", &r), "[]");
    }
    #[test]
    fn passes_through_braces_when_no_close() {
        let r = Record::new();
        assert_eq!(render("hello {{.x", &r), "hello {{.x");
    }
}
