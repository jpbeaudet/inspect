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

    /// F6 (v0.1.3): compose projects discovered on this host via
    /// `docker compose ls --format json`. Empty when the host runs
    /// no compose projects (or docker compose is not installed).
    /// Read by `inspect compose ls`, `inspect compose ps`, and the
    /// new `compose_projects:` line in `inspect status`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compose_projects: Vec<ComposeProject>,

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
            compose_projects: Vec::new(),
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
    /// P13: when a per-container `docker inspect` timed out at
    /// discovery time we surface a partial entry (name + container_id
    /// only) and flag it here so `inspect setup --retry-failed` and
    /// downstream verbs can detect incomplete data.
    #[serde(default, skip_serializing_if = "is_false")]
    pub discovery_incomplete: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
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

/// F6 (v0.1.3): a single compose project as reported by
/// `docker compose ls --format json`. Discovered at `inspect setup`
/// time and cached on the [`Profile`]; consulted by `inspect compose
/// ls`, `inspect compose ps`, and the `compose_projects:` line in
/// `inspect status`.
///
/// `working_dir` is the directory containing `compose_file` and is
/// the path every subsequent `docker compose` command must `cd` into,
/// because compose resolves relative `volumes`, `env_file`, etc.
/// against it. Without this we'd reproduce the exact "operator drops
/// back to `inspect run -- 'cd /opt/luminary-onyx && sudo docker
/// compose …'`" pattern F6 was filed to eliminate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComposeProject {
    /// Project name (the value of `-p` / `COMPOSE_PROJECT_NAME`).
    pub name: String,
    /// Raw status string from `docker compose ls`, e.g. `"running(3)"`
    /// or `"running(2), exited(1)"`. Preserved verbatim so operators
    /// can see the original docker phrasing; counts below are derived.
    pub status: String,
    /// Absolute path of the compose file (the leftmost entry from
    /// `docker compose ls`'s `ConfigFiles` field).
    pub compose_file: String,
    /// Directory containing `compose_file` — every per-project verb
    /// (`ps`, `config`, `logs`, `restart`) `cd`s here before invoking
    /// docker so relative paths in the compose file resolve correctly.
    pub working_dir: String,
    /// Total services known to this project at discovery time
    /// (sum of all per-state counts in `status`). Surfaced as
    /// `service_count` in JSON envelopes.
    #[serde(default)]
    pub service_count: u32,
    /// Services in the `running` state at discovery time. Surfaced
    /// as `running_count` in JSON envelopes.
    #[serde(default)]
    pub running_count: u32,
}

impl ComposeProject {
    /// Parse `docker compose ls --format json` output into a typed
    /// list. Tolerates the field-name variation between docker
    /// versions (older daemons sometimes lowercase keys, newer ones
    /// sometimes add fields we don't care about). Per-row failures
    /// are skipped silently — discovery should be best-effort, not
    /// brittle.
    pub fn parse_ls_json(raw: &str) -> Vec<Self> {
        let value: serde_json::Value = match serde_json::from_str(raw) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        let arr = match value.as_array() {
            Some(a) => a,
            None => return Vec::new(),
        };
        let mut out = Vec::with_capacity(arr.len());
        for entry in arr {
            let name = entry
                .get("Name")
                .or_else(|| entry.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if name.is_empty() {
                continue;
            }
            let status = entry
                .get("Status")
                .or_else(|| entry.get("status"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let config_files = entry
                .get("ConfigFiles")
                .or_else(|| entry.get("configFiles"))
                .or_else(|| entry.get("config_files"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            // ConfigFiles is comma-separated when there are multiple
            // overlay files (e.g. `docker-compose.yml,override.yml`);
            // we treat the leftmost as canonical because that's how
            // compose resolves the project's working directory.
            let compose_file = config_files
                .split(',')
                .next()
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            let working_dir = std::path::Path::new(&compose_file)
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            let (running_count, service_count) = parse_status_counts(&status);
            out.push(ComposeProject {
                name: name.to_string(),
                status,
                compose_file,
                working_dir,
                service_count,
                running_count,
            });
        }
        out
    }
}

/// Parse `docker compose ls` Status strings like `running(3)` or
/// `running(2), exited(1)` into `(running_count, total_count)`.
/// Unrecognized shapes return `(0, 0)` so the caller can still
/// surface the raw string without inventing numbers.
fn parse_status_counts(status: &str) -> (u32, u32) {
    let mut running = 0u32;
    let mut total = 0u32;
    for part in status.split(',') {
        let part = part.trim();
        // Find `state(N)` shape.
        let open = match part.find('(') {
            Some(i) => i,
            None => continue,
        };
        let close = match part.rfind(')') {
            Some(i) if i > open => i,
            _ => continue,
        };
        let state = part[..open].trim().to_ascii_lowercase();
        let n: u32 = match part[open + 1..close].trim().parse() {
            Ok(n) => n,
            Err(_) => continue,
        };
        total = total.saturating_add(n);
        if state == "running" {
            running = running.saturating_add(n);
        }
    }
    (running, total)
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
            discovery_incomplete: false,
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
    fn parse_status_counts_basic() {
        // running-only: count both as running and total.
        assert_eq!(parse_status_counts("running(3)"), (3, 3));
        // mixed states: total sums all, running stays distinct.
        assert_eq!(parse_status_counts("running(2), exited(1)"), (2, 3));
        // ordering doesn't matter.
        assert_eq!(parse_status_counts("exited(1), running(2)"), (2, 3));
        // case-insensitive on the state name (older daemons capitalize).
        assert_eq!(parse_status_counts("Running(4)"), (4, 4));
    }

    #[test]
    fn parse_status_counts_handles_unknown_states_and_garbage() {
        // Unknown states (paused, dead) still contribute to total.
        assert_eq!(parse_status_counts("paused(2)"), (0, 2));
        // Garbage / empty returns zeros without panicking.
        assert_eq!(parse_status_counts(""), (0, 0));
        assert_eq!(parse_status_counts("running"), (0, 0));
        assert_eq!(parse_status_counts("running()"), (0, 0));
    }

    #[test]
    fn parse_compose_ls_real_output() {
        // Modern docker compose v2 output: capitalized keys, ConfigFiles is comma-separated.
        let raw = r#"[
            {"Name":"luminary-onyx","Status":"running(3)","ConfigFiles":"/opt/luminary-onyx/docker-compose.yml"},
            {"Name":"atlas","Status":"running(2), exited(1)","ConfigFiles":"/opt/atlas/docker-compose.yml,/opt/atlas/override.yml"}
        ]"#;
        let projects = ComposeProject::parse_ls_json(raw);
        assert_eq!(projects.len(), 2);

        assert_eq!(projects[0].name, "luminary-onyx");
        assert_eq!(projects[0].status, "running(3)");
        assert_eq!(
            projects[0].compose_file,
            "/opt/luminary-onyx/docker-compose.yml"
        );
        assert_eq!(projects[0].working_dir, "/opt/luminary-onyx");
        assert_eq!(projects[0].running_count, 3);
        assert_eq!(projects[0].service_count, 3);

        // Multi-file overlay: leftmost wins; mixed-state status parses.
        assert_eq!(projects[1].name, "atlas");
        assert_eq!(projects[1].compose_file, "/opt/atlas/docker-compose.yml");
        assert_eq!(projects[1].working_dir, "/opt/atlas");
        assert_eq!(projects[1].running_count, 2);
        assert_eq!(projects[1].service_count, 3);
    }

    #[test]
    fn parse_compose_ls_tolerates_lowercase_keys_and_missing_fields() {
        // Some older / non-CE daemons emit lowercase keys.
        let raw = r#"[{"name":"foo","status":"exited(1)","configFiles":"/srv/foo/compose.yml"}]"#;
        let projects = ComposeProject::parse_ls_json(raw);
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "foo");
        assert_eq!(projects[0].running_count, 0);
        assert_eq!(projects[0].service_count, 1);
    }

    #[test]
    fn parse_compose_ls_skips_nameless_entries_and_returns_empty_on_garbage() {
        // Empty array, not-an-array, and entries with no name are all
        // tolerated without panicking.
        assert!(ComposeProject::parse_ls_json("[]").is_empty());
        assert!(ComposeProject::parse_ls_json("not json at all").is_empty());
        assert!(ComposeProject::parse_ls_json(r#"{"Name":"x"}"#).is_empty());
        let raw = r#"[{"Status":"running(1)"},{"Name":"keepme","Status":"running(1)","ConfigFiles":"/x/c.yml"}]"#;
        let projects = ComposeProject::parse_ls_json(raw);
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "keepme");
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
            discovery_incomplete: false,
        });
        assert_ne!(a.fingerprint(), b.fingerprint());
    }
}
