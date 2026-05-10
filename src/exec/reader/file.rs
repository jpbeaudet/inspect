//! File reader: emits one Record per line of a file (read via `cat`).

use anyhow::Result;

use super::{lines_to_records, push_line_filters_grep, ReadOpts, ReadStep, Reader};
use crate::exec::record::Record;
use crate::profile::schema::ServiceKind;
use crate::ssh::exec::RunOpts;
use crate::verbs::quote::shquote;
use crate::verbs::runtime::RemoteRunner;

pub struct FileReader {
    pub path: String,
}

impl Reader for FileReader {
    fn read(
        &self,
        runner: &dyn RemoteRunner,
        step: &ReadStep<'_>,
        opts: &ReadOpts,
    ) -> Result<Vec<Record>> {
        let mut cmd = build_cat(step, &self.path);
        push_line_filters_grep(&mut cmd, &opts.line_filters);
        let out = runner.run(step.namespace, step.target, &cmd, RunOpts::with_timeout(30))?;
        if !out.ok() {
            return Ok(Vec::new());
        }
        let src = format!("file:{}", self.path);
        let mut recs = lines_to_records(&out.stdout);
        for r in &mut recs {
            r.labels.insert("server".into(), step.namespace.to_string());
            r.labels
                .insert("service".into(), step.service.unwrap_or("_").to_string());
            r.labels.insert("source".into(), src.clone());
            r.labels.insert("path".into(), self.path.clone());
        }
        Ok(recs)
    }
}

fn build_cat(step: &ReadStep<'_>, path: &str) -> String {
    let qpath = shquote(path);
    match step.service {
        Some(svc)
            if step
                .service_def
                .map(|s| s.kind)
                .unwrap_or(ServiceKind::Container)
                == ServiceKind::Container =>
        {
            // Use real container name for docker exec, not the user-facing token.
            let container = step
                .service_def
                .map(|s| s.container_name.as_str())
                .unwrap_or(svc);
            format!("docker exec {} cat {qpath}", shquote(container))
        }
        _ => format!("cat {qpath}"),
    }
}
