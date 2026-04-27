//! Profile YAML schema. Mirrors §5.2 of the bible.
//!
//! Fields the user is expected to edit by hand are preserved across
//! re-discovery (see `cache::merge_local_edits`). Discovery owns the
//! "physical" sections (`services`, `volumes`, `images`, `networks`,
//! `remote_tooling`); the user owns `groups`, `aliases`, and
//! `local_overrides`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const PROFILE_SCHEMA_VERSION: u32 = 1;

/// Top-level profile document persisted at
/// `~/.inspect/profiles/<ns>.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Profile {
    pub schema_version: u32,
    pub namespace: String,
    pub host: String,
    /// RFC3339 UTC timestamp. Stored as a string so YAML round-trips are
    /// byte-stable across timezones.
    pub discovered_at: String,

    #[serde(default)]
    pub remote_tooling: RemoteTooling,

    #[serde(default)]
    pub services: Vec<Service>,
    #[serde(default)]
    pub volumes: Vec<Volume>,
    #[serde(default)]
    pub images: Vec<Image>,
    #[serde(default)]
    pub networks: Vec<Network>,

    /// Non-fatal warnings emitted during discovery (missing tools, denied
    /// permissions, partial inventories). Surfaced in the `setup` summary.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,

    /// Field pitfall §5.3: signed offset between *remote* `date +%s`
    /// and the local clock at discovery time, in seconds. Positive
    /// means the remote is ahead of us; negative means it lags.
    /// Captured once per discovery so operators can see whether
    /// `--since`/`--until` semantics will surprise them, and so we
    /// can warn (or eventually adjust) when skew exceeds a threshold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock_offset_secs: Option<i64>,

    // ---- user-owned sections (preserved across re-discovery) ---------------
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub groups: BTreeMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub aliases: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_overrides: Option<serde_yaml::Value>,
}

impl Profile {
    pub fn empty(namespace: &str, host: &str, discovered_at: &str) -> Self {
        Self {
            schema_version: PROFILE_SCHEMA_VERSION,
            namespace: namespace.to_string(),
            host: host.to_string(),
            discovered_at: discovered_at.to_string(),
            remote_tooling: RemoteTooling::default(),
            services: Vec::new(),
            volumes: Vec::new(),
            images: Vec::new(),
            networks: Vec::new(),
            warnings: Vec::new(),
            clock_offset_secs: None,
            groups: BTreeMap::new(),
            aliases: BTreeMap::new(),
            local_overrides: None,
        }
    }

    /// Stable, content-only fingerprint used by drift detection. Excludes
    /// `discovered_at` and `warnings` so spurious changes don't trigger a
    /// drift signal.
    #[cfg(test)]
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(self.namespace.as_bytes());
        h.update(b"\0");
        h.update(self.host.as_bytes());
        h.update(b"\0");
        for s in &self.services {
            h.update(s.name.as_bytes());
            h.update(b"|");
            h.update(s.image.as_deref().unwrap_or("").as_bytes());
            h.update(b"|");
            h.update(s.container_id.as_deref().unwrap_or("").as_bytes());
            h.update(b"|");
            for p in &s.ports {
                h.update(format!("{}:{}/{}", p.host, p.container, p.proto).as_bytes());
                h.update(b",");
            }
            h.update(b"\n");
        }
        for v in &self.volumes {
            h.update(v.name.as_bytes());
            h.update(b"\n");
        }
        for i in &self.images {
            h.update(i.repo_tag.as_bytes());
            h.update(b"\n");
        }
        for n in &self.networks {
            h.update(n.name.as_bytes());
            h.update(b"\n");
        }
        let t = &self.remote_tooling;
        h.update(
            format!(
            "rg={} jq={} journalctl={} sed={} grep={} netstat={} ss={} systemctl={} docker={}\n",
            t.rg, t.jq, t.journalctl, t.sed, t.grep, t.netstat, t.ss, t.systemctl, t.docker,
        )
            .as_bytes(),
        );
        let bytes = h.finalize();
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteTooling {
    #[serde(default)]
    pub rg: bool,
    #[serde(default)]
    pub jq: bool,
    #[serde(default)]
    pub journalctl: bool,
    #[serde(default)]
    pub sed: bool,
    #[serde(default)]
    pub grep: bool,
    #[serde(default)]
    pub netstat: bool,
    #[serde(default)]
    pub ss: bool,
    #[serde(default)]
    pub systemctl: bool,
    #[serde(default)]
    pub docker: bool,
    #[serde(default)]
    pub podman: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Service {
    /// User-facing name used in selectors (e.g. `arte/api`). When the
    /// container carries a `com.docker.compose.service` label that's
    /// unambiguous on this host, we promote that label to the name.
    /// Otherwise this is the raw container name.
    pub name: String,
    /// Real container name as reported by `docker ps --format {{.Names}}`.
    /// **Always** the value passed to `docker logs|exec|restart|stop|...`,
    /// never `name` — that's the v0.1.0 phantom-service bug.
    /// For non-container kinds (systemd, host listener) this mirrors `name`.
    pub container_name: String,
    /// Compose service label, when present. Informational only — used
    /// at discovery time to decide whether to promote it to `name`,
    /// then preserved for forensics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compose_service: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<Port>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_status: Option<HealthStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_driver: Option<LogDriver>,
    #[serde(default)]
    pub log_readable_directly: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<Mount>,
    /// Optional kind: `container` (default), `systemd`, etc.
    #[serde(default = "Service::default_kind")]
    pub kind: ServiceKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
}

impl Service {
    fn default_kind() -> ServiceKind {
        ServiceKind::Container
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServiceKind {
    Container,
    Systemd,
    HostListener,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Port {
    pub host: u16,
    pub container: u16,
    #[serde(default = "default_proto")]
    pub proto: String,
}

fn default_proto() -> String {
    "tcp".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Mount {
    pub source: String,
    pub target: String,
    #[serde(rename = "type", default = "default_mount_type")]
    pub mount_type: String,
}

fn default_mount_type() -> String {
    "bind".to_string()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Ok,
    Unhealthy,
    Starting,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LogDriver {
    JsonFile,
    Journald,
    Local,
    Syslog,
    /// Field pitfall §2.3: drivers that ship logs out-of-process and
    /// are NOT readable via `docker logs`. We track them as distinct
    /// variants so `inspect logs` can emit a clear, actionable error
    /// instead of returning empty output.
    Fluentd,
    Awslogs,
    Gelf,
    Splunk,
    None,
    Other,
}

impl LogDriver {
    /// Can `docker logs <svc>` read history for this driver?
    /// `false` for drivers that ship logs to a remote sink (fluentd,
    /// awslogs, splunk, gelf) and for the `none` driver.
    pub fn is_readable_via_docker_logs(&self) -> bool {
        matches!(
            self,
            LogDriver::JsonFile | LogDriver::Journald | LogDriver::Local | LogDriver::Syslog
        )
    }

    /// Stable, human-friendly identifier (kebab-case, matches the
    /// docker driver name where possible).
    pub fn as_str(&self) -> &'static str {
        match self {
            LogDriver::JsonFile => "json-file",
            LogDriver::Journald => "journald",
            LogDriver::Local => "local",
            LogDriver::Syslog => "syslog",
            LogDriver::Fluentd => "fluentd",
            LogDriver::Awslogs => "awslogs",
            LogDriver::Gelf => "gelf",
            LogDriver::Splunk => "splunk",
            LogDriver::None => "none",
            LogDriver::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Volume {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mountpoint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Image {
    /// Canonical `repo:tag` (e.g. `nginx:1.27`).
    pub repo_tag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Network {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_yaml() {
        let mut p = Profile::empty("arte", "arte.example", "2026-04-25T14:32:18Z");
        p.services.push(Service {
            name: "pulse".into(),
            container_name: "pulse".into(),
            compose_service: None,
            container_id: Some("8a3f".into()),
            image: Some("luminary/pulse:1.4.2".into()),
            ports: vec![Port {
                host: 8000,
                container: 8000,
                proto: "tcp".into(),
            }],
            health: Some("http://localhost:8000/health".into()),
            health_status: Some(HealthStatus::Ok),
            log_driver: Some(LogDriver::JsonFile),
            log_readable_directly: true,
            mounts: vec![Mount {
                source: "/opt/x".into(),
                target: "/etc/x".into(),
                mount_type: "bind".into(),
            }],
            kind: ServiceKind::Container,
            depends_on: vec![],
        });
        p.remote_tooling.rg = true;
        p.remote_tooling.docker = true;
        p.groups
            .insert("storage".into(), vec!["postgres".into(), "redis".into()]);

        let s = serde_yaml::to_string(&p).expect("serialize");
        let back: Profile = serde_yaml::from_str(&s).expect("deserialize");
        assert_eq!(p, back);
    }

    #[test]
    fn fingerprint_is_stable_across_warnings_and_timestamp() {
        let mut a = Profile::empty("arte", "h", "2026-04-25T00:00:00Z");
        let mut b = a.clone();
        b.discovered_at = "2026-04-26T00:00:00Z".into();
        b.warnings.push("noisy warning".into());
        assert_eq!(a.fingerprint(), b.fingerprint());
        a.services.push(Service {
            name: "x".into(),
            container_name: "x".into(),
            compose_service: None,
            container_id: None,
            image: None,
            ports: vec![],
            health: None,
            health_status: None,
            log_driver: None,
            log_readable_directly: false,
            mounts: vec![],
            kind: ServiceKind::Container,
            depends_on: vec![],
        });
        assert_ne!(a.fingerprint(), b.fingerprint());
    }
}
