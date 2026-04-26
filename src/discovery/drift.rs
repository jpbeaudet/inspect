//! Async drift detection.
//!
//! Per the bible (§5.1), every command may trigger a non-blocking drift check
//! against the cached profile. We run a *cheap* fingerprint probe (just
//! `docker ps` IDs + image tags) on a background thread; if it diverges from
//! the cached fingerprint, we write a drift marker. Subsequent commands can
//! surface the marker as a `NEXT:` hint.
//!
//! Phase 2 ships the mechanism. Phases that own user-facing read verbs will
//! call drift detection from their hot paths.

use std::time::Duration;

use crate::profile::cache::{clear_drift_marker, load_profile, write_drift_marker};
use crate::profile::schema::Profile;
use crate::ssh::{run_remote, RunOpts, SshTarget};

/// Synchronous drift check. Used in tests and from `inspect setup --check-drift`.
pub fn run_drift_check(namespace: &str, target: &SshTarget) -> anyhow::Result<DriftStatus> {
    let cached = match load_profile(namespace)? {
        Some(p) => p,
        None => return Ok(DriftStatus::NoCache),
    };
    let cheap = match cheap_fingerprint(namespace, target) {
        Ok(fp) => fp,
        Err(_) => return Ok(DriftStatus::ProbeFailed),
    };
    let baseline = baseline_fingerprint(&cached);
    if cheap == baseline {
        clear_drift_marker(namespace);
        Ok(DriftStatus::Fresh)
    } else {
        write_drift_marker(namespace, &cheap, &baseline)?;
        Ok(DriftStatus::Drifted {
            current: cheap,
            cached: baseline,
        })
    }
}

/// Drift outcome.
#[derive(Debug, Clone)]
pub enum DriftStatus {
    NoCache,
    ProbeFailed,
    Fresh,
    Drifted { current: String, cached: String },
}

/// Cheap fingerprint of the live host. We just list container IDs and image
/// tags — not full inspect output. Stable, sortable, fast.
fn cheap_fingerprint(namespace: &str, target: &SshTarget) -> anyhow::Result<String> {
    let cmd = "docker ps --format '{{.ID}}\t{{.Image}}' 2>/dev/null | sort";
    let out = run_remote(
        namespace,
        target,
        cmd,
        RunOpts {
            timeout: Some(Duration::from_secs(8)),
        },
    )?;
    if !out.ok() {
        anyhow::bail!("cheap fingerprint probe exited {}", out.exit_code);
    }
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(out.stdout.as_bytes());
    let bytes = h.finalize();
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Project the cached profile down to the same shape as `cheap_fingerprint`
/// so the two can be compared apples-to-apples.
fn baseline_fingerprint(p: &Profile) -> String {
    let mut rows: Vec<String> = p
        .services
        .iter()
        .filter(|s| matches!(s.kind, crate::profile::schema::ServiceKind::Container))
        .map(|s| {
            format!(
                "{}\t{}",
                s.container_id.clone().unwrap_or_default(),
                s.image.clone().unwrap_or_default()
            )
        })
        .collect();
    rows.sort();
    // Match the wire format of `docker ps ... | sort` exactly: each row is
    // newline-terminated, and the empty case is the empty string (NOT "\n").
    let body: String = rows.iter().map(|r| format!("{r}\n")).collect();
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(body.as_bytes());
    let bytes = h.finalize();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
