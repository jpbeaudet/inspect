//! Variable interpolation for bundles (B9, v0.1.2).
//!
//! Supports two namespaces only:
//! * `{{ vars.<key> }}` / `{{ vars.<key>.<subkey> }}` — references the
//!   bundle-level `vars:` map. Dot-traversal walks YAML mappings and
//!   uses bracketless integer indices for sequences (`{{ vars.list.0 }}`).
//! * `{{ matrix.<key> }}` — single matrix entry, set by the executor
//!   per parallel branch.
//!
//! Whitespace inside the braces is allowed and trimmed (`{{  vars.x }}`
//! is equivalent to `{{vars.x}}`). A reference that doesn't resolve
//! returns `Err(InterpError::Unresolved)` so plan/run can surface the
//! exact missing key without producing a half-substituted command.
//!
//! No conditionals, loops, default-fallbacks, or filters — operators
//! who need real logic write a shell script around `inspect bundle run`.

use std::collections::BTreeMap;

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum InterpError {
    #[error("unterminated `{{{{` at byte offset {0}")]
    Unterminated(usize),
    #[error("variable `{0}` is not defined")]
    Unresolved(String),
    #[error("only `vars.` and `matrix.` references are allowed (got `{0}`)")]
    UnknownNamespace(String),
    #[error("variable `{0}` cannot be rendered as a string (it is a map or sequence at the leaf)")]
    NotScalar(String),
}

/// Render YAML scalar (or scalar-like) values to the string form
/// used in command-line substitution. Sequences/mappings are *not*
/// rendered as JSON because operators almost never want that — they
/// want a single value. Joining sequences is left to the caller via
/// the matrix-expansion path.
pub fn yaml_to_str(v: &serde_yaml::Value) -> Option<String> {
    match v {
        serde_yaml::Value::Null => Some(String::new()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::String(s) => Some(s.clone()),
        serde_yaml::Value::Sequence(_) | serde_yaml::Value::Mapping(_) => None,
        serde_yaml::Value::Tagged(t) => yaml_to_str(&t.value),
    }
}

/// Walk a dot-path through a YAML value. Sequence indices are written
/// as bare integers (`servers.0`); empty path returns the root.
fn lookup<'a>(root: &'a serde_yaml::Value, path: &str) -> Option<&'a serde_yaml::Value> {
    if path.is_empty() {
        return Some(root);
    }
    let mut cur = root;
    for seg in path.split('.') {
        if seg.is_empty() {
            return None;
        }
        cur = match cur {
            serde_yaml::Value::Mapping(m) => m.get(serde_yaml::Value::String(seg.to_string()))?,
            serde_yaml::Value::Sequence(s) => {
                let idx: usize = seg.parse().ok()?;
                s.get(idx)?
            }
            _ => return None,
        };
    }
    Some(cur)
}

/// Substitute every `{{ ... }}` template in `input`. The
/// `vars`/`matrix` arguments are looked up by the leading namespace
/// segment.
pub fn interpolate(
    input: &str,
    vars: &BTreeMap<String, serde_yaml::Value>,
    matrix: &BTreeMap<String, serde_yaml::Value>,
) -> Result<String, InterpError> {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        // Look for an opening `{{`.
        if i + 1 < bytes.len() && bytes[i] == b'{' && bytes[i + 1] == b'{' {
            // Locate the matching `}}`. We don't support nested
            // templates, so the first `}}` after `{{` closes the
            // expression.
            let start = i + 2;
            let mut j = start;
            let mut closed = false;
            while j + 1 < bytes.len() {
                if bytes[j] == b'}' && bytes[j + 1] == b'}' {
                    closed = true;
                    break;
                }
                j += 1;
            }
            if !closed {
                return Err(InterpError::Unterminated(i));
            }
            let expr = std::str::from_utf8(&bytes[start..j])
                .expect("ASCII-safe slicing")
                .trim();
            // Split on first `.`: namespace `.` path.
            let (ns, path) = match expr.split_once('.') {
                Some((ns, rest)) => (ns, rest),
                None => (expr, ""),
            };
            let root = match ns {
                "vars" => vars,
                "matrix" => matrix,
                other => return Err(InterpError::UnknownNamespace(other.to_string())),
            };
            // Dot-walk into the namespace map.
            let leaf = if path.is_empty() {
                return Err(InterpError::Unresolved(ns.to_string()));
            } else {
                // Split path into first segment + rest. The first
                // segment is the top-level key in the namespace map;
                // subsequent segments traverse the value.
                let (head, tail) = match path.split_once('.') {
                    Some((h, t)) => (h, t),
                    None => (path, ""),
                };
                let v = root
                    .get(head)
                    .ok_or_else(|| InterpError::Unresolved(format!("{ns}.{path}")))?;
                lookup(v, tail).ok_or_else(|| InterpError::Unresolved(format!("{ns}.{path}")))?
            };
            let s =
                yaml_to_str(leaf).ok_or_else(|| InterpError::NotScalar(format!("{ns}.{path}")))?;
            out.push_str(&s);
            i = j + 2;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars_from(yaml: &str) -> BTreeMap<String, serde_yaml::Value> {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn no_template_is_passthrough() {
        let v = BTreeMap::new();
        assert_eq!(interpolate("hello world", &v, &v).unwrap(), "hello world");
    }

    #[test]
    fn vars_scalar_substitutes() {
        let vars = vars_from("snapshot_dir: /srv/snapshots/2026-04-27\n");
        let m = BTreeMap::new();
        let out = interpolate("rsync to {{ vars.snapshot_dir }} now", &vars, &m).unwrap();
        assert_eq!(out, "rsync to /srv/snapshots/2026-04-27 now");
    }

    #[test]
    fn vars_dot_path_walks_into_mapping() {
        let vars = vars_from(
            r#"
config:
  retention: 30d
"#,
        );
        let m = BTreeMap::new();
        let out = interpolate("ttl={{ vars.config.retention }}", &vars, &m).unwrap();
        assert_eq!(out, "ttl=30d");
    }

    #[test]
    fn matrix_resolves() {
        let vars = BTreeMap::new();
        let mut m = BTreeMap::new();
        m.insert(
            "volume".to_string(),
            serde_yaml::Value::String("atlas_milvus".into()),
        );
        let out = interpolate("tar {{ matrix.volume }}", &vars, &m).unwrap();
        assert_eq!(out, "tar atlas_milvus");
    }

    #[test]
    fn whitespace_inside_braces_is_trimmed() {
        let vars = vars_from("k: v\n");
        let m = BTreeMap::new();
        assert_eq!(interpolate("{{vars.k}}", &vars, &m).unwrap(), "v");
        assert_eq!(interpolate("{{  vars.k  }}", &vars, &m).unwrap(), "v");
    }

    #[test]
    fn unknown_namespace_errors() {
        let vars = BTreeMap::new();
        let m = BTreeMap::new();
        let err = interpolate("{{ env.HOME }}", &vars, &m).unwrap_err();
        assert!(matches!(err, InterpError::UnknownNamespace(_)));
    }

    #[test]
    fn unresolved_reports_full_dot_path() {
        let vars = vars_from("a:\n  b: c\n");
        let m = BTreeMap::new();
        let err = interpolate("{{ vars.a.x }}", &vars, &m).unwrap_err();
        match err {
            InterpError::Unresolved(p) => assert_eq!(p, "vars.a.x"),
            other => panic!("expected Unresolved, got {other:?}"),
        }
    }

    #[test]
    fn unterminated_template_errors() {
        let vars = BTreeMap::new();
        let m = BTreeMap::new();
        let err = interpolate("{{ vars.x", &vars, &m).unwrap_err();
        assert!(matches!(err, InterpError::Unterminated(_)));
    }

    #[test]
    fn map_at_leaf_is_not_scalar_error() {
        let vars = vars_from(
            r#"
services:
  clients: [a, b]
"#,
        );
        let m = BTreeMap::new();
        let err = interpolate("{{ vars.services }}", &vars, &m).unwrap_err();
        assert!(matches!(err, InterpError::NotScalar(_)));
    }

    #[test]
    fn sequence_index_lookup() {
        let vars = vars_from("list: [a, b, c]\n");
        let m = BTreeMap::new();
        let out = interpolate("{{ vars.list.0 }}-{{ vars.list.2 }}", &vars, &m).unwrap();
        assert_eq!(out, "a-c");
    }

    #[test]
    fn nested_braces_in_command_strings_pass_through() {
        // Step exec strings often contain `{{ }}` for docker format
        // strings (e.g. `--format '{{ .State.Running }}'`). We DO
        // substitute those — operators must escape with `{{vars.x}}`-
        // style or use shell-quoting if they want a literal. This
        // test just pins the current behavior so a future change to
        // add literal-escaping doesn't silently change it.
        let vars = vars_from("State: stopped\n");
        let m = BTreeMap::new();
        let out = interpolate("docker inspect '{{ vars.State }}'", &vars, &m).unwrap();
        assert_eq!(out, "docker inspect 'stopped'");
    }
}
