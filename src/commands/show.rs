//! `inspect show <ns>` — display the resolved configuration with secrets
//! redacted.

use crate::cli::ShowArgs;
use crate::commands::list::{json_opt_string, json_string};
use crate::config::namespace::{validate_namespace_name, NamespaceSource};
use crate::config::resolver;
use crate::error::ExitKind;
use crate::redact;

pub fn run(args: ShowArgs) -> anyhow::Result<ExitKind> {
    validate_namespace_name(&args.namespace)?;
    let r = resolver::resolve(&args.namespace)?;

    if args.format.is_json() {
        let body = format!(
            "{{\"schema_version\":1,\"name\":{name},\"host\":{host},\"user\":{user},\
             \"port\":{port},\"key_path\":{key_path},\"key_passphrase_env\":{kpe},\
             \"key_inline\":{inline},\"source\":{src}}}",
            name = json_string(&r.name),
            host = json_opt_string(&r.config.host),
            user = json_opt_string(&r.config.user),
            port = r
                .config
                .port
                .map(|p| p.to_string())
                .unwrap_or_else(|| "null".into()),
            key_path = json_opt_string(&r.config.key_path),
            kpe = json_opt_string(&r.config.key_passphrase_env),
            // Never disclose the inline key value, even in JSON output.
            inline = if r.config.key_inline.is_some() {
                json_string(redact::REDACTED)
            } else {
                "null".to_string()
            },
            src = json_string(describe_source(r.source)),
        );
        println!("{body}");
        return Ok(ExitKind::Success);
    }

    println!("SUMMARY: namespace '{}' resolved from {}", r.name, describe_source(r.source));
    println!("DATA:");
    println!("  host:                {}", r.config.host.as_deref().unwrap_or("<unset>"));
    println!("  user:                {}", r.config.user.as_deref().unwrap_or("<unset>"));
    println!(
        "  port:                {}",
        r.config
            .port
            .map(|p| p.to_string())
            .unwrap_or_else(|| "22 (default)".to_string())
    );
    println!(
        "  key_path:            {}",
        r.config.key_path.as_deref().unwrap_or("<unset>")
    );
    println!(
        "  key_passphrase_env:  {}",
        r.config.key_passphrase_env.as_deref().unwrap_or("<unset>")
    );
    println!(
        "  key_inline:          {}",
        redact::redact_opt(&r.config.key_inline)
    );
    println!("NEXT:    inspect test {0}   inspect connect {0}", r.name);
    Ok(ExitKind::Success)
}

fn describe_source(s: NamespaceSource) -> &'static str {
    match s {
        NamespaceSource::EnvOnly => "env",
        NamespaceSource::FileOnly => "file",
        NamespaceSource::EnvOverFile => "env-over-file",
    }
}
