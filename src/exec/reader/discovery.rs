//! Discovery reader: synthesizes records from the locally-cached
//! `Profile` (no remote round-trip). Each service in the profile
//! becomes one record carrying the discovered metadata.
//!
//! This is the "hey, what do you know about this server" probe.

use anyhow::Result;
use serde_json::json;

use super::{ReadOpts, ReadStep, Reader};
use crate::exec::record::Record;
use crate::profile::cache::load_profile;
use crate::verbs::runtime::RemoteRunner;

pub struct DiscoveryReader;

impl Reader for DiscoveryReader {
    fn kind(&self) -> &'static str {
        "discovery"
    }
    fn read(
        &self,
        _runner: &dyn RemoteRunner,
        step: &ReadStep<'_>,
        _opts: &ReadOpts,
    ) -> Result<Vec<Record>> {
        let profile = load_profile(step.namespace)?.ok_or_else(|| {
            anyhow::anyhow!(
                "no profile cached for `{}` (run `inspect setup {}`)",
                step.namespace,
                step.namespace
            )
        })?;
        let target = step.service;
        let mut recs = Vec::new();
        for svc in &profile.services {
            if let Some(t) = target {
                if svc.name != t {
                    continue;
                }
            }
            let mut r = Record::new();
            r.labels.insert("server".into(), step.namespace.into());
            r.labels.insert("service".into(), svc.name.clone());
            r.labels.insert("source".into(), "discovery".into());
            r.fields.insert(
                "kind".into(),
                json!(format!("{:?}", svc.kind).to_lowercase()),
            );
            if let Some(img) = &svc.image {
                r.fields.insert("image".into(), json!(img));
            }
            r.fields.insert(
                "ports".into(),
                json!(svc
                    .ports
                    .iter()
                    .map(|p| format!("{}/{}", p.host, p.proto.as_str()))
                    .collect::<Vec<_>>()),
            );
            r.line = Some(format!(
                "{}\t{:?}\t{}",
                svc.name,
                svc.kind,
                svc.image.as_deref().unwrap_or("-")
            ));
            recs.push(r);
        }
        Ok(recs)
    }
}
