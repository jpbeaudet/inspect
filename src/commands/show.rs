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

    // K3/WA-3 (v0.1.4, JP-2026-07-05): `show` is a pure CONFIG READ — it
    // ALWAYS displays the on-disk config plus a kubectl *readiness line*, and
    // NEVER hard-fails / breaks `--json` on an absent backend. Enforcement of
    // "kubectl must be present" belongs to the ACTION verbs (test / setup /
    // read / write), not to a read that reports state. Probe kubectl LOCALLY
    // (never over SSH — surface map §10) only to REPORT its status.
    let k8s_probe = if is_k8s {
        Some(crate::exec::kubectl::probe_kubectl())
    } else {
        None
    };

    if args.format.is_json() {
        // K2 (v0.1.4): schema 2 adds `type` + the k8s addressing fields.
        // SSH-only fields serialize as their real value (null for a k8s
        // namespace, since they are inert / unset there).
        let body = format!(
            "{{\"schema_version\":2,\"name\":{name},\"type\":{ty},\"host\":{host},\
             \"user\":{user},\"port\":{port},\"key_path\":{key_path},\
             \"key_passphrase_env\":{kpe},\"key_inline\":{inline},\"auth\":{auth},\
             \"password_env\":{pe},\"session_ttl\":{ttl},\"kubeconfig\":{kubeconfig},\
             \"context\":{context},\"namespace\":{k8sns},\
             \"kubectl_available\":{kavail},\"kubectl_version\":{kver},\"source\":{src}}}",
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
            // K3 (v0.1.4): kubectl backend readiness. For k8s namespaces
            // the preflight above guarantees availability (absent bails),
            // so this is `true` with the detected client version; docker
            // namespaces carry `false`/null (the field is inert there).
            // WA-3: reflect the ACTUAL probe, not a hardcoded `true` — `show`
            // no longer preflights, so kubectl may genuinely be absent.
            kavail = k8s_probe
                .as_ref()
                .map(|p| if p.available { "true" } else { "false" })
                .unwrap_or("false"),
            kver = k8s_probe
                .as_ref()
                .and_then(|p| p.version.as_ref())
                .map(|v| json_string(v))
                .unwrap_or_else(|| "null".to_string()),
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
        // WA-3 (v0.1.4): the kubectl backend readiness line — reported, not
        // enforced. Present → version; absent → NOT FOUND + the fix, so an
        // operator sees exactly why an action verb will refuse without `show`
        // itself failing.
        if let Some(p) = &k8s_probe {
            if p.available {
                match &p.version {
                    Some(v) => println!("  kubectl:             {v}"),
                    None => println!("  kubectl:             present (version unknown)"),
                }
            } else {
                println!(
                    "  kubectl:             NOT FOUND — install kubectl \
                     (https://kubernetes.io/docs/tasks/tools/) to run k8s verbs"
                );
            }
        }
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
            // Match the `field:` colon + column alignment of the k8s
            // fields above so the DATA block reads as one consistent
            // key/value list (no colon on some rows = an agent trap).
            println!("  {:<21}N/A (k8s)", format!("{field}:"));
        }
        // K3 (v0.1.4): a below-floor kubectl warns (never fails) — every
        // k8s verb inspect drives works far below the floor; this is a
        // heads-up, not a gate.
        if let Some(w) = k8s_probe.as_ref().and_then(|p| p.floor_warning()) {
            println!("WARNING: {w}");
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
