//! L2 (v0.1.3): OS keychain integration for opt-in cross-session
//! passphrase / password persistence.
//!
//! ## Default behavior is unchanged.
//!
//! Most operators want exactly the v0.1.2 behavior: ssh-agent (or
//! `inspect`'s own ControlMaster) holds the credential for the life
//! of the shell session; logout / reboot clears it; the next session
//! prompts once again. That path is the secure default and remains
//! the recommended one. The keychain is for the smaller group of
//! operators who want passphrases to survive a reboot without
//! leaving them in env vars, `.envrc` files, or shell history.
//!
//! ## Opt-in only.
//!
//! - `inspect connect <ns> --save-passphrase` (or `--save-password`)
//!   prompts once, stores the secret in the OS keychain under
//!   service `inspect-cli`, account `<ns>`. Without the flag,
//!   nothing is ever written.
//! - Subsequent `inspect connect <ns>` consults the keychain
//!   automatically — but only for namespaces that were previously
//!   saved. There is no implicit cross-namespace lookup.
//!
//! ## Backends (keyring v3.6 feature selection in Cargo.toml).
//!
//! - macOS: Keychain Services (`apple-native`)
//! - Windows: Credential Manager (`windows-native`); also reachable
//!   from WSL2.
//! - Linux: Secret Service via DBus (`sync-secret-service`); covers
//!   GNOME Keyring and KDE Wallet. `vendored` builds libdbus from
//!   source so we don't need system dev headers.
//! - Pure-Rust crypto via `crypto-rust` so we don't depend on
//!   system OpenSSL.
//!
//! ## Headless / CI fallback.
//!
//! Backends can be unreachable (no GNOME Keyring daemon, no
//! Credential Manager registered, container without a session bus,
//! etc.). The contract is loud about it:
//!
//! - **`inspect connect --save-passphrase`** with no backend warns
//!   once and falls back to the per-session prompt (no save).
//!   The verb still completes successfully — the master comes up.
//! - **Auto-retrieval** during a normal `inspect connect` (no
//!   flag, just consulting the keychain in the credential chain)
//!   silently treats backend errors as "not stored" so a
//!   transient backend issue does not interrupt every connect
//!   with a stderr line.
//! - **`inspect keychain test`** is the explicit probe: it writes
//!   a known dummy entry, reads it back, deletes it, and exits 0
//!   if everything succeeded. Non-zero with a chained hint when
//!   the backend is unreachable.
//!
//! ## Index file.
//!
//! The keyring crate's enumeration support is platform-spotty
//! (Linux Secret Service has it; macOS / Windows don't expose it
//! cleanly). We maintain a small index at
//! `~/.inspect/keychain-index` (mode 0600, one namespace per line,
//! sorted alphabetically) that records the namespaces we have
//! saved. Saves append; removes prune; the index never holds
//! secrets, only the namespace names.
//!
//! [`list`] is self-healing: it reads the index file, then for
//! each entry probes the keychain (`get_password`) and prunes any
//! entry that the backend no longer recognizes. So an operator who
//! deletes `inspect-cli/arte` directly through `Keychain Access.app`
//! (or `secret-tool clear …`) will see `inspect keychain list`
//! reflect that on the next call without manual cleanup.

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use thiserror::Error;

/// Service name used for every keyring entry inspect creates. Each
/// stored credential is `(SERVICE, namespace)` — one entry per
/// namespace.
pub const SERVICE: &str = "inspect-cli";

/// Account name used for the `keychain test` round-trip probe. The
/// double-underscore prefix ensures it cannot collide with any
/// real namespace (namespace validation rejects names that start
/// with `_`).
const TEST_ACCOUNT: &str = "__inspect_keychain_test__";

/// Result of a keychain operation classified for the caller.
#[derive(Debug)]
pub enum SaveOutcome {
    /// The secret was written and the index updated.
    Saved,
    /// The secret was already present and unchanged (idempotent
    /// re-save is treated as a no-op so the audit log doesn't
    /// double-count operator intent).
    AlreadyPresent,
}

/// Result of a backend reachability probe.
#[derive(Debug)]
pub enum BackendStatus {
    /// Round-trip write/read/delete all succeeded.
    Available,
    /// At least one of write / read / delete failed. Carries the
    /// underlying error message for the operator's chained hint.
    Unavailable(String),
}

/// Keychain-layer errors. We keep the surface small and route
/// everything through `anyhow::Error` at the verb boundary so
/// callers don't have to match on a long taxonomy. The
/// distinguishing variant is `BackendUnavailable`, which the
/// credential chain treats as "fall through" (silent) rather
/// than as a hard failure.
#[derive(Debug, Error)]
pub enum KeychainError {
    #[error("keychain backend unavailable: {0}")]
    BackendUnavailable(String),
    #[error("keychain index I/O at '{path}': {source}")]
    IndexIo {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// Save `secret` for `namespace` in the OS keychain. Idempotent:
/// re-saving the same secret returns `AlreadyPresent`.
///
/// On backend failure, returns `KeychainError::BackendUnavailable`
/// — the caller chooses whether to surface the warning or treat it
/// as silent fall-through.
pub fn save(namespace: &str, secret: &str) -> std::result::Result<SaveOutcome, KeychainError> {
    let entry = entry_for(namespace).map_err(KeychainError::BackendUnavailable)?;

    // Idempotent fast path: only write if the value actually changes.
    // The Linux Secret Service backend does not deduplicate writes
    // and a no-op re-save would create a duplicate entry the user
    // could see in `seahorse`. macOS / Windows overwrite cleanly,
    // but the explicit check keeps semantics uniform.
    match entry.get_password() {
        Ok(existing) if existing == secret => {
            update_index(namespace, /*present=*/ true)?;
            return Ok(SaveOutcome::AlreadyPresent);
        }
        _ => {}
    }

    entry
        .set_password(secret)
        .map_err(|e| KeychainError::BackendUnavailable(e.to_string()))?;
    update_index(namespace, /*present=*/ true)?;
    Ok(SaveOutcome::Saved)
}

/// Look up the saved secret for `namespace`. Returns:
///
/// - `Ok(Some(secret))` — entry exists and was read.
/// - `Ok(None)` — no entry stored for this namespace (the common
///   case for first-time connect or for a namespace that opted
///   out).
/// - `Err(BackendUnavailable)` — the backend itself is broken or
///   absent. Callers in the auto-retrieval credential chain treat
///   this the same as `Ok(None)` so a transient backend issue
///   doesn't block every connect with a stderr line.
pub fn get(namespace: &str) -> std::result::Result<Option<String>, KeychainError> {
    let entry = entry_for(namespace).map_err(KeychainError::BackendUnavailable)?;
    match entry.get_password() {
        Ok(s) => Ok(Some(s)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(KeychainError::BackendUnavailable(e.to_string())),
    }
}

/// Remove the saved entry for `namespace`. Returns `true` if an
/// entry was actually deleted, `false` if no entry existed.
///
/// On backend failure, returns `BackendUnavailable`. The index
/// file is pruned regardless of whether the keychain delete
/// succeeded — if the entry is gone from the operator's view of
/// the keychain (because they deleted it externally) we want the
/// index to reflect that, even if the backend now reports
/// "no entry to delete".
pub fn remove(namespace: &str) -> std::result::Result<bool, KeychainError> {
    let entry = entry_for(namespace).map_err(KeychainError::BackendUnavailable)?;
    let was_present = match entry.delete_credential() {
        Ok(()) => true,
        Err(keyring::Error::NoEntry) => false,
        Err(e) => return Err(KeychainError::BackendUnavailable(e.to_string())),
    };
    update_index(namespace, /*present=*/ false)?;
    Ok(was_present)
}

/// Enumerate the namespaces that have saved keychain entries.
/// Self-healing: the on-disk index is consulted first, then each
/// entry is probed against the live backend. Any index entry the
/// backend no longer recognizes is pruned silently so subsequent
/// calls reflect reality.
///
/// Returns the namespace names sorted alphabetically. Never
/// returns secret material.
pub fn list_namespaces() -> std::result::Result<Vec<String>, KeychainError> {
    let names = read_index()?;
    let mut alive: Vec<String> = Vec::with_capacity(names.len());
    let mut pruned = false;
    for name in names {
        match get(&name) {
            Ok(Some(_)) => alive.push(name),
            Ok(None) => {
                pruned = true;
            }
            // On backend errors we keep the index entry — a transient
            // issue should not nuke the operator's record of what
            // they saved. The next successful list call will resolve.
            Err(_) => alive.push(name),
        }
    }
    if pruned {
        write_index(&alive)?;
    }
    Ok(alive)
}

/// Probe whether the OS keychain backend is reachable. Performs a
/// full round-trip (write a known dummy, read it back, delete it)
/// so transient open-but-broken backends are caught.
pub fn test_backend() -> BackendStatus {
    let entry = match entry_for(TEST_ACCOUNT) {
        Ok(e) => e,
        Err(msg) => return BackendStatus::Unavailable(msg),
    };
    let probe = "ok";
    if let Err(e) = entry.set_password(probe) {
        return BackendStatus::Unavailable(format!("set: {e}"));
    }
    let got = match entry.get_password() {
        Ok(s) => s,
        Err(e) => {
            // Best-effort cleanup; we only get here if read failed
            // post-write, so the entry may or may not exist.
            let _ = entry.delete_credential();
            return BackendStatus::Unavailable(format!("get: {e}"));
        }
    };
    if got != probe {
        let _ = entry.delete_credential();
        return BackendStatus::Unavailable(format!(
            "round-trip mismatch (wrote '{probe}', read '{got}')"
        ));
    }
    if let Err(e) = entry.delete_credential() {
        return BackendStatus::Unavailable(format!("delete: {e}"));
    }
    BackendStatus::Available
}

fn entry_for(namespace: &str) -> std::result::Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, namespace).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Index file: ~/.inspect/keychain-index
// ---------------------------------------------------------------------------

mod index;

fn index_path() -> PathBuf {
    crate::paths::inspect_home().join("keychain-index")
}

fn read_index() -> std::result::Result<Vec<String>, KeychainError> {
    let path = index_path();
    index::read(&path).map_err(|e| KeychainError::IndexIo {
        path: path.display().to_string(),
        source: e,
    })
}

fn write_index(names: &[String]) -> std::result::Result<(), KeychainError> {
    let path = index_path();
    index::write(&path, names).map_err(|e| KeychainError::IndexIo {
        path: path.display().to_string(),
        source: e,
    })
}

fn update_index(namespace: &str, present: bool) -> std::result::Result<(), KeychainError> {
    let mut names = read_index()?;
    let pos = names.binary_search(&namespace.to_string());
    match (pos, present) {
        (Ok(_), true) | (Err(_), false) => {
            // Already in the desired state; no write.
        }
        (Err(idx), true) => {
            names.insert(idx, namespace.to_string());
            write_index(&names)?;
        }
        (Ok(idx), false) => {
            names.remove(idx);
            write_index(&names)?;
        }
    }
    Ok(())
}

/// Validate that `name` is a syntactically acceptable namespace for
/// keychain operations. Mirrors the connect-time namespace name
/// rules so a typo here can't accidentally save under a name the
/// resolver wouldn't accept.
pub fn validate_namespace_for_keychain(name: &str) -> Result<()> {
    crate::config::namespace::validate_namespace_name(name).map_err(|e| anyhow!("{e}"))?;
    if name.starts_with("__") {
        return Err(anyhow!(
            "namespace '{name}' is reserved (the '__' prefix is used by inspect's internal keychain probes)"
        ));
    }
    Ok(())
}

/// Best-effort: ensure `~/.inspect/` exists with mode 0700 before
/// the first index write. Mirrors the pattern in `paths::ensure_home`.
pub(crate) fn ensure_home() -> std::result::Result<(), KeychainError> {
    crate::paths::ensure_home()
        .map(|_| ())
        .map_err(|e| KeychainError::IndexIo {
            path: crate::paths::inspect_home().display().to_string(),
            source: std::io::Error::other(e.to_string()),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l2_save_outcome_variants_distinguish_first_save_from_idempotent() {
        // Pure type test — the variant set is part of the contract
        // surface that the verb code matches on.
        let saved = SaveOutcome::Saved;
        let already = SaveOutcome::AlreadyPresent;
        assert!(matches!(saved, SaveOutcome::Saved));
        assert!(matches!(already, SaveOutcome::AlreadyPresent));
    }

    #[test]
    fn l2_validate_namespace_rejects_internal_prefix() {
        assert!(validate_namespace_for_keychain("arte").is_ok());
        assert!(validate_namespace_for_keychain("legacy-box").is_ok());
        // The internal probe account name `__inspect_keychain_test__`
        // is rejected by the underlying `validate_namespace_name`
        // rules (leading underscore is invalid in a namespace name)
        // — the explicit "__"-prefix gate in
        // `validate_namespace_for_keychain` is belt-and-braces. Either
        // rejection path is sufficient as long as the verb refuses
        // the probe account.
        let err = validate_namespace_for_keychain(TEST_ACCOUNT)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("reserved") || err.contains("invalid namespace name"),
            "expected rejection of probe account: {err}"
        );
    }

    #[test]
    fn l2_validate_namespace_rejects_invalid_names() {
        // Already covered by validate_namespace_name's own tests, but
        // we keep the regression here so a future relaxation of the
        // namespace rules doesn't silently widen the keychain surface.
        assert!(validate_namespace_for_keychain("Bad Name").is_err());
        assert!(validate_namespace_for_keychain("").is_err());
    }
}
