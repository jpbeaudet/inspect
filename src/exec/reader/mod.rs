//! Reader trait + dispatch.
//!
//! A reader takes a single resolved (namespace, target) plus the
//! semantic medium pre-extracted from the selector and returns a
//! Vec<Record> for that step. Readers shell out via [`RemoteRunner`]
//! so they're free to be unit-tested with `INSPECT_MOCK_REMOTE_FILE`.

use anyhow::Result;

use crate::exec::medium::Medium;
use crate::exec::record::Record;
use crate::ssh::options::SshTarget;
use crate::verbs::runtime::RemoteRunner;

pub mod logs;
pub mod file;
pub mod dir;
pub mod discovery;
pub mod state;
pub mod volume;
pub mod image;
pub mod network;
pub mod host;

/// Hints the planner can pass down to push filters/time/tail to the
/// remote command when the medium can honor them.
#[derive(Debug, Clone, Default)]
pub struct ReadOpts {
    pub since: Option<String>,
    pub until: Option<String>,
    pub tail: Option<usize>,
    /// Optional pre-pushdown line filters. Empty means "no pushdown".
    pub line_filters: Vec<LineFilter>,
}

#[derive(Debug, Clone)]
pub struct LineFilter {
    pub negated: bool,
    pub regex: bool,
    pub pattern: String,
}

/// Concrete (namespace, target) step the reader will operate on.
pub struct ReadStep<'a> {
    pub namespace: &'a str,
    pub target: &'a SshTarget,
    /// Service name (`None` for host-level `_`).
    pub service: Option<&'a str>,
    /// Optional service definition from the cached profile (for kind etc).
    pub service_def: Option<&'a crate::profile::schema::Service>,
}

/// Trait every medium reader implements.
pub trait Reader {
    /// Best-effort kind tag (matches [`Medium::kind`]).
    fn kind(&self) -> &'static str;

    /// Read records for one step.
    fn read(
        &self,
        runner: &dyn RemoteRunner,
        step: &ReadStep<'_>,
        opts: &ReadOpts,
    ) -> Result<Vec<Record>>;
}

/// Dispatch: pick a reader implementation for a parsed [`Medium`].
pub fn for_medium(m: &Medium) -> Box<dyn Reader> {
    match m {
        Medium::Logs => Box::new(logs::LogsReader),
        Medium::File(path) => Box::new(file::FileReader { path: path.clone() }),
        Medium::Dir(path) => Box::new(dir::DirReader { path: path.clone() }),
        Medium::Discovery => Box::new(discovery::DiscoveryReader),
        Medium::State => Box::new(state::StateReader),
        Medium::Volume(name) => Box::new(volume::VolumeReader { filter: name.clone() }),
        Medium::Image => Box::new(image::ImageReader),
        Medium::Network => Box::new(network::NetworkReader),
        Medium::Host(path) => Box::new(host::HostReader { path: path.clone() }),
    }
}

/// Helper used by readers that produce one record per stdout line.
pub fn lines_to_records(stdout: &str) -> Vec<Record> {
    stdout
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| Record::new().with_line(l.to_string()))
        .collect()
}
