//! Host reader: read a file at a host path (no container exec).

use anyhow::Result;

use super::{lines_to_records, push_line_filters_grep, ReadOpts, ReadStep, Reader};
use crate::exec::record::Record;
use crate::ssh::exec::RunOpts;
use crate::verbs::quote::shquote;
use crate::verbs::runtime::RemoteRunner;

pub struct HostReader {
    pub path: String,
}

impl Reader for HostReader {
    fn kind(&self) -> &'static str {
        "host"
    }
    fn read(
        &self,
        runner: &dyn RemoteRunner,
        step: &ReadStep<'_>,
        opts: &ReadOpts,
    ) -> Result<Vec<Record>> {
        let path = shquote(&self.path);
        let mut cmd = match opts.tail {
            Some(n) => format!("tail -n {n} {path}"),
            None => format!("cat {path}"),
        };
        push_line_filters_grep(&mut cmd, &opts.line_filters);
        let out = runner.run(step.namespace, step.target, &cmd, RunOpts::with_timeout(30))?;
        if !out.ok() {
            return Ok(Vec::new());
        }
        let src = format!("host:{}", self.path);
        let mut recs = lines_to_records(&out.stdout);
        for r in &mut recs {
            r.labels.insert("server".into(), step.namespace.into());
            r.labels.insert("service".into(), "_".into());
            r.labels.insert("source".into(), src.clone());
            r.labels.insert("path".into(), self.path.clone());
        }
        Ok(recs)
    }
}
