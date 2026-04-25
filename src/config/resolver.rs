//! Merge env-var overrides over file-defined namespaces.

use std::collections::BTreeSet;

use super::env;
use super::file::{self, ServersFile};
use super::namespace::{NamespaceConfig, NamespaceSource, ResolvedNamespace};
use crate::error::ConfigError;

/// Resolve a single namespace by merging env over file. Returns
/// `Err(UnknownNamespace)` if the namespace is defined in neither source.
pub fn resolve(name: &str) -> Result<ResolvedNamespace, ConfigError> {
    let file_cfg = file::load()?.namespaces.get(name).cloned();
    let env_cfg = env::read_env(name);
    merge(name, file_cfg, env_cfg)
}

/// List every known namespace (env ∪ file). Returns an empty list if none.
pub fn list_all() -> Result<Vec<ResolvedNamespace>, ConfigError> {
    let servers = file::load()?;
    let mut names: BTreeSet<String> = servers.namespaces.keys().cloned().collect();
    names.extend(env::enumerate_env_namespaces());

    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let file_cfg = servers.namespaces.get(&name).cloned();
        let env_cfg = env::read_env(&name);
        out.push(merge(&name, file_cfg, env_cfg)?);
    }
    Ok(out)
}

pub(crate) fn merge(
    name: &str,
    file_cfg: Option<NamespaceConfig>,
    env_cfg: Option<NamespaceConfig>,
) -> Result<ResolvedNamespace, ConfigError> {
    match (file_cfg, env_cfg) {
        (None, None) => Err(ConfigError::UnknownNamespace(name.to_string())),
        (Some(file), None) => Ok(ResolvedNamespace {
            name: name.to_string(),
            config: file,
            source: NamespaceSource::FileOnly,
        }),
        (None, Some(env)) => Ok(ResolvedNamespace {
            name: name.to_string(),
            config: env,
            source: NamespaceSource::EnvOnly,
        }),
        (Some(file), Some(env)) => Ok(ResolvedNamespace {
            name: name.to_string(),
            config: file.merge_over(&env),
            source: NamespaceSource::EnvOverFile,
        }),
    }
}

/// Convenience: load the on-disk servers file (without env overrides).
#[allow(dead_code)]
pub fn load_servers_file() -> Result<ServersFile, ConfigError> {
    file::load()
}
