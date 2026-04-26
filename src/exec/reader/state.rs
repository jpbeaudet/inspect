//! State reader: live container/process state. Uses `docker ps` (or
//! `systemctl is-active` when the service is systemd-kind).

use anyhow::Result;
use serde_json::json;

use super::{ReadOpts, ReadStep, Reader};
use crate::exec::record::Record;
use crate::profile::schema::ServiceKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::quote::shquote;
use crate::verbs::runtime::RemoteRunner;

pub struct StateReader;

/// Parses one stdout line into an optional `Record`.
/// Args: `(line, namespace, service_name)`.
type StateLineParser = fn(&str, &str, &str) -> Option<Record>;

impl Reader for StateReader {
    fn read(
        &self,
        runner: &dyn RemoteRunner,
        step: &ReadStep<'_>,
        _opts: &ReadOpts,
    ) -> Result<Vec<Record>> {
        let kind = step
            .service_def
            .map(|s| s.kind)
            .unwrap_or(ServiceKind::Container);
        let name = step.service.unwrap_or("_");

        let (cmd, parser): (String, StateLineParser) = match (name, kind) {
            ("_", _) => (
                // Host-level state: load average + uptime.
                "uptime".into(),
                |line, ns, svc| {
                    let mut r = Record::new()
                        .with_label("server", ns)
                        .with_label("service", svc)
                        .with_label("source", "state")
                        .with_line(line.to_string());
                    r.fields.insert("uptime".into(), json!(line.trim()));
                    Some(r)
                },
            ),
            (_, ServiceKind::Systemd) => (
                format!(
                    "systemctl is-active {name} 2>&1; systemctl is-enabled {name} 2>&1",
                    name = shquote(name)
                ),
                |stdout, ns, svc| {
                    let mut it = stdout.lines();
                    let active = it.next().unwrap_or("unknown").trim();
                    let enabled = it.next().unwrap_or("unknown").trim();
                    let mut r = Record::new()
                        .with_label("server", ns)
                        .with_label("service", svc)
                        .with_label("source", "state")
                        .with_line(format!("active={active} enabled={enabled}"));
                    r.fields.insert("active".into(), json!(active));
                    r.fields.insert("enabled".into(), json!(enabled));
                    Some(r)
                },
            ),
            (_, _) => (
                format!(
                    "docker ps --no-trunc --filter name={name} --format '{{{{.ID}}}}|{{{{.Image}}}}|{{{{.Status}}}}|{{{{.RunningFor}}}}'",
                    name = shquote(name)
                ),
                |line, ns, svc| {
                    let parts: Vec<&str> = line.splitn(4, '|').collect();
                    if parts.len() < 4 {
                        return None;
                    }
                    let mut r = Record::new()
                        .with_label("server", ns)
                        .with_label("service", svc)
                        .with_label("source", "state")
                        .with_line(line.to_string());
                    r.fields.insert("container_id".into(), json!(parts[0]));
                    r.fields.insert("image".into(), json!(parts[1]));
                    r.fields.insert("status".into(), json!(parts[2]));
                    r.fields.insert("running_for".into(), json!(parts[3]));
                    Some(r)
                },
            ),
        };

        let out = runner.run(step.namespace, step.target, &cmd, RunOpts::with_timeout(15))?;
        if out.stdout.trim().is_empty() {
            return Ok(Vec::new());
        }
        // For docker ps we get one line per container; for systemd we collapse into one.
        let mut recs = Vec::new();
        if matches!(kind, ServiceKind::Systemd) || name == "_" {
            if let Some(r) = parser(out.stdout.as_str(), step.namespace, name) {
                recs.push(r);
            }
        } else {
            for line in out.stdout.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                if let Some(r) = parser(line, step.namespace, name) {
                    recs.push(r);
                }
            }
        }
        Ok(recs)
    }
}
