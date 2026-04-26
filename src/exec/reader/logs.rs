//! Logs reader: `docker logs` for container services, `journalctl -u`
//! for systemd, `tail /var/log/syslog` for host-level.

use anyhow::Result;

use super::{push_line_filters_grep, ReadOpts, ReadStep, Reader, lines_to_records};
use crate::exec::record::Record;
use crate::profile::schema::ServiceKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::quote::shquote;
use crate::verbs::runtime::RemoteRunner;

pub struct LogsReader;

impl Reader for LogsReader {
    fn kind(&self) -> &'static str {
        "logs"
    }
    fn read(
        &self,
        runner: &dyn RemoteRunner,
        step: &ReadStep<'_>,
        opts: &ReadOpts,
    ) -> Result<Vec<Record>> {
        let cmd = build_logs_cmd(step, opts);
        let out = runner.run(step.namespace, step.target, &cmd, RunOpts::with_timeout(60))?;
        if !out.ok() && out.stdout.is_empty() {
            return Ok(Vec::new());
        }
        let mut recs = lines_to_records(&out.stdout);
        for r in &mut recs {
            r.labels.insert("server".into(), step.namespace.to_string());
            r.labels
                .insert("service".into(), step.service.unwrap_or("_").to_string());
            r.labels.insert("source".into(), "logs".into());
        }
        Ok(recs)
    }
}

fn build_logs_cmd(step: &ReadStep<'_>, opts: &ReadOpts) -> String {
    let kind = step.service_def.map(|s| s.kind).unwrap_or(ServiceKind::Container);
    let name = step.service;
    let mut cmd = match (name, kind) {
        (Some(name), ServiceKind::Systemd) => {
            let mut s = format!("journalctl -u {}", shquote(name));
            if let Some(since) = &opts.since {
                s.push_str(" --since ");
                s.push_str(&shquote(since));
            }
            if let Some(until) = &opts.until {
                s.push_str(" --until ");
                s.push_str(&shquote(until));
            }
            if let Some(n) = opts.tail {
                s.push_str(&format!(" -n {n}"));
            }
            s
        }
        (Some(name), _) => {
            let mut s = String::from("docker logs");
            if let Some(since) = &opts.since {
                s.push_str(" --since ");
                s.push_str(&shquote(since));
            }
            if let Some(until) = &opts.until {
                s.push_str(" --until ");
                s.push_str(&shquote(until));
            }
            if let Some(n) = opts.tail {
                s.push_str(&format!(" --tail {n}"));
            }
            s.push(' ');
            s.push_str(&shquote(name));
            // docker logs writes to stderr by default; merge.
            s.push_str(" 2>&1");
            s
        }
        (None, _) => {
            let n = opts.tail.unwrap_or(500);
            format!(
                "tail -n {n} /var/log/syslog 2>/dev/null || tail -n {n} /var/log/messages"
            )
        }
    };
    push_line_filters_grep(&mut cmd, &opts.line_filters);
    cmd
}
