//! F6 (v0.1.3): `inspect compose ps <ns>/<project>` — per-service
//! status table for one compose project.
//!
//! Wraps `docker compose -p <project> ps --all --format json` over
//! the persistent ssh socket. The `cd <working_dir>` prefix is what
//! lets compose resolve relative `volumes` / `env_file` paths
//! correctly — without it, replicas of the same project stored
//! under different paths would produce inconsistent output.

use anyhow::Result;
use serde_json::{json, Value};

use crate::cli::ComposePsArgs;
use crate::error::ExitKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::output::{NextStep, OutputDoc};
use crate::verbs::quote::shquote;
use crate::verbs::runtime::{current_runner, resolve_target};

use super::resolve::{project_in_profile, Parsed};

pub fn run(args: ComposePsArgs) -> Result<ExitKind> {
    let fmt = args.format.resolve()?;
    let parsed = match Parsed::parse(&args.selector) {
        Ok(p) => p,
        Err(e) => {
            crate::error::emit(format!("{e}"));
            return Ok(ExitKind::Error);
        }
    };
    let project_name = match parsed.project.as_deref() {
        Some(p) => p,
        None => {
            crate::error::emit(format!(
                "selector '{}' is missing the project portion — \
                 expected '<ns>/<project>'",
                args.selector
            ));
            return Ok(ExitKind::Error);
        }
    };
    let (_profile, project) = match project_in_profile(&parsed.namespace, project_name) {
        Ok(t) => t,
        Err(e) => {
            crate::error::emit(format!("{e}"));
            return Ok(ExitKind::NoMatches);
        }
    };

    let runner = current_runner();
    let (_resolved, target) = resolve_target(&parsed.namespace)?;
    // `cd <wd> &&` so relative paths in the compose file resolve;
    // `--all` so stopped services show up too (an exited replica is
    // exactly what the operator wants `compose ps` to surface).
    let cmd = format!(
        "cd {wd} && docker compose -p {p} ps --all --format json 2>/dev/null",
        wd = shquote(&project.working_dir),
        p = shquote(&project.name),
    );
    let out = runner.run(&parsed.namespace, &target, &cmd, RunOpts::with_timeout(20))?;
    if !out.ok() {
        crate::error::emit(format!(
            "docker compose ps exited {} on {}/{}: {}",
            out.exit_code,
            parsed.namespace,
            project.name,
            out.stderr.trim()
        ));
        return Ok(ExitKind::Error);
    }

    // `docker compose ps --format json` emits either a JSON array or
    // newline-delimited JSON objects depending on the docker version.
    // Handle both — modern v2 emits ndjson, older emits a single array.
    let services = parse_ps_output(out.stdout.trim());
    let mut data_lines: Vec<String> = Vec::new();
    let mut json_services: Vec<Value> = Vec::new();
    let mut running = 0usize;
    for svc in &services {
        if svc.state.eq_ignore_ascii_case("running") {
            running += 1;
        }
        data_lines.push(format!(
            "{name:<24} {state:<10} {image:<40} {ports}",
            name = svc.service,
            state = svc.state,
            image = svc.image,
            ports = svc.ports,
        ));
        json_services.push(json!({
            "service": svc.service,
            "state": svc.state,
            "image": svc.image,
            "ports": svc.ports,
            "uptime": svc.uptime,
        }));
    }

    let total = services.len();
    let summary = format!(
        "{total} service(s) in {ns}/{p}: {running} running, {down} not running",
        ns = parsed.namespace,
        p = project.name,
        down = total.saturating_sub(running),
    );

    let mut doc = OutputDoc::new(
        summary,
        json!({
            "namespace": parsed.namespace,
            "project": project.name,
            "working_dir": project.working_dir,
            "compose_file": project.compose_file,
            "services": json_services,
        }),
    )
    .with_meta("selector", args.selector.clone())
    .with_quiet(args.format.quiet);

    if running < total {
        doc.push_next(NextStep::new(
            format!(
                "inspect compose logs {ns}/{p} --tail 200",
                ns = parsed.namespace,
                p = project.name
            ),
            "show recent logs from every service in the project",
        ));
    }

    crate::format::render::render_doc(&doc, &fmt, &data_lines, args.format.select_spec())
}

/// One row of `docker compose ps`.
struct PsRow {
    service: String,
    state: String,
    image: String,
    ports: String,
    uptime: String,
}

/// Parse `docker compose ps --format json` output. Tolerates both
/// the modern ndjson form (one object per line) and the older
/// single-array form. Field names are capitalized in modern docker
/// (`Service`, `State`, `Image`, `Publishers`, `Status`); fall back
/// to lowercase for tolerance.
fn parse_ps_output(raw: &str) -> Vec<PsRow> {
    let mut rows = Vec::new();
    if raw.is_empty() {
        return rows;
    }
    // Try array first.
    if raw.starts_with('[') {
        if let Ok(serde_json::Value::Array(arr)) = serde_json::from_str::<Value>(raw) {
            for entry in arr {
                if let Some(row) = ps_row_from_value(&entry) {
                    rows.push(row);
                }
            }
            return rows;
        }
    }
    // Fallback: one JSON object per line.
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if let Some(row) = ps_row_from_value(&v) {
                rows.push(row);
            }
        }
    }
    rows
}

fn ps_row_from_value(v: &Value) -> Option<PsRow> {
    let service = v
        .get("Service")
        .or_else(|| v.get("service"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    if service.is_empty() {
        return None;
    }
    let state = v
        .get("State")
        .or_else(|| v.get("state"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let image = v
        .get("Image")
        .or_else(|| v.get("image"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    // Modern docker: `Publishers` array of {URL,TargetPort,PublishedPort,Protocol}.
    let ports = format_publishers(v);
    let uptime = v
        .get("Status")
        .or_else(|| v.get("status"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    Some(PsRow {
        service,
        state,
        image,
        ports,
        uptime,
    })
}

/// Render `Publishers` (modern compose v2) into a comma-separated
/// `host:container/proto` string. Falls back to the literal `Ports`
/// field on older daemons that emit a flat string instead.
fn format_publishers(v: &Value) -> String {
    if let Some(arr) = v.get("Publishers").and_then(|x| x.as_array()) {
        let mut out = Vec::with_capacity(arr.len());
        for p in arr {
            let host = p.get("PublishedPort").and_then(|x| x.as_u64()).unwrap_or(0);
            let cont = p.get("TargetPort").and_then(|x| x.as_u64()).unwrap_or(0);
            let proto = p.get("Protocol").and_then(|x| x.as_str()).unwrap_or("tcp");
            if host == 0 && cont == 0 {
                continue;
            }
            out.push(format!("{host}:{cont}/{proto}"));
        }
        return out.join(",");
    }
    v.get("Ports")
        .or_else(|| v.get("ports"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ps_handles_modern_ndjson() {
        let raw = r#"{"Service":"onyx-vault","State":"running","Image":"vault:1.15","Publishers":[{"PublishedPort":8200,"TargetPort":8200,"Protocol":"tcp"}],"Status":"Up 2 hours"}
{"Service":"pulse","State":"exited","Image":"pulse:1.4","Publishers":[],"Status":"Exited (0) 5 minutes ago"}"#;
        let rows = parse_ps_output(raw);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].service, "onyx-vault");
        assert_eq!(rows[0].state, "running");
        assert_eq!(rows[0].ports, "8200:8200/tcp");
        assert_eq!(rows[1].state, "exited");
        assert!(rows[1].ports.is_empty());
    }

    #[test]
    fn parse_ps_handles_legacy_array() {
        let raw = r#"[{"Service":"a","State":"running","Image":"i:1","Publishers":[]}]"#;
        let rows = parse_ps_output(raw);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].service, "a");
    }

    #[test]
    fn parse_ps_skips_nameless_and_empty() {
        assert!(parse_ps_output("").is_empty());
        assert!(parse_ps_output("[]").is_empty());
        let raw = r#"{"State":"running"}"#; // no Service field
        assert!(parse_ps_output(raw).is_empty());
    }
}
