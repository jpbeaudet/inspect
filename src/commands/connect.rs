//! `inspect connect <ns>` — open or reuse a persistent SSH master.

use anyhow::Context;

use crate::cli::ConnectArgs;
use crate::commands::list::json_string;
use crate::config::namespace::validate_namespace_name;
use crate::config::resolver;
use crate::error::ExitKind;
use crate::ssh::master::AuthSelection;
use crate::ssh::{self, SshTarget};

pub fn run(args: ConnectArgs) -> anyhow::Result<ExitKind> {
    validate_namespace_name(&args.namespace)?;
    let resolved = resolver::resolve(&args.namespace)?;
    resolved.config.validate(&resolved.name)?;
    let target = SshTarget::from_resolved(&resolved)?;

    let (ttl, ttl_source) = ssh::ttl::resolve(args.ttl.as_deref())?;

    let allow_interactive = !args.non_interactive;
    let passphrase_env = if args.interactive {
        None
    } else {
        resolved.config.key_passphrase_env.as_deref()
    };

    let outcome = ssh::start_master(
        &resolved.name,
        &target,
        &ttl,
        AuthSelection {
            passphrase_env,
            allow_interactive,
            skip_existing_mux_check: args.no_existing_mux,
        },
    )
    .with_context(|| format!("connect '{}'", resolved.name))?;

    if args.format.is_json() {
        let socket = outcome
            .socket
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let socket_json = if outcome.socket.is_some() {
            json_string(&socket)
        } else {
            "null".to_string()
        };
        println!(
            "{{\"schema_version\":1,\"namespace\":{ns},\"auth\":{auth},\
             \"socket\":{sock},\"ttl\":{ttl},\"ttl_source\":{src}}}",
            ns = json_string(&resolved.name),
            auth = json_string(outcome.auth_mode.label()),
            sock = socket_json,
            ttl = json_string(&outcome.ttl),
            src = json_string(ttl_source.label()),
        );
        return Ok(ExitKind::Success);
    }

    println!(
        "SUMMARY: '{}' connected via {} (ttl {}, {})",
        resolved.name,
        outcome.auth_mode.label(),
        outcome.ttl,
        ttl_source.label()
    );
    println!("DATA:");
    println!("  host:   {}@{}:{}", target.user, target.host, target.port);
    if let Some(sock) = &outcome.socket {
        println!("  socket: {}", sock.display());
    } else {
        println!("  socket: (delegated to user's existing ControlMaster)");
    }
    println!(
        "NEXT:    inspect connections    inspect disconnect {}",
        resolved.name
    );
    Ok(ExitKind::Success)
}
