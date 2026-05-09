//! `inspect disconnect <ns>` — close the persistent SSH master.

use serde_json::json;

use crate::cli::DisconnectArgs;
use crate::config::namespace::validate_namespace_name;
use crate::config::resolver;
use crate::error::ExitKind;
use crate::safety::audit::{AuditEntry, AuditStore, Revert};
use crate::ssh::master::{check_socket, exit_master, socket_path, MasterStatus};
use crate::ssh::SshTarget;
use crate::verbs::output::{NextStep, OutputDoc};

pub fn run(args: DisconnectArgs) -> anyhow::Result<ExitKind> {
    validate_namespace_name(&args.namespace)?;

    // P8-C fix (v0.1.3): pair with the corresponding `set_namespace`
    // in `connect.rs` so disconnect also lands in the per-ns
    // transcript log. Set early — before resolver::resolve so even a
    // resolution failure produces a transcript block recording the
    // attempt.
    crate::transcript::set_namespace(&args.namespace);

    let resolved = resolver::resolve(&args.namespace)?;
    let target = SshTarget::from_resolved(&resolved)?;
    let socket = socket_path(&resolved.name);

    let prior = check_socket(&socket, &target);
    let mut closed = false;
    if matches!(prior, MasterStatus::Alive | MasterStatus::Stale) {
        exit_master(&socket, &target)?;
        closed = true;
    }

    // P8-C fix (v0.1.3): write an audit entry for the disconnect
    // itself so the F18 transcript footer carries an
    // `audit_id=<id>` cross-link. Same rationale as `connect`: gives
    // `audit grep verb=disconnect` a cleanly enumerable forensic
    // surface, and a `revert` preview pointing at the inverse
    // (`inspect connect <ns>`). Always emitted, even when the
    // socket was missing — the audit log captures the *attempt*,
    // and `prior=missing,closed=false` is itself a useful forensic
    // signal ("the operator thought a master was alive; it
    // wasn't"). Best-effort: a closed socket is not undone by an
    // audit-write failure.
    if let Ok(store) = AuditStore::open() {
        let mut e = AuditEntry::new("disconnect", &resolved.name);
        e.args = format!("prior={},closed={}", prior.label(), closed);
        e.revert = Some(Revert::unsupported(format!(
            "inspect connect {}",
            resolved.name
        )));
        let _ = store.append(&e);
    }

    if args.format.is_json() {
        // P0.6 sweep (v0.1.3): L7 envelope.
        let summary = if closed {
            format!("'{}' disconnected (was {})", resolved.name, prior.label())
        } else {
            format!("'{}' had no inspect-managed master", resolved.name)
        };
        let data = json!({
            "namespace": resolved.name,
            "prior": prior.label(),
            "closed": closed,
            "socket": socket.display().to_string(),
        });
        let mut doc = OutputDoc::new(summary, data);
        doc.push_next(NextStep::new(
            format!("inspect connect {}", resolved.name),
            "reopen the master",
        ));
        return doc.print_json(args.format.select_spec());
    }

    if closed {
        println!(
            "SUMMARY: '{}' disconnected (was {})",
            resolved.name,
            prior.label()
        );
    } else {
        println!("SUMMARY: '{}' had no inspect-managed master", resolved.name);
    }
    println!("DATA:    socket {}", socket.display());
    println!("NEXT:    inspect connect {}", resolved.name);
    Ok(ExitKind::Success)
}
