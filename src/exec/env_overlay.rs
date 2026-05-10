//! Per-namespace remote environment overlay.
//!
//! Renders an `env KEY1="VAL1" KEY2="VAL2" ...` prefix that the remote
//! shell parses and applies before the operator's command runs. The
//! overlay is consulted for `inspect run` and `inspect exec` only —
//! read verbs (`logs`, `ps`, `status`, ...) issue inspect-internal
//! commands whose env we control directly, so they are out of scope.
//!
//! ## Quoting model
//!
//! Each value is wrapped in double quotes with `"`, `\`, and backtick
//! escaped. The semantics are deliberate:
//!
//! * `$HOME` / `$PATH` / etc. **expand on the remote** — operators
//!   want the *remote* user's home, not the local one. (Spec: "the
//!   $HOME and $PATH references on the right-hand side are resolved
//!   on the remote host at command-dispatch time, not on the
//!   client.")
//! * `;`, `&&`, `|`, `<`, `>`, newlines stay **literal**. Double
//!   quotes kill word splitting and command-list metacharacters, so
//!   a value of `"v;rm -rf /"` is preserved as a single env-var
//!   string and the `;` does not chain a second command.
//! * Backticks are escaped (kills the legacy `\`...\`` command
//!   substitution form).
//! * `$(...)` style command substitution remains active. This is by
//!   intent: config-file values are operator-owned; users who want
//!   a literal `$` in a value must double it (`$$`) per shell
//!   convention. The 99% use case (`$HOME/.local/bin:$PATH`) needs
//!   `$VAR` expansion; banning `$(...)` would also ban that.
//!
//! ## Audit
//!
//! The full overlay map is recorded structurally on every audit
//! entry that uses it (see [`crate::safety::audit::AuditEntry::env_overlay`]),
//! plus the final rendered remote command line in
//! [`crate::safety::audit::AuditEntry::rendered_cmd`], so post-hoc
//! readers can replay byte-for-byte without parsing the operator's
//! `args` field.

use std::collections::BTreeMap;

use anyhow::{anyhow, Result};

use crate::config::namespace::is_valid_env_key;

/// Wrap `s` for the remote shell with double quotes, escaping `"`,
/// `\`, and backticks. Inside double quotes, `;`, `&`, `|`, `<`, `>`,
/// newlines, and word splits stay literal — but `$VAR` expands.
pub fn dquote_expandable(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' | '\\' | '`' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// Render an `env KEY1="VAL1" KEY2="VAL2" ` prefix (note trailing
/// space) for `overlay`. Empty overlay → empty string. Iteration is
/// in `BTreeMap` order so the rendered string is deterministic across
/// runs (audit-log diff stability, test reproducibility).
///
/// SMOKE 2026-05-09 fix: GNU `env` (coreutils 8.30+) does NOT treat
/// `--` as an option terminator — `env PATH=foo -- cmd` exits with
/// `env: '--': No such file or directory` on every Linux distro
/// shipping GNU env (Ubuntu 22.04+, Debian 12+, Fedora 38+,
/// arteOS at smoke time). The `[-]` form in env's synopsis is
/// the `--ignore-environment` short option, not a `--`-style
/// terminator. The pre-fix recipe rendered `env KEY=VAL -- CMD`
/// which surfaced live during release smoke. Drop
/// the `--` and rely on the natural `env KEY=VAL CMD` shape that
/// every env (GNU, BSD, busybox) supports. Safe because
/// `is_valid_env_key` already rejects keys starting with `-` (POSIX
/// shell variable name regex), and shell commands themselves never
/// begin with `-` in our use sites (`docker …` / structured-write
/// renderers / operator-supplied `inspect run -- "<cmd>"` payloads
/// all start with a program name, not a flag).
pub fn render_env_prefix(overlay: &BTreeMap<String, String>) -> String {
    if overlay.is_empty() {
        return String::new();
    }
    let mut out = String::from("env");
    for (k, v) in overlay {
        out.push(' ');
        out.push_str(k);
        out.push('=');
        out.push_str(&dquote_expandable(v));
    }
    out.push(' ');
    out
}

/// Apply an overlay to `cmd`. When `overlay` is empty, returns `cmd`
/// borrowed unchanged so the no-overlay path is allocation-free.
pub fn apply_to_cmd<'a>(
    cmd: &'a str,
    overlay: &BTreeMap<String, String>,
) -> std::borrow::Cow<'a, str> {
    if overlay.is_empty() {
        std::borrow::Cow::Borrowed(cmd)
    } else {
        std::borrow::Cow::Owned(format!("{}{}", render_env_prefix(overlay), cmd))
    }
}

/// Parse a `KEY=VALUE` argv token (used for `--env KEY=VALUE` and
/// `--set-env KEY=VALUE`). Returns the validated key and the raw
/// value (no shell interpretation here — that happens on the remote).
pub fn parse_kv(raw: &str) -> Result<(String, String)> {
    let (k, v) = raw
        .split_once('=')
        .ok_or_else(|| anyhow!("--env / --set-env value '{raw}' must be KEY=VALUE"))?;
    if !is_valid_env_key(k) {
        return Err(anyhow!(
            "--env / --set-env key '{k}' must match [A-Za-z_][A-Za-z0-9_]* \
             (POSIX shell variable name)"
        ));
    }
    Ok((k.to_string(), v.to_string()))
}

/// Compute the effective overlay for a single
/// `inspect run` / `inspect exec` invocation.
///
/// * `base` — the namespace overlay from `[namespaces.<ns>.env]`,
///   or `None` if no overlay is configured.
/// * `user` — per-invocation `--env KEY=VALUE` flags (already
///   parsed). Operator wins on collision with `base`.
/// * `clear` — `--env-clear` was passed: drop `base` for this
///   invocation; `user` is the entire overlay.
///
/// The result is empty-when-empty so call sites can use
/// [`apply_to_cmd`] without branching.
pub fn merge(
    base: Option<&BTreeMap<String, String>>,
    user: &[(String, String)],
    clear: bool,
) -> BTreeMap<String, String> {
    let mut out = if clear {
        BTreeMap::new()
    } else {
        base.cloned().unwrap_or_default()
    };
    for (k, v) in user {
        out.insert(k.clone(), v.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn dquote_preserves_var_references() {
        assert_eq!(
            dquote_expandable("$HOME/.local/bin:$PATH"),
            "\"$HOME/.local/bin:$PATH\""
        );
    }

    #[test]
    fn dquote_escapes_quote_backslash_backtick() {
        assert_eq!(dquote_expandable("a\"b"), "\"a\\\"b\"");
        assert_eq!(dquote_expandable("a\\b"), "\"a\\\\b\"");
        assert_eq!(dquote_expandable("a`b`c"), "\"a\\`b\\`c\"");
    }

    #[test]
    fn dquote_keeps_metachars_literal() {
        // `;` inside a value never splits
        // the remote command list. Test mirrors the spec's
        // `MALICIOUS = "v;rm -rf /"` reproducer.
        assert_eq!(dquote_expandable("v;rm -rf /"), "\"v;rm -rf /\"");
        assert_eq!(dquote_expandable("a&&b"), "\"a&&b\"");
        assert_eq!(dquote_expandable("a|b"), "\"a|b\"");
    }

    #[test]
    fn render_prefix_empty_overlay_is_empty() {
        let m = BTreeMap::new();
        assert_eq!(render_env_prefix(&m), "");
    }

    #[test]
    fn render_prefix_is_alphabetical_and_quoted() {
        // SMOKE 2026-05-09 fix: trailing separator is a single space,
        // NOT `-- ` (GNU env doesn't treat `--` as an option
        // terminator; pre-fix `env KEY=VAL -- CMD` died with
        // `env: '--': No such file or directory`).
        let m = map(&[("PATH", "$HOME/.local/bin:$PATH"), ("LANG", "C.UTF-8")]);
        assert_eq!(
            render_env_prefix(&m),
            "env LANG=\"C.UTF-8\" PATH=\"$HOME/.local/bin:$PATH\" "
        );
    }

    #[test]
    fn apply_to_cmd_no_overlay_is_borrow() {
        let m = BTreeMap::new();
        let out = apply_to_cmd("echo hi", &m);
        assert!(matches!(out, std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn apply_to_cmd_with_overlay_prepends_env() {
        // SMOKE 2026-05-09 fix: render `env KEY=VAL CMD` (single
        // space separator), not `env KEY=VAL -- CMD` — GNU env
        // does not treat `--` as an option terminator.
        let m = map(&[("FOO", "bar")]);
        let out = apply_to_cmd("echo hi", &m);
        assert_eq!(out.as_ref(), "env FOO=\"bar\" echo hi");
    }

    #[test]
    fn parse_kv_accepts_simple() {
        assert_eq!(
            parse_kv("FOO=bar").unwrap(),
            ("FOO".to_string(), "bar".to_string())
        );
    }

    #[test]
    fn parse_kv_accepts_empty_value() {
        assert_eq!(
            parse_kv("FOO=").unwrap(),
            ("FOO".to_string(), String::new())
        );
    }

    #[test]
    fn parse_kv_accepts_value_with_equals() {
        assert_eq!(
            parse_kv("FOO=a=b").unwrap(),
            ("FOO".to_string(), "a=b".to_string())
        );
    }

    #[test]
    fn parse_kv_rejects_no_equals() {
        assert!(parse_kv("FOO").is_err());
    }

    #[test]
    fn parse_kv_rejects_invalid_key() {
        assert!(parse_kv("foo-bar=x").is_err());
        assert!(parse_kv("1FOO=x").is_err());
        assert!(parse_kv("=x").is_err());
    }

    #[test]
    fn merge_clear_drops_base() {
        let base = map(&[("PATH", "/a")]);
        let user = vec![("LANG".to_string(), "C".to_string())];
        let out = merge(Some(&base), &user, true);
        assert_eq!(out.len(), 1);
        assert_eq!(out.get("LANG").map(String::as_str), Some("C"));
        assert!(!out.contains_key("PATH"));
    }

    #[test]
    fn merge_user_wins_collision() {
        let base = map(&[("PATH", "/a"), ("LANG", "C")]);
        let user = vec![("PATH".to_string(), "/b".to_string())];
        let out = merge(Some(&base), &user, false);
        assert_eq!(out.get("PATH").map(String::as_str), Some("/b"));
        assert_eq!(out.get("LANG").map(String::as_str), Some("C"));
    }

    #[test]
    fn merge_no_base_no_user_is_empty() {
        let out = merge(None, &[], false);
        assert!(out.is_empty());
    }
}
