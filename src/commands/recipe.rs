//! `inspect recipe <name>` — multi-step diagnostic / remediation flow.
//!
//! Recipes are YAML documents shipped built-in or stored under
//! `~/.inspect/recipes/<name>.yaml`. The runner spawns the current
//! `inspect` binary once per step (`std::env::current_exe`), inheriting
//! environment so test mocks (`INSPECT_MOCK_REMOTE_FILE`,
//! `INSPECT_HOME`) propagate naturally and so each step's own safety
//! contract still applies.
//!
//! Mutating recipes (`mutating: true`) require an explicit `--apply`
//! at the recipe level; without it, every mutating sub-step is left in
//! its native dry-run mode. With `--apply`, the flag is forwarded only
//! to steps whose first token is a known mutating verb. Non-mutating
//! verbs never receive `--apply`.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use crate::cli::RecipeArgs;
use crate::error::ExitKind;
use crate::paths;
use crate::verbs::output::OutputDoc;
use serde_json::json;

/// Verbs that mutate remote state. Keep in sync with the write-verb
/// matrix in `src/verbs/write/`.
const MUTATING_VERBS: &[&str] = &[
    "restart", "stop", "start", "reload", "cp", "edit", "rm", "mkdir", "touch", "chmod", "chown",
    "exec",
];

#[derive(Debug, Clone, Deserialize)]
struct RecipeDoc {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub mutating: bool,
    pub steps: Vec<String>,
}

pub fn run(args: RecipeArgs) -> Result<ExitKind> {
    let doc = load_recipe(&args.name)?;

    if doc.mutating && !args.apply {
        // Surface a clear dry-run banner before steps run, mirroring
        // the per-verb safety contract.
        if !args.format.is_json() {
            eprintln!("note: recipe '{}' is marked mutating; running in dry-run mode (pass --apply to enact)", doc.name);
        }
    }

    let me = std::env::current_exe()
        .context("locating current `inspect` binary for recipe step execution")?;

    let mut step_results: Vec<StepResult> = Vec::with_capacity(doc.steps.len());
    let mut data_lines: Vec<String> = Vec::new();
    let mut any_failed = false;

    for (idx, raw) in doc.steps.iter().enumerate() {
        let mut argv = match shell_split(raw) {
            Ok(v) if !v.is_empty() => v,
            Ok(_) => return Err(anyhow!("recipe '{}' step #{} is empty", doc.name, idx + 1)),
            Err(e) => return Err(anyhow!("recipe '{}' step #{}: {}", doc.name, idx + 1, e)),
        };
        substitute_placeholders(&mut argv, &args);
        if doc.mutating
            && args.apply
            && argv
                .first()
                .map(|v| MUTATING_VERBS.contains(&v.as_str()))
                .unwrap_or(false)
            && !argv.iter().any(|a| a == "--apply")
        {
            argv.push("--apply".to_string());
        }

        let mut cmd = Command::new(&me);
        cmd.args(&argv);
        if args.format.is_json() {
            cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        } else {
            // Prefix per-step header so users can correlate output to steps.
            println!(
                "=== step {}/{}: inspect {} ===",
                idx + 1,
                doc.steps.len(),
                argv.join(" ")
            );
            cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());
        }
        let output = cmd
            .output()
            .with_context(|| format!("spawning step #{}: inspect {}", idx + 1, argv.join(" ")))?;
        let exit_code = output.status.code().unwrap_or(-1);
        let ok = output.status.success();
        if !ok {
            any_failed = true;
        }
        step_results.push(StepResult {
            argv: argv.clone(),
            exit_code,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
        if !args.format.is_json() {
            data_lines.push(format!(
                "step {}: inspect {} -> exit {}",
                idx + 1,
                argv.join(" "),
                exit_code
            ));
        }
    }

    let steps_json: Vec<serde_json::Value> = step_results
        .iter()
        .map(|s| {
            json!({
                "argv": s.argv,
                "exit_code": s.exit_code,
                "stdout": s.stdout,
                "stderr": s.stderr,
            })
        })
        .collect();
    let summary = format!(
        "recipe '{}' completed {} step(s){}",
        doc.name,
        doc.steps.len(),
        if any_failed { " (with failures)" } else { "" }
    );
    let mut doc_out = OutputDoc::new(
        summary,
        json!({
            "recipe": doc.name.clone(),
            "description": doc.description.clone().unwrap_or_default(),
            "mutating": doc.mutating,
            "apply": args.apply,
            "steps": steps_json,
            "totals": {
                "steps": doc.steps.len(),
                "failed": step_results.iter().filter(|s| s.exit_code != 0).count(),
            }
        }),
    )
    .with_meta("phase", 10);
    if doc.mutating && !args.apply {
        doc_out.push_next(crate::verbs::output::NextStep::new(
            format!("inspect recipe {} --apply", args.name),
            "apply mutating steps",
        ));
    }
    let fmt = args.format.resolve()?;
    let exit =
        crate::format::render::render_doc(&doc_out, &fmt, &data_lines, args.format.select_spec())?;

    // The recipe-failure exit class (Error if any step exited non-zero)
    // takes precedence over filter-class exit codes.
    Ok(if any_failed { ExitKind::Error } else { exit })
}

struct StepResult {
    argv: Vec<String>,
    exit_code: i32,
    stdout: String,
    stderr: String,
}

fn load_recipe(name_or_path: &str) -> Result<RecipeDoc> {
    // Path lookup if user passed something path-like.
    let looks_like_path = name_or_path.contains('/')
        || name_or_path.ends_with(".yaml")
        || name_or_path.ends_with(".yml");
    if looks_like_path {
        let p = PathBuf::from(name_or_path);
        let body = std::fs::read_to_string(&p)
            .with_context(|| format!("reading recipe file '{}'", p.display()))?;
        return parse_recipe(&body, &p.display().to_string());
    }
    // Built-in resolution: user override first, then built-in pack.
    let user = paths::inspect_home()
        .join("recipes")
        .join(format!("{name_or_path}.yaml"));
    if user.exists() {
        let body = std::fs::read_to_string(&user)
            .with_context(|| format!("reading recipe file '{}'", user.display()))?;
        return parse_recipe(&body, &user.display().to_string());
    }
    if let Some(body) = builtin::find(name_or_path) {
        return parse_recipe(body, &format!("<builtin:{name_or_path}>"));
    }
    Err(anyhow!(
        "no recipe named '{name_or_path}' (looked in built-ins and {}). Built-ins: {}",
        user.display(),
        builtin::names().join(", ")
    ))
}

fn parse_recipe(body: &str, source: &str) -> Result<RecipeDoc> {
    let doc: RecipeDoc =
        serde_yaml::from_str(body).with_context(|| format!("parsing recipe at {source}"))?;
    if doc.steps.is_empty() {
        return Err(anyhow!("recipe at {source} has no `steps`"));
    }
    Ok(doc)
}

/// Replace `$SEL` tokens in argv with the recipe-level `--sel` value.
/// We use plain placeholder substitution, not shell expansion, so the
/// substitution is whitespace-safe.
fn substitute_placeholders(argv: &mut [String], args: &RecipeArgs) {
    if let Some(sel) = args.sel.as_deref() {
        for a in argv.iter_mut() {
            if a.contains("$SEL") {
                *a = a.replace("$SEL", sel);
            }
        }
    }
}

/// Tiny shell-style splitter. Honors single and double quotes, and
/// backslash escapes inside double quotes. Unknown escapes are left
/// literal (matches POSIX-ish bash). Errors on unterminated quotes.
fn shell_split(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escape = false;
    let mut have_token = false;
    for c in s.chars() {
        if escape {
            cur.push(c);
            escape = false;
            have_token = true;
            continue;
        }
        if in_single {
            if c == '\'' {
                in_single = false;
            } else {
                cur.push(c);
            }
            continue;
        }
        if in_double {
            if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_double = false;
            } else {
                cur.push(c);
            }
            continue;
        }
        match c {
            '\'' => {
                in_single = true;
                have_token = true;
            }
            '"' => {
                in_double = true;
                have_token = true;
            }
            '\\' => {
                escape = true;
                have_token = true;
            }
            c if c.is_whitespace() => {
                if have_token {
                    out.push(std::mem::take(&mut cur));
                    have_token = false;
                }
            }
            _ => {
                cur.push(c);
                have_token = true;
            }
        }
    }
    if in_single || in_double {
        return Err("unterminated quoted string".to_string());
    }
    if escape {
        return Err("trailing backslash".to_string());
    }
    if have_token {
        out.push(cur);
    }
    Ok(out)
}

mod builtin {
    /// Built-in recipe pack (bible §12.1). Each is a small, deterministic
    /// flow keyed on a `$SEL` placeholder for the user's selector.
    const DEPLOY_CHECK: &str = r#"
name: deploy-check
description: "Status, health, error scan, and connectivity for a target service."
steps:
  - "status $SEL"
  - "health $SEL"
  - "search '{server=\"$SEL\", source=\"logs\"} |= \"error\"' --since 5m"
  - "connectivity $SEL"
"#;

    const DISK_AUDIT: &str = r#"
name: disk-audit
description: "Volumes inventory and host disk usage."
steps:
  - "volumes $SEL"
  - "exec $SEL -- df -hP"
"#;

    const NETWORK_AUDIT: &str = r#"
name: network-audit
description: "Networks, ports, and live connectivity probe."
steps:
  - "network $SEL"
  - "ports $SEL"
  - "connectivity $SEL --probe"
"#;

    const LOG_ROUNDUP: &str = r#"
name: log-roundup
description: "Recent error and warning lines across services in the namespace."
steps:
  - "search '{server=\"$SEL\", source=\"logs\"} |~ \"(?i)(error|warn)\"' --since 15m --tail 200"
"#;

    const HEALTH_EVERYTHING: &str = r#"
name: health-everything
description: "Inventory + health probe across the namespace."
steps:
  - "status $SEL"
  - "health $SEL"
"#;

    pub fn find(name: &str) -> Option<&'static str> {
        match name {
            "deploy-check" => Some(DEPLOY_CHECK),
            "disk-audit" => Some(DISK_AUDIT),
            "network-audit" => Some(NETWORK_AUDIT),
            "log-roundup" => Some(LOG_ROUNDUP),
            "health-everything" => Some(HEALTH_EVERYTHING),
            _ => None,
        }
    }

    pub fn names() -> Vec<&'static str> {
        vec![
            "deploy-check",
            "disk-audit",
            "network-audit",
            "log-roundup",
            "health-everything",
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_split_basic() {
        assert_eq!(
            shell_split("status arte/pulse").unwrap(),
            vec!["status", "arte/pulse"]
        );
    }

    #[test]
    fn shell_split_single_quotes_preserve_spaces() {
        let v = shell_split("search '{server=\"arte\", source=\"logs\"} |= \"err\"' --since 5m")
            .unwrap();
        assert_eq!(v[0], "search");
        assert_eq!(v[1], r#"{server="arte", source="logs"} |= "err""#);
        assert_eq!(v[2], "--since");
        assert_eq!(v[3], "5m");
    }

    #[test]
    fn shell_split_double_quotes_with_escape() {
        let v = shell_split(r#"echo "hello \"world\"""#).unwrap();
        assert_eq!(v, vec!["echo", "hello \"world\""]);
    }

    #[test]
    fn shell_split_unterminated_errors() {
        assert!(shell_split("status 'arte/pulse").is_err());
    }

    #[test]
    fn builtin_pack_resolves() {
        for n in builtin::names() {
            let body = builtin::find(n).expect(n);
            let doc = parse_recipe(body, n).expect(n);
            assert_eq!(doc.name, n);
            assert!(!doc.steps.is_empty());
        }
    }

    #[test]
    fn substitute_replaces_sel() {
        let mut argv = vec!["status".to_string(), "$SEL".to_string()];
        let args = RecipeArgs {
            name: "x".into(),
            sel: Some("arte".into()),
            apply: false,
            format: crate::format::FormatArgs::default(),
        };
        substitute_placeholders(&mut argv, &args);
        assert_eq!(argv, vec!["status", "arte"]);
    }
}
