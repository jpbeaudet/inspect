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
pub trait Reader: Send + Sync {
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

/// `Arc`-shareable variant of [`for_medium`] used when the engine
/// fans out reads across worker threads.
pub fn for_medium_arc(m: &Medium) -> std::sync::Arc<dyn Reader + Send + Sync> {
    match m {
        Medium::Logs => std::sync::Arc::new(logs::LogsReader),
        Medium::File(path) => std::sync::Arc::new(file::FileReader { path: path.clone() }),
        Medium::Dir(path) => std::sync::Arc::new(dir::DirReader { path: path.clone() }),
        Medium::Discovery => std::sync::Arc::new(discovery::DiscoveryReader),
        Medium::State => std::sync::Arc::new(state::StateReader),
        Medium::Volume(name) => std::sync::Arc::new(volume::VolumeReader { filter: name.clone() }),
        Medium::Image => std::sync::Arc::new(image::ImageReader),
        Medium::Network => std::sync::Arc::new(network::NetworkReader),
        Medium::Host(path) => std::sync::Arc::new(host::HostReader { path: path.clone() }),
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

/// Append a `| grep ...` chain to `cmd` for each line filter, so the
/// remote command pre-filters bytes before we ever read them. Bible
/// §9.10 — filter pushdown.
///
/// Each clause uses `|| true` to swallow grep's exit-1 on no-match (we
/// rely on the upstream command's exit code, not grep's).
pub fn push_line_filters_grep(cmd: &mut String, filters: &[LineFilter]) {
    use crate::verbs::quote::shquote;
    for f in filters {
        cmd.push_str(" | grep ");
        if f.negated {
            cmd.push_str("-v ");
        }
        if f.regex {
            cmd.push_str("-E ");
        } else {
            cmd.push_str("-F ");
        }
        cmd.push_str(&shquote(&f.pattern));
        cmd.push_str(" || true");
    }
}
