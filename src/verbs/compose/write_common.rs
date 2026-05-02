//! F6 (v0.1.3): shared scaffolding for the audited compose write
//! verbs (`up` / `down` / `pull` / `build` / `restart`).
//!
//! The four verbs ending in `up` / `down` / `pull` / `build` follow
//! a near-identical shape:
//!
//! 1. Parse `<ns>/<project>[/<service>]`.
//! 2. Look up the project in the cached profile.
//! 3. Compute a 12-hex `compose_file_hash` from the live remote
//!    file body so the audit entry can be cross-verified later.
//! 4. Honor the `--apply` / dry-run gate.
//! 5. Dispatch the docker compose command (buffered or streaming).
//! 6. Append a single audit entry with `verb=compose.<sub>` plus
//!    the standard `[project=…] [compose_file_hash=…]` arg tags.
//!
//! `restart` does its own iteration because it fans out per-service
//! and uses the safety-gate prompt; the rest are single-shot.

use crate::profile::schema::ComposeProject;
use crate::ssh::exec::RunOpts;
use crate::ssh::options::SshTarget;
use crate::verbs::quote::shquote;
use crate::verbs::runtime::RemoteRunner;

/// Fetch the project's compose file body and return the first 12
/// hex chars of its SHA-256. Returns `String::new()` on any
/// failure — the audit entry still records project + service so a
/// best-effort hash is enough.
pub(crate) fn compose_file_sha_short(
    runner: &dyn RemoteRunner,
    ns: &str,
    target: &SshTarget,
    project: &ComposeProject,
) -> String {
    if project.compose_file.is_empty() {
        return String::new();
    }
    let cmd = format!("cat {f} 2>/dev/null", f = shquote(&project.compose_file));
    let out = match runner.run(ns, target, &cmd, RunOpts::with_timeout(15)) {
        Ok(o) if o.ok() => o,
        _ => return String::new(),
    };
    let hex = crate::safety::snapshot::sha256_hex(out.stdout.as_bytes());
    hex.chars().take(12).collect()
}

/// Build the standard audit args tag string for a project-scoped
/// compose write. The optional service portion is included only
/// when present (per-service pull / build / restart). The hash is
/// elided when the cat probe failed so we don't stamp an empty tag.
pub(crate) fn project_tags(
    project: &str,
    service: Option<&str>,
    compose_hash: &str,
    extras: &[&str],
) -> String {
    let mut out = format!("[project={project}]");
    if let Some(svc) = service {
        out.push_str(&format!(" [service={svc}]"));
    }
    if !compose_hash.is_empty() {
        out.push_str(&format!(" [compose_file_hash={compose_hash}]"));
    }
    for tag in extras {
        out.push(' ');
        out.push_str(tag);
    }
    out
}

/// Render the canonical `cd <wd> && docker compose -p <p> <sub>
/// [args] [service]` command for the compose write verbs. `extra`
/// is the per-verb flag set already shquoted; `service` narrows the
/// invocation when set.
pub(crate) fn build_compose_cmd(
    project: &ComposeProject,
    sub: &str,
    extra_flags: &[&str],
    service: Option<&str>,
) -> String {
    let mut parts: Vec<String> = vec![
        format!("cd {wd} &&", wd = shquote(&project.working_dir)),
        format!("docker compose -p {p} {sub}", p = shquote(&project.name)),
    ];
    for f in extra_flags {
        parts.push((*f).to_string());
    }
    if let Some(svc) = service {
        parts.push(shquote(svc));
    }
    parts.join(" ")
}

/// L8 (v0.1.3): per-service teardown command shape. `docker compose
/// down <svc>` is not a documented form and behaves inconsistently
/// across compose versions; the explicit two-step (`stop <svc> && rm
/// -f <svc>`) is what every operator's runbook uses and what
/// docker-compose-down's per-service semantics actually mean. Other
/// services in the project remain running. `--volumes` and `--rmi`
/// are rejected at the verb layer (see `compose down`'s caller).
pub(crate) fn build_compose_per_service_down_cmd(
    project: &ComposeProject,
    service: &str,
) -> String {
    let wd = shquote(&project.working_dir);
    let p = shquote(&project.name);
    let svc = shquote(service);
    format!("cd {wd} && docker compose -p {p} stop {svc} && docker compose -p {p} rm -f {svc}",)
}
