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
            std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let _ = set_dir_mode_0700(&dir);
        Ok(Self { dir })
    }

    /// Hash + write `data`. Returns the canonical sha256 hex (without
    /// the `sha256-` prefix) so callers can store it in audit entries.
    pub fn put(&self, data: &[u8]) -> Result<String> {
        let hex = sha256_hex(data);
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
        let hex_only = strip_sha256_prefix(hash_hex);
        let path = self.dir.join(format!("sha256-{hex_only}"));
        std::fs::read(&path).with_context(|| format!("reading snapshot {}", path.display()))
    }

    pub fn path_for(&self, hash_hex: &str) -> PathBuf {
        let hex_only = strip_sha256_prefix(hash_hex);
        self.dir.join(format!("sha256-{hex_only}"))
    }
}

/// Strip either `sha256-` (the on-disk filename prefix) or `sha256:`
/// (the audit-entry prefix used in `previous_hash` / `new_hash`) from
/// a hash string. Field smoke (v0.1.3) caught this mismatch when
/// `inspect revert` of an `edit` entry built the path
/// `sha256-sha256:HEX` and got ENOENT — capture-site stored the
/// colon form, snapshot-store only stripped the dash form.
fn strip_sha256_prefix(s: &str) -> &str {
    s.strip_prefix("sha256-")
        .or_else(|| s.strip_prefix("sha256:"))
        .unwrap_or(s)
}

/// Stand-alone hex digest of a byte slice.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex_encode(&h.finalize())
}

/// Lowercase hex encoding of `bytes`. Native replacement for
/// `hex::encode` per the Dependency Policy in CLAUDE.md — SHA-256
/// hex digests are the only hex consumer and a 64-char encoder is
/// well under the 500-LOC threshold for "import vs write native".
pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_encode_matches_classic_examples() {
        assert_eq!(hex_encode(&[]), "");
        assert_eq!(hex_encode(&[0x00]), "00");
        assert_eq!(hex_encode(&[0xff]), "ff");
        assert_eq!(hex_encode(&[0xde, 0xad, 0xbe, 0xef]), "deadbeef");
        // SHA-256 of "" — RFC 6234 well-known vector.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn dedups_identical_content() {
        let _guard = crate::paths::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
