//! `inspect show <ns>` — display the resolved configuration with secrets
//! redacted.

use crate::cli::ShowArgs;
use crate::commands::list::{json_opt_string, json_string};
use crate::config::namespace::{validate_namespace_name, NamespaceSource};
use crate::config::resolver;
use crate::error::ExitKind;
use crate::exec::runtime::RuntimeKind;
use crate::redact;

pub fn run(args: ShowArgs) -> anyhow::Result<ExitKind> {
    validate_namespace_name(&args.namespace)?;
    let r = resolver::resolve(&args.namespace)?;
    // Surface config-validation errors from `show` so a
    // typo in `auth` / `password_env` / `session_ttl` is loud at
    // inspection time rather than waiting for the next `connect`.
    r.config.validate(&r.name)?;

    let is_k8s = r.config.runtime_kind() == RuntimeKind::K8s;

    if args.format.is_json() {
        // K2 (v0.1.4): schema 2 adds `type` + the k8s addressing fields.
        // SSH-only fields serialize as their real value (null for a k8s
        // namespace, since they are inert / unset there).
        let body = format!(
            "{{\"schema_version\":2,\"name\":{name},\"type\":{ty},\"host\":{host},\
             \"user\":{user},\"port\":{port},\"key_path\":{key_path},\
             \"key_passphrase_env\":{kpe},\"key_inline\":{inline},\"auth\":{auth},\
             \"password_env\":{pe},\"session_ttl\":{ttl},\"kubeconfig\":{kubeconfig},\
             \"context\":{context},\"namespace\":{k8sns},\"source\":{src}}}",
            name = json_string(&r.name),
            ty = json_string(if is_k8s { "k8s" } else { "docker" }),
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
            auth = json_opt_string(&r.config.auth),
            pe = json_opt_string(&r.config.password_env),
            ttl = json_opt_string(&r.config.session_ttl),
            kubeconfig = json_opt_string(&r.config.kubeconfig),
            context = json_opt_string(&r.config.context),
            k8sns = json_opt_string(&r.config.k8s_namespace),
            src = json_string(describe_source(r.source)),
        );
        println!("{body}");
        return Ok(ExitKind::Success);
    }

    println!(
        "SUMMARY: namespace '{}' resolved from {} (type: {})",
        r.name,
        describe_source(r.source),
        if is_k8s { "k8s" } else { "docker" }
    );
    println!("DATA:");

    if is_k8s {
        // Kubernetes namespace: show the k8s addressing; the SSH-only
        // fields are inert here and render N/A rather than <unset>, so
        // an operator doesn't read a blank as "misconfigured".
        println!("  type:                k8s");
        println!(
            "  context:             {}",
            r.config.context.as_deref().unwrap_or("<current-context>")
        );
        println!(
            "  kubeconfig:          {}",
            r.config
                .kubeconfig
                .as_deref()
                .unwrap_or("<kubectl default>")
        );
        println!(
            "  namespace:           {}",
            r.config.k8s_namespace.as_deref().unwrap_or("<default>")
        );
        for field in [
            "host",
            "user",
            "port",
            "auth",
            "key_path",
            "key_passphrase_env",
            "key_inline",
            "password_env",
            "session_ttl",
        ] {
            println!("  {field:<19} N/A (k8s)");
        }
        println!("NEXT:    inspect test {0}   inspect setup {0}", r.name);
        return Ok(ExitKind::Success);
    }

    println!("  type:                docker");
    println!(
        "  host:                {}",
        r.config.host.as_deref().unwrap_or("<unset>")
    );
    println!(
        "  user:                {}",
        r.config.user.as_deref().unwrap_or("<unset>")
    );
    println!(
        "  port:                {}",
        r.config
            .port
            .map(|p| p.to_string())
            .unwrap_or_else(|| "22 (default)".to_string())
    );
    println!(
        "  auth:                {}",
        r.config.auth.as_deref().unwrap_or("key (default)")
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
    println!(
        "  password_env:        {}",
        r.config.password_env.as_deref().unwrap_or("<unset>")
    );
    println!(
        "  session_ttl:         {}",
        r.config.session_ttl.as_deref().unwrap_or("<unset>")
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
