//! Directory reader: emits one Record per file in `<dir>` (one level
//! deep). Each record carries `path` and `size_bytes`. Use `| map` to
//! recurse into individual files.

use anyhow::Result;

use super::{ReadOpts, ReadStep, Reader};
use crate::exec::record::Record;
use crate::ssh::exec::RunOpts;
use crate::verbs::quote::shquote;
use crate::verbs::runtime::RemoteRunner;

pub struct DirReader {
    pub path: String,
}

impl Reader for DirReader {
    fn kind(&self) -> &'static str {
        "dir"
    }
    fn read(
        &self,
        runner: &dyn RemoteRunner,
        step: &ReadStep<'_>,
        _opts: &ReadOpts,
    ) -> Result<Vec<Record>> {
        // -1 one-per-line, -A include dotfiles, -p append `/` to dirs.
        let cmd = format!(
            "ls -1Ap --color=never {} 2>/dev/null",
            shquote(&self.path)
        );
        let out = runner.run(step.namespace, step.target, &cmd, RunOpts::with_timeout(30))?;
        if !out.ok() {
            return Ok(Vec::new());
        }
        let src = format!("dir:{}", self.path);
        let mut recs = Vec::new();
        for line in out.stdout.lines() {
            let entry = line.trim();
            if entry.is_empty() {
                continue;
            }
            let is_dir = entry.ends_with('/');
            let name = entry.trim_end_matches('/');
            let mut r = Record::new();
            r.labels.insert("server".into(), step.namespace.to_string());
            r.labels
                .insert("service".into(), step.service.unwrap_or("_").to_string());
            r.labels.insert("source".into(), src.clone());
            r.labels
                .insert("path".into(), format!("{}/{}", self.path.trim_end_matches('/'), name));
            r.fields
                .insert("name".into(), serde_json::Value::String(name.into()));
            r.fields.insert(
                "is_dir".into(),
                serde_json::Value::Bool(is_dir),
            );
            r.line = Some(name.into());
            recs.push(r);
        }
        Ok(recs)
    }
}
