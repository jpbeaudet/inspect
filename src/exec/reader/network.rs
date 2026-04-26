//! Network reader: listening sockets via `ss -tunlp` with a `netstat` fallback.

use anyhow::Result;
use serde_json::json;

use super::{ReadOpts, ReadStep, Reader};
use crate::exec::record::Record;
use crate::ssh::exec::RunOpts;
use crate::verbs::runtime::RemoteRunner;

pub struct NetworkReader;

impl Reader for NetworkReader {
    fn kind(&self) -> &'static str {
        "network"
    }
    fn read(
        &self,
        runner: &dyn RemoteRunner,
        step: &ReadStep<'_>,
        _opts: &ReadOpts,
    ) -> Result<Vec<Record>> {
        let cmd = "ss -H -tunlp 2>/dev/null || netstat -tunlp 2>/dev/null";
        let out = runner.run(step.namespace, step.target, cmd, RunOpts::with_timeout(15))?;
        if !out.ok() {
            return Ok(Vec::new());
        }
        let svc = step.service.unwrap_or("_");
        let mut recs = Vec::new();
        for line in out.stdout.lines() {
            if line.trim().is_empty() {
                continue;
            }
            // Either ss columns: NetID State Recv-Q Send-Q Local Peer Process
            // or netstat columns. We don't try to parse perfectly; expose raw
            // line + a best-effort `local` field so users can `| pattern` it.
            let cols: Vec<&str> = line.split_whitespace().collect();
            let local = cols.get(4).copied().unwrap_or("");
            let mut r = Record::new()
                .with_label("server", step.namespace)
                .with_label("service", svc)
                .with_label("source", "network")
                .with_line(line.to_string());
            r.fields.insert("local".into(), json!(local));
            r.fields.insert("raw".into(), json!(line));
            recs.push(r);
        }
        Ok(recs)
    }
}
