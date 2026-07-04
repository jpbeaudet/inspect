//! `inspect remove <ns>` — delete a namespace from `servers.toml`.
//!
//! Note: this does NOT clear environment variables; users in env-only mode
//! must unset the corresponding `INSPECT_<NS>_*` variables themselves.

use std::io::{self, BufRead, Write};

use anyhow::anyhow;

use crate::cli::RemoveArgs;
use crate::config::env;
use crate::config::file;
use crate::config::namespace::validate_namespace_name;
use crate::error::{ConfigError, ExitKind};

pub fn run(args: RemoveArgs) -> anyhow::Result<ExitKind> {
    validate_namespace_name(&args.namespace)?;

    let mut servers = file::load()?;
    let in_file = servers.namespaces.contains_key(&args.namespace);
    let env_namespaces = env::enumerate_env_namespaces();
    let in_env = env_namespaces.contains(&args.namespace);

    if !in_file && !in_env {
        return Err(anyhow!(ConfigError::UnknownNamespace(
            args.namespace.clone()
        )));
    }

    if in_file {
        if !args.yes && !confirm(&args.namespace)? {
            println!("SUMMARY: cancelled");
            return Ok(ExitKind::Success);
        }
        servers.namespaces.remove(&args.namespace);
        file::save(&servers)?;
        // WA-1 (v0.1.4): name the RESOLVED servers.toml path so an agent
        // knows exactly which file changed (honors INSPECT_HOME).
        println!(
            "SUMMARY: namespace '{}' removed from {}",
            args.namespace,
            crate::paths::servers_toml_display()
        );
    } else {
        println!(
            "SUMMARY: namespace '{}' is not in {} (file-side no-op)",
            args.namespace,
            crate::paths::servers_toml_display()
        );
    }

    if in_env {
        println!(
            "DATA:    namespace '{}' is also defined via INSPECT_{}_* env vars; \
             unset them in your shell to fully remove",
            args.namespace,
            args.namespace.to_ascii_uppercase()
        );
    }
    println!("NEXT:    inspect list");
    Ok(ExitKind::Success)
}

fn confirm(namespace: &str) -> anyhow::Result<bool> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    write!(stdout, "Remove namespace '{namespace}'? [y/N] ").ok();
    stdout.flush().ok();
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}
