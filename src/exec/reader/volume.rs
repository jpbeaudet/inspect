//! Volume reader: `docker volume ls` (optionally filtered).

use anyhow::Result;
use serde_json::json;

use super::{ReadOpts, ReadStep, Reader};
use crate::exec::record::Record;
use crate::ssh::exec::RunOpts;
use crate::verbs::quote::shquote;
use crate::verbs::runtime::RemoteRunner;

pub struct VolumeReader {
    pub filter: Option<String>,
}

impl Reader for VolumeReader {
    fn read(
        &self,
        runner: &dyn RemoteRunner,
        step: &ReadStep<'_>,
        _opts: &ReadOpts,
    ) -> Result<Vec<Record>> {
        let cmd = match &self.filter {
            Some(name) => format!(
                "docker volume ls --filter name={} --format '{{{{.Name}}}}|{{{{.Driver}}}}|{{{{.Mountpoint}}}}'",
                shquote(name)
            ),
            None => "docker volume ls --format '{{.Name}}|{{.Driver}}|{{.Mountpoint}}'".into(),
        };
        let out = runner.run(step.namespace, step.target, &cmd, RunOpts::with_timeout(15))?;
        if !out.ok() {
            return Ok(Vec::new());
        }
        let svc = step.service.unwrap_or("_");
        let mut recs = Vec::new();
        for line in out.stdout.lines() {
            let parts: Vec<&str> = line.splitn(3, '|').collect();
            if parts.len() < 3 {
                continue;
            }
            let label_src = format!("volume:{}", parts[0]);
            let mut r = Record::new()
                .with_label("server", step.namespace)
                .with_label("service", svc)
                .with_label("source", &label_src)
                .with_line(line.to_string());
            r.fields.insert("name".into(), json!(parts[0]));
            r.fields.insert("driver".into(), json!(parts[1]));
            r.fields.insert("mountpoint".into(), json!(parts[2]));
            recs.push(r);
        }
        Ok(recs)
    }
}
