//! `inspect list` — enumerate configured namespaces (env ∪ file).

use crate::cli::ListArgs;
use crate::config::namespace::NamespaceSource;
use crate::config::resolver;
use crate::error::ExitKind;

pub fn run(args: ListArgs) -> anyhow::Result<ExitKind> {
    let resolved = resolver::list_all()?;

    if args.format.is_json() {
        emit_json(&resolved);
        return Ok(ExitKind::Success);
    }

    if resolved.is_empty() {
        println!("SUMMARY: no namespaces configured");
        println!("DATA:    (none)");
        println!("NEXT:    inspect add <ns>");
        return Ok(ExitKind::Success);
    }

    println!("SUMMARY: {} namespace(s) configured", resolved.len());
    println!("DATA:");
    println!("  NAMESPACE             HOST                             USER           SOURCE");
    for r in &resolved {
        println!(
            "  {:<20}  {:<32} {:<14} {}",
            r.name,
            r.config.host.as_deref().unwrap_or("-"),
            r.config.user.as_deref().unwrap_or("-"),
            describe_source(r.source),
        );
    }
    println!("NEXT:    inspect show <ns>   inspect test <ns>");
    Ok(ExitKind::Success)
}

fn describe_source(s: NamespaceSource) -> &'static str {
    match s {
        NamespaceSource::EnvOnly => "env",
        NamespaceSource::FileOnly => "file",
        NamespaceSource::EnvOverFile => "env-over-file",
    }
}

fn emit_json(resolved: &[crate::config::ResolvedNamespace]) {
    // Hand-rolled JSON to keep Phase 0 dependency surface minimal.
    let mut s = String::from("{\"schema_version\":1,\"namespaces\":[");
    for (i, r) in resolved.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"name\":{name},\"host\":{host},\"user\":{user},\"port\":{port},\"source\":{src}}}",
            name = json_string(&r.name),
            host = json_opt_string(&r.config.host),
            user = json_opt_string(&r.config.user),
            port = r
                .config
                .port
                .map(|p| p.to_string())
                .unwrap_or_else(|| "null".into()),
            src = json_string(describe_source(r.source)),
        ));
    }
    s.push_str("]}");
    println!("{s}");
}

pub(crate) fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub(crate) fn json_opt_string(s: &Option<String>) -> String {
    match s {
        Some(v) => json_string(v),
        None => "null".to_string(),
    }
}
