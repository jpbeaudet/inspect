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

    let cfg = NamespaceConfig {
        env: None,
        auto_reauth: None,
        host: Some(host),
        user: Some(user),
        port,
        key_path: Some(key_path),
        key_passphrase_env,
        key_inline: None,
    };
    cfg.validate(&args.namespace)?;

    servers.namespaces.insert(args.namespace.clone(), cfg);
    file::save(&servers).context("writing servers.toml")?;

    println!(
        "SUMMARY: namespace '{}' {} in ~/.inspect/servers.toml",
        args.namespace,
        if exists { "updated" } else { "added" }
    );
    println!("DATA:    host, user, port, key_path stored (passphrases never on disk)");
    println!(
        "NEXT:    inspect test {} && inspect connect {}",
        args.namespace, args.namespace
    );
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
        return Err(anyhow!(
            "missing required value for '{label}' in non-interactive mode"
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
