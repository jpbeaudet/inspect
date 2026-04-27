//! Filesystem layout for `~/.inspect/` and permission enforcement.
//!
//! The bible mandates mode 0600 on all config, socket, and audit files.

use std::path::{Path, PathBuf};

use crate::error::ConfigError;

/// Environment override for the inspect home directory. Useful for tests and
/// for users who want to relocate config (e.g. into a sandboxed dir on CI).
pub const INSPECT_HOME_ENV: &str = "INSPECT_HOME";

/// Returns the inspect home directory, honoring `INSPECT_HOME` if set.
pub fn inspect_home() -> PathBuf {
    if let Ok(custom) = std::env::var(INSPECT_HOME_ENV) {
        if !custom.is_empty() {
            return PathBuf::from(custom);
        }
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".inspect")
}

pub fn servers_toml() -> PathBuf {
    inspect_home().join("servers.toml")
}

pub fn aliases_toml() -> PathBuf {
    inspect_home().join("aliases.toml")
}

pub fn groups_toml() -> PathBuf {
    inspect_home().join("groups.toml")
}

pub fn sockets_dir() -> PathBuf {
    inspect_home().join("sockets")
}

pub fn audit_dir() -> PathBuf {
    inspect_home().join("audit")
}

pub fn snapshots_dir() -> PathBuf {
    audit_dir().join("snapshots")
}

/// P10 (v0.1.1): root for `--since-last` cursors. One small file per
/// (namespace, service) pair, mode 0600 inside a 0700 directory.
pub fn cursors_dir() -> PathBuf {
    inspect_home().join("cursors")
}

/// Per-(namespace, service) cursor file path. Service name is sanitized
/// (slashes/colons stripped) so it always fits a single filename.
pub fn cursor_file(namespace: &str, service: &str) -> PathBuf {
    let sanitize = |s: &str| {
        s.chars()
            .map(|c| match c {
                '/' | '\\' | ':' | ' ' | '\t' | '\n' => '_',
                c => c,
            })
            .collect::<String>()
    };
    cursors_dir()
        .join(sanitize(namespace))
        .join(format!("{}.kv", sanitize(service)))
}

/// Ensure the inspect home directory exists with mode 0700 on unix.
pub fn ensure_home() -> Result<PathBuf, ConfigError> {
    let home = inspect_home();
    if !home.exists() {
        std::fs::create_dir_all(&home).map_err(|e| ConfigError::Io {
            path: home.display().to_string(),
            source: e,
        })?;
    }
    set_dir_mode_0700(&home)?;
    Ok(home)
}

/// Set 0700 on a directory (unix only; no-op elsewhere).
pub fn set_dir_mode_0700(path: &Path) -> Result<(), ConfigError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o700);
        std::fs::set_permissions(path, perms).map_err(|e| ConfigError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Set 0600 on a file (unix only; no-op elsewhere).
pub fn set_file_mode_0600(path: &Path) -> Result<(), ConfigError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms).map_err(|e| ConfigError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Verify that a config file has mode 0600. Returns Ok(()) on non-unix.
pub fn check_file_mode_0600(path: &Path) -> Result<(), ConfigError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(path).map_err(|e| ConfigError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        let mode = meta.permissions().mode() & 0o777;
        if mode != 0o600 {
            return Err(ConfigError::UnsafePermissions {
                path: path.display().to_string(),
                mode,
            });
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Shared mutex for tests that mutate the process-wide `INSPECT_HOME`
/// env var. Without this, tests in different modules run in parallel,
/// clobber each other's tempdir, and produce flaky CI failures.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
