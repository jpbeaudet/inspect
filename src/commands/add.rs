//! `inspect add <ns>` — register or update a namespace.

use std::io::{self, BufRead, Write};

use anyhow::{anyhow, Context};

use crate::cli::AddArgs;
use crate::config::file::{self, ServersFile};
use crate::config::namespace::{validate_namespace_name, NamespaceConfig};
use crate::error::{ConfigError, ExitKind};

pub fn run(args: AddArgs) -> anyhow::Result<ExitKind> {
    validate_namespace_name(&args.namespace)?;

    let mut servers = file::load().or_else(|e| match e {
        ConfigError::UnsafePermissions { .. } => Err(e),
        _ => Ok(ServersFile::default()),
    })?;

    let exists = servers.namespaces.contains_key(&args.namespace);
    if exists && !args.force {
        return Err(anyhow!(ConfigError::NamespaceExists(
            args.namespace.clone()
        )));
    }

    // A `--type k8s` namespace collects kubeconfig /
    // context / namespace and skips the SSH host/user/key prompts
    // entirely (it is addressed by its kubeconfig context, not SSH).
    let is_k8s = matches!(
        args.runtime_type
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("k8s") | Some("kubernetes")
    );

    let cfg = if is_k8s {
        let context = collect_value(
            "context (kubeconfig context, optional but recommended)",
            args.context.as_deref(),
            args.non_interactive,
            true,
        )?;
        let kubeconfig = collect_value(
            "kubeconfig (path, optional — blank uses kubectl default)",
            args.kubeconfig.as_deref(),
            args.non_interactive,
            true,
        )?;
        let k8s_namespace = collect_value(
            "namespace (k8s namespace, optional — blank uses default)",
            args.k8s_namespace.as_deref(),
            args.non_interactive,
            true,
        )?;
        NamespaceConfig {
            runtime_type: Some("k8s".to_string()),
            kubeconfig,
            context,
            k8s_namespace,
            ..Default::default()
        }
    } else {
        let host = collect_value("host", args.host.as_deref(), args.non_interactive, false)?
            .ok_or_else(|| anyhow!("host is required"))?;
        let user = collect_value("user", args.user.as_deref(), args.non_interactive, false)?
            .ok_or_else(|| anyhow!("user is required"))?;
        let key_path = collect_value(
            "key_path",
            args.key_path.as_deref(),
            args.non_interactive,
            false,
        )?
        .ok_or_else(|| anyhow!("key_path is required"))?;
        let key_passphrase_env = collect_value(
            "key_passphrase_env (optional, env var name)",
            args.key_passphrase_env.as_deref(),
            args.non_interactive,
            true,
        )?;
        let port = match args.port {
            Some(p) => Some(p),
            None if args.non_interactive => None,
            None => prompt_optional_u16("port (default 22)")?,
        };
        NamespaceConfig {
            host: Some(host),
            user: Some(user),
            port,
            key_path: Some(key_path),
            key_passphrase_env,
            ..Default::default()
        }
    };
    cfg.validate(&args.namespace)?;

    servers.namespaces.insert(args.namespace.clone(), cfg);
    file::save(&servers).context("writing servers.toml")?;

    // Report the RESOLVED servers.toml path, not a literal
    // `~/.inspect/servers.toml`. When `INSPECT_HOME` relocates config, the
    // hardcoded string lied about where the write landed — a silent
    // "what I said" vs "what I did" divergence that traps an agent going to
    // edit the file. `servers_toml_display()` shows the real path.
    println!(
        "SUMMARY: namespace '{}' {} in {}",
        args.namespace,
        if exists { "updated" } else { "added" },
        crate::paths::servers_toml_display()
    );
    if is_k8s {
        println!("DATA:    type=k8s; context, kubeconfig, namespace stored (kubeconfig inherits kubectl auth)");
    } else {
        println!("DATA:    host, user, port, key_path stored (passphrases never on disk)");
    }
    // The NEXT hint must be runtime-aware. A k8s namespace
    // is sessionless (kubeconfig is stateless) — suggesting `inspect
    // connect` there is a mindtrap: `connect` requires an SSH host and
    // fails with `namespace has no host` (exit 2). Only docker namespaces
    // get the `connect` step.
    if is_k8s {
        println!(
            "NEXT:    inspect test {} (k8s is sessionless — no `connect` step)",
            args.namespace
        );
    } else {
        println!(
            "NEXT:    inspect test {} && inspect connect {}",
            args.namespace, args.namespace
        );
    }
    Ok(ExitKind::Success)
}

fn collect_value(
    label: &str,
    flag_value: Option<&str>,
    non_interactive: bool,
    optional: bool,
) -> anyhow::Result<Option<String>> {
    if let Some(v) = flag_value {
        let trimmed = v.trim();
        if trimmed.is_empty() {
            if optional {
                return Ok(None);
            }
            return Err(anyhow!("{label} is empty"));
        }
        return Ok(Some(trimmed.to_string()));
    }
    if non_interactive {
        if optional {
            return Ok(None);
        }
        // Chain to recovery: tell the operator the exact flag to pass.
        // The `label` carries any parenthetical context, so for the
        // optional-only `key_passphrase_env (optional, env var name)`
        // we strip the parenthetical to keep the flag suggestion tight.
        let flag = label
            .split_whitespace()
            .next()
            .unwrap_or(label)
            .replace('_', "-");
        return Err(anyhow!(
            "missing required value for '{label}' in non-interactive mode\n\
             hint: pass `--{flag} <value>` on the command line \
             (env vars like INSPECT_<NS>_HOST are not consulted)"
        ));
    }
    prompt_string(label, optional)
}

fn prompt_string(label: &str, optional: bool) -> anyhow::Result<Option<String>> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    write!(stdout, "{label}: ").ok();
    stdout.flush().ok();
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    let trimmed = line.trim().to_string();
    if trimmed.is_empty() {
        if optional {
            return Ok(None);
        }
        return Err(anyhow!("{label} is required"));
    }
    Ok(Some(trimmed))
}

fn prompt_optional_u16(label: &str) -> anyhow::Result<Option<u16>> {
    let v = prompt_string(label, true)?;
    match v {
        None => Ok(None),
        Some(s) => Ok(Some(
            s.parse::<u16>().map_err(|_| anyhow!("invalid port: {s}"))?,
        )),
    }
}
