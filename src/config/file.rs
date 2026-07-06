//! On-disk storage for `~/.inspect/servers.toml`.
//!
//! File layout:
//!
//! ```toml
//! schema_version = 2
//!
//! [namespaces.arte]              # docker (default) namespace
//! host = "arte.example.internal"
//! user = "ubuntu"
//! port = 22
//! key_path = "~/.ssh/id_ed25519"
//! key_passphrase_env = "ARTE_SSH_PASSPHRASE"
//!
//! [namespaces.staging-k8s]       # kubernetes namespace
//! type = "k8s"
//! kubeconfig = "~/.kube/staging.yaml"  # optional
//! context = "staging"                  # optional
//! namespace = "default"                # optional (k8s namespace)
//! ```
//!
//! Schema version 2 adds the `type` / `kubeconfig` /
//! `context` / `namespace` fields for the kubernetes runtime medium.
//! All four are optional and skipped when unset, so a v1 file (no
//! `type`) loads unchanged as a docker namespace — the bump is purely
//! additive.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::namespace::NamespaceConfig;
use crate::error::ConfigError;
use crate::paths;

pub const SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServersFile {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub namespaces: BTreeMap<String, NamespaceConfig>,
}

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

impl Default for ServersFile {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            namespaces: BTreeMap::new(),
        }
    }
}

/// Load `servers.toml` from the inspect home. Returns an empty `ServersFile`
/// if the file does not exist. If it does exist, enforces 0600 permissions
/// before reading.
pub fn load() -> Result<ServersFile, ConfigError> {
    let path = paths::servers_toml();
    if !path.exists() {
        return Ok(ServersFile::default());
    }
    paths::check_file_mode_0600(&path)?;
    load_from(&path)
}

pub fn load_from(path: &Path) -> Result<ServersFile, ConfigError> {
    let bytes = std::fs::read(path).map_err(|e| ConfigError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let parsed: ServersFile = toml::from_str(&text).map_err(|e| ConfigError::Parse {
        path: path.display().to_string(),
        source: e,
    })?;
    Ok(parsed)
}

/// Persist `servers.toml` atomically with mode 0600.
pub fn save(file: &ServersFile) -> Result<(), ConfigError> {
    paths::ensure_home()?;
    let path = paths::servers_toml();
    save_to(&path, file)
}

pub fn save_to(path: &Path, file: &ServersFile) -> Result<(), ConfigError> {
    let serialized = toml::to_string_pretty(file)?;
    write_atomic_0600(path, serialized.as_bytes())
}

/// Atomically write `bytes` to `path` with mode 0600. The temp file is
/// created in the same directory so that rename(2) stays on a single
/// filesystem.
fn write_atomic_0600(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| ConfigError::Io {
        path: parent.display().to_string(),
        source: e,
    })?;
    let tmp = parent.join(format!(
        ".{}.tmp.{}",
        path.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "servers.toml".into()),
        std::process::id()
    ));

    // Open with mode 0600 from the start on unix.
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| ConfigError::Io {
                path: tmp.display().to_string(),
                source: e,
            })?;
        file.write_all(bytes).map_err(|e| ConfigError::Io {
            path: tmp.display().to_string(),
            source: e,
        })?;
        file.sync_all().map_err(|e| ConfigError::Io {
            path: tmp.display().to_string(),
            source: e,
        })?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&tmp, bytes).map_err(|e| ConfigError::Io {
            path: tmp.display().to_string(),
            source: e,
        })?;
    }

    std::fs::rename(&tmp, path).map_err(|e| ConfigError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    paths::set_file_mode_0600(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn k2_schema_version_bumped() {
        // Bumps the servers.toml schema to 2 for the new
        // `type` / `kubeconfig` / `context` / `namespace` k8s fields.
        // Bumped the servers.toml schema 1 -> 2.
        assert_eq!(SCHEMA_VERSION, 2);
        assert_eq!(ServersFile::default().schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn k2_v1_file_without_type_loads_as_docker() {
        // A v1 file with no `type` field still parses and its
        // namespace resolves as docker — the bump is purely additive.
        let toml = "schema_version = 1\n\n[namespaces.arte]\nhost = \"h\"\nuser = \"u\"\n";
        let parsed: ServersFile = toml::from_str(toml).expect("v1 file parses");
        let ns = &parsed.namespaces["arte"];
        assert!(ns.runtime_type.is_none());
        assert!(ns.validate("arte").is_ok());
    }
}
