//! `inspect disconnect <ns>` — close the persistent SSH master.

use crate::cli::DisconnectArgs;
use crate::commands::list::json_string;
use crate::config::namespace::validate_namespace_name;
use crate::config::resolver;
use crate::error::ExitKind;
use crate::ssh::master::{check_socket, exit_master, socket_path, MasterStatus};
use crate::ssh::SshTarget;

pub fn run(args: DisconnectArgs) -> anyhow::Result<ExitKind> {
    validate_namespace_name(&args.namespace)?;
    let resolved = resolver::resolve(&args.namespace)?;
    let target = SshTarget::from_resolved(&resolved)?;
    let socket = socket_path(&resolved.name);

    let prior = check_socket(&socket, &target);
    let mut closed = false;
    if matches!(prior, MasterStatus::Alive | MasterStatus::Stale) {
        exit_master(&socket, &target)?;
        closed = true;
    }

    if args.format.is_json() {
        println!(
            "{{\"schema_version\":1,\"namespace\":{ns},\"prior\":{prior},\"closed\":{closed}}}",
            ns = json_string(&resolved.name),
            prior = json_string(prior.label()),
            closed = if closed { "true" } else { "false" }
        );
        return Ok(ExitKind::Success);
    }

    if closed {
        println!("SUMMARY: '{}' disconnected (was {})", resolved.name, prior.label());
    } else {
        println!("SUMMARY: '{}' had no inspect-managed master", resolved.name);
    }
    println!("DATA:    socket {}", socket.display());
    println!("NEXT:    inspect connect {}", resolved.name);
    Ok(ExitKind::Success)
}
