//! Image reader: `docker images`.

use anyhow::Result;
use serde_json::json;

use super::{ReadOpts, ReadStep, Reader};
use crate::exec::record::Record;
use crate::ssh::exec::RunOpts;
use crate::verbs::runtime::RemoteRunner;

pub struct ImageReader;

impl Reader for ImageReader {
    fn read(
        &self,
        runner: &dyn RemoteRunner,
        step: &ReadStep<'_>,
        _opts: &ReadOpts,
    ) -> Result<Vec<Record>> {
        let cmd = "docker images --format '{{.Repository}}|{{.Tag}}|{{.ID}}|{{.Size}}|{{.CreatedSince}}'";
        let out = runner.run(step.namespace, step.target, cmd, RunOpts::with_timeout(15))?;
        if !out.ok() {
            return Ok(Vec::new());
        }
        let svc = step.service.unwrap_or("_");
        let mut recs = Vec::new();
        for line in out.stdout.lines() {
            let parts: Vec<&str> = line.splitn(5, '|').collect();
            if parts.len() < 5 {
                continue;
            }
            let mut r = Record::new()
                .with_label("server", step.namespace)
                .with_label("service", svc)
                .with_label("source", "image")
                .with_line(line.to_string());
            r.fields.insert("repository".into(), json!(parts[0]));
            r.fields.insert("tag".into(), json!(parts[1]));
            r.fields.insert("id".into(), json!(parts[2]));
            r.fields.insert("size".into(), json!(parts[3]));
            r.fields.insert("created_since".into(), json!(parts[4]));
            recs.push(r);
        }
        Ok(recs)
    }
}
