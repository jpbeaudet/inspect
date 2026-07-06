//! Kubernetes discovery — the local, kubectl-based analogue of
//! the SSH/docker discovery in this module. One API round-trip
//! (`kubectl get pods -o json`, context-pinned) is parsed into the
//! shared [`Profile`] / [`Service`] model so every downstream read verb
//! consumes k8s the same way it consumes docker.
//!
//! Scope note: this first increment maps **pods → services** (the
//! container-equivalent). Service/Endpoint ports, PVC volumes, and image
//! inventory are the province of the dedicated
//! `ports`/`network`/`volumes`/`images` verbs; deployment grouping + richer
//! health rollup land in a later increment. What ships here is
//! the discovery seam + the pod inventory that `setup`/`profile`/`status` need.

use anyhow::{Context, Result};

use crate::config::namespace::NamespaceConfig;
use crate::profile::schema::{HealthStatus, Profile, Service, ServiceKind};

/// Run k8s discovery for a namespace and return a populated [`Profile`].
/// The kubectl command pins `--context` (and `--kubeconfig`) explicitly — it
/// never reads the ambient current-context (anti-footgun invariant).
pub fn discover_k8s(name: &str, cfg: &NamespaceConfig, discovered_at: &str) -> Result<Profile> {
    let context = cfg
        .context
        .as_deref()
        .context("k8s namespace has no context (validate() should have rejected this)")?;

    let mut cmd = std::process::Command::new("kubectl");
    cmd.args(["--context", context]);
    if let Some(kc) = cfg.kubeconfig.as_deref() {
        cmd.args(["--kubeconfig", &expand_tilde(kc)]);
    }
    if let Some(ns) = cfg.k8s_namespace.as_deref() {
        cmd.args(["-n", ns]);
    }
    cmd.args([
        "get",
        "pods",
        "-o",
        "json",
        crate::exec::kubectl::READ_REQUEST_TIMEOUT,
    ]);

    let out = cmd
        .output()
        .context("failed to spawn kubectl for discovery")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let fc =
            crate::exec::kubectl::classify_kubectl_failure(&stderr, out.status.code().unwrap_or(1));
        anyhow::bail!(
            "k8s discovery failed [{}] {}",
            fc.failure_class(),
            fc.hint("")
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let services = parse_pods(&stdout);

    // `host` carries the cluster identity for a k8s profile (the context),
    // mirroring how a docker profile carries the SSH host.
    let mut profile = Profile::empty(name, context, discovered_at);
    profile.runtime = Some("k8s".into());
    profile.services = services;
    Ok(profile)
}

/// Pure parser: `kubectl get pods -o json` → `Vec<Service>`. Each pod becomes a
/// service whose `container_name` is the pod name (the value every kubectl
/// verb addresses). Kept pure + total (a malformed item is skipped, never
/// panics) so it is unit-testable against real captured cluster JSON.
pub fn parse_pods(json: &str) -> Vec<Service> {
    let root: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let items = root.get("items").and_then(|i| i.as_array());
    let items = match items {
        Some(a) => a,
        None => return Vec::new(),
    };
    let mut services = Vec::new();
    for pod in items {
        let name = pod
            .pointer("/metadata/name")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if name.is_empty() {
            continue;
        }
        let (phase, health_status) = pod_health(pod);
        let image = pod
            .pointer("/spec/containers/0/image")
            .and_then(|v| v.as_str())
            .map(String::from);
        services.push(Service {
            name: name.to_string(),
            container_name: name.to_string(),
            compose_service: None,
            container_id: None,
            image,
            ports: Vec::new(),
            health: phase,
            health_status,
            log_driver: None,
            log_readable_directly: false,
            mounts: Vec::new(),
            kind: ServiceKind::Container,
            depends_on: Vec::new(),
            discovery_incomplete: false,
        });
    }
    services
}

/// Map a pod's phase + container readiness into inspect's `HealthStatus`
/// rollup, returning `(phase_string, status)`. A CrashLoopBackOff / Error
/// waiting-reason on any container is Unhealthy even while the phase is still
/// "Running" (the pod object lags the container state) — this is what an
/// operator means by "unhealthy" and drives the `status` rollup + `why`.
fn pod_health(pod: &serde_json::Value) -> (Option<String>, Option<HealthStatus>) {
    let phase = pod
        .pointer("/status/phase")
        .and_then(|v| v.as_str())
        .map(String::from);
    let statuses = pod
        .pointer("/status/containerStatuses")
        .and_then(|v| v.as_array());
    let (all_ready, any_crashloop) = match statuses {
        Some(arr) if !arr.is_empty() => {
            let all_ready = arr
                .iter()
                .all(|c| c.get("ready").and_then(|r| r.as_bool()).unwrap_or(false));
            let any_crashloop = arr.iter().any(|c| {
                c.pointer("/state/waiting/reason")
                    .and_then(|r| r.as_str())
                    .map(|r| {
                        r.contains("CrashLoop") || r.contains("Error") || r.contains("ImagePull")
                    })
                    .unwrap_or(false)
            });
            (all_ready, any_crashloop)
        }
        _ => (false, false),
    };
    let status = match phase.as_deref() {
        _ if any_crashloop => HealthStatus::Unhealthy,
        Some("Running") if all_ready => HealthStatus::Ok,
        Some("Running") => HealthStatus::Starting, // running but not all containers ready
        Some("Succeeded") => HealthStatus::Ok,     // completed job pod
        Some("Pending") => HealthStatus::Starting,
        Some("Failed") => HealthStatus::Unhealthy,
        _ => HealthStatus::Unknown,
    };
    (phase, Some(status))
}

/// Local copy of the tilde expander (the config/test one is not public here).
fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = crate::paths::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real `kubectl get pods -n kube-system -o json` shape harvested from the
    // maker cluster (2026-07-05) — trimmed to the fields the parser reads.
    const REAL_PODS: &str = r#"{
      "items": [
        {"metadata": {"name": "coredns-c4dbffb5f-lpx6g"},
         "spec": {"containers": [{"image": "rancher/mirrored-coredns-coredns:1.14.2"}]},
         "status": {"phase": "Running", "containerStatuses": [{"ready": true, "restartCount": 0}]}},
        {"metadata": {"name": "metrics-server-xyz"},
         "spec": {"containers": [{"image": "rancher/mirrored-metrics-server:v0.8.0"}]},
         "status": {"phase": "Running"}}
      ]
    }"#;

    #[test]
    fn k6_parse_pods_maps_name_image_phase() {
        let svcs = parse_pods(REAL_PODS);
        assert_eq!(svcs.len(), 2);
        assert_eq!(svcs[0].name, "coredns-c4dbffb5f-lpx6g");
        assert_eq!(svcs[0].container_name, svcs[0].name); // pod name is the address
        assert_eq!(
            svcs[0].image.as_deref(),
            Some("rancher/mirrored-coredns-coredns:1.14.2")
        );
        assert_eq!(svcs[0].health.as_deref(), Some("Running"));
        // ready:true + Running -> Ok
        assert_eq!(svcs[0].health_status, Some(HealthStatus::Ok));
    }

    #[test]
    fn k7_pod_health_rollup() {
        // CrashLoopBackOff even while phase=Running -> Unhealthy.
        let crash = r#"{"items":[{"metadata":{"name":"p"},"spec":{"containers":[{"image":"x"}]},
            "status":{"phase":"Running","containerStatuses":[
              {"ready":false,"state":{"waiting":{"reason":"CrashLoopBackOff"}}}]}}]}"#;
        assert_eq!(
            parse_pods(crash)[0].health_status,
            Some(HealthStatus::Unhealthy)
        );
        // Pending -> Starting.
        let pending = r#"{"items":[{"metadata":{"name":"p"},"spec":{"containers":[{"image":"x"}]},
            "status":{"phase":"Pending"}}]}"#;
        assert_eq!(
            parse_pods(pending)[0].health_status,
            Some(HealthStatus::Starting)
        );
        // Running but a container not ready (no crashloop) -> Starting.
        let notready = r#"{"items":[{"metadata":{"name":"p"},"spec":{"containers":[{"image":"x"}]},
            "status":{"phase":"Running","containerStatuses":[{"ready":false}]}}]}"#;
        assert_eq!(
            parse_pods(notready)[0].health_status,
            Some(HealthStatus::Starting)
        );
    }

    #[test]
    fn k6_parse_pods_total_on_garbage() {
        assert!(parse_pods("not json").is_empty());
        assert!(parse_pods("{}").is_empty());
        assert!(parse_pods(r#"{"items": []}"#).is_empty());
    }
}
