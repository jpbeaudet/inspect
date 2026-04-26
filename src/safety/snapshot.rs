//! Hash-keyed snapshot store for revertible mutations (bible §8.2).
//!
//! Stored under `~/.inspect/audit/snapshots/sha256-<hex>` with mode 0600.
//! Identical content de-duplicates automatically (the file name *is* the
//! hash) so kilobyte config files cost almost nothing.

use std::path::PathBuf;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::paths::{audit_dir, ensure_home, set_dir_mode_0700, set_file_mode_0600, snapshots_dir};

pub struct SnapshotStore {
    dir: PathBuf,
}

impl SnapshotStore {
    pub fn open() -> Result<Self> {
        let _ = ensure_home();
        let audit = audit_dir();
        if !audit.exists() {
            std::fs::create_dir_all(&audit)
                .with_context(|| format!("creating {}", audit.display()))?;
        }
        let _ = set_dir_mode_0700(&audit);
        let dir = snapshots_dir();
        if !dir.exists() {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("creating {}", dir.display()))?;
        }
        let _ = set_dir_mode_0700(&dir);
        Ok(Self { dir })
    }

    /// Hash + write `data`. Returns the canonical sha256 hex (without
    /// the `sha256-` prefix) so callers can store it in audit entries.
    pub fn put(&self, data: &[u8]) -> Result<String> {
        let mut h = Sha256::new();
        h.update(data);
        let hex = hex::encode(h.finalize());
        let path = self.dir.join(format!("sha256-{hex}"));
        if !path.exists() {
            // Atomic write: tmp → rename.
            let tmp = path.with_extension("part");
            std::fs::write(&tmp, data)
                .with_context(|| format!("writing snapshot {}", tmp.display()))?;
            let _ = set_file_mode_0600(&tmp);
            std::fs::rename(&tmp, &path)
                .with_context(|| format!("renaming snapshot {}", path.display()))?;
        }
        Ok(hex)
    }

    pub fn get(&self, hash_hex: &str) -> Result<Vec<u8>> {
        let hex_only = hash_hex.strip_prefix("sha256-").unwrap_or(hash_hex);
        let path = self.dir.join(format!("sha256-{hex_only}"));
        std::fs::read(&path).with_context(|| format!("reading snapshot {}", path.display()))
    }

    pub fn path_for(&self, hash_hex: &str) -> PathBuf {
        let hex_only = hash_hex.strip_prefix("sha256-").unwrap_or(hash_hex);
        self.dir.join(format!("sha256-{hex_only}"))
    }
}

/// Stand-alone hex digest of a byte slice.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedups_identical_content() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("INSPECT_HOME", tmp.path());
        let s = SnapshotStore::open().unwrap();
        let h1 = s.put(b"hello").unwrap();
        let h2 = s.put(b"hello").unwrap();
        assert_eq!(h1, h2);
        let r = s.get(&h1).unwrap();
        assert_eq!(r, b"hello");
    }
}
