//! L2 (v0.1.3): `inspect keychain ...` verb cluster.
//!
//! Three sub-verbs over the OS keychain (per-namespace storage of
//! SSH passphrases / passwords for cross-session reuse):
//!
//! - `inspect keychain list` — what is stored.
//! - `inspect keychain remove <ns>` — delete one entry.
//! - `inspect keychain test` — probe whether the backend is
//!   reachable on this host.
//!
//! Saving is implicit in `inspect connect --save-passphrase` and
//! lives in `commands::connect` + `ssh::master`. The keychain
//! crate's API is sync; we never expose a save sub-verb here
//! because saving without an associated `connect` would mean
//! prompting the operator for a credential without using it,
//! which is a sharp edge worth refusing.

use anyhow::{Context, Result};

use crate::cli::{
    KeychainArgs, KeychainListArgs, KeychainRemoveArgs, KeychainSubcommand, KeychainTestArgs,
};
use crate::commands::list::json_string;
use crate::error::ExitKind;
use crate::keychain;
use crate::safety::audit::{AuditEntry, AuditStore, Revert};

pub fn run(args: KeychainArgs) -> Result<ExitKind> {
    match args.command {
        KeychainSubcommand::List(a) => list::run(a),
        KeychainSubcommand::Remove(a) => remove::run(a),
        KeychainSubcommand::Test(a) => test::run(a),
    }
}

mod list {
    use super::*;

    pub fn run(args: KeychainListArgs) -> Result<ExitKind> {
        let names_result = keychain::list_namespaces();
        let (names, backend_status, backend_reason) = match names_result {
            Ok(n) => (n, "available", None),
            Err(e) => {
                // Index-IO failure or hard backend error. We surface
                // the names from the index alone (none in this branch
                // since list_namespaces failed up-front) and report
                // the underlying reason so an agent can branch on
                // backend_status.
                (Vec::new(), "unavailable", Some(e.to_string()))
            }
        };

        if args.format.is_json() {
            let mut s = String::from("{\"schema_version\":1,\"namespaces\":[");
            for (i, n) in names.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                s.push_str(&json_string(n));
            }
            s.push_str("],\"backend_status\":");
            s.push_str(&json_string(backend_status));
            if let Some(reason) = backend_reason.as_deref() {
                s.push_str(",\"reason\":");
                s.push_str(&json_string(reason));
            }
            s.push('}');
            println!("{s}");
            return Ok(ExitKind::Success);
        }

        if names.is_empty() {
            if backend_status == "unavailable" {
                println!("SUMMARY: keychain backend unavailable and no on-disk index");
                println!("DATA:");
                if let Some(reason) = backend_reason.as_deref() {
                    println!("  reason: {reason}");
                }
                println!("NEXT:    inspect keychain test    inspect help ssh");
                return Ok(ExitKind::Error);
            }
            println!("SUMMARY: no namespaces saved to the OS keychain");
            println!("DATA:    (none)");
            println!("NEXT:    inspect connect <ns> --save-passphrase");
            return Ok(ExitKind::Success);
        }

        println!(
            "SUMMARY: {n} namespace{plural} stored in the OS keychain",
            n = names.len(),
            plural = if names.len() == 1 { "" } else { "s" }
        );
        println!("DATA:");
        for n in &names {
            println!("  {n}");
        }
        println!("NEXT:    inspect keychain remove <ns>    inspect connect <ns>");
        Ok(ExitKind::Success)
    }
}

mod remove {
    use super::*;

    pub fn run(args: KeychainRemoveArgs) -> Result<ExitKind> {
        keychain::validate_namespace_for_keychain(&args.namespace)?;
        let started = std::time::Instant::now();
        let outcome = keychain::remove(&args.namespace);

        let (was_present, exit_code, error_msg) = match outcome {
            Ok(true) => (true, 0, None),
            Ok(false) => (false, 0, None),
            Err(e) => (false, 1, Some(e.to_string())),
        };
        let duration_ms = started.elapsed().as_millis() as u64;

        let mut entry = AuditEntry::new("keychain.remove", &args.namespace);
        entry.exit = exit_code;
        entry.duration_ms = duration_ms;
        entry.reason = crate::safety::validate_reason(args.reason.as_deref())?;
        entry.applied = Some(exit_code == 0);
        entry.args = format!("[was_present={was_present}]");
        entry.revert = Some(Revert::unsupported(format!(
            "keychain.remove has no inverse — re-save with \
             `inspect connect {} --save-passphrase` (prompts for the secret)",
            args.namespace
        )));
        let store = AuditStore::open().context("opening audit log")?;
        store.append(&entry)?;

        if args.format.is_json() {
            let mut s = format!(
                "{{\"schema_version\":1,\"namespace\":{ns},\"was_present\":{wp},\"audit_id\":{aid}",
                ns = json_string(&args.namespace),
                wp = was_present,
                aid = json_string(&entry.id),
            );
            if let Some(msg) = error_msg.as_deref() {
                s.push_str(",\"error\":");
                s.push_str(&json_string(msg));
            }
            s.push('}');
            println!("{s}");
            return Ok(if exit_code == 0 {
                ExitKind::Success
            } else {
                ExitKind::Error
            });
        }

        if let Some(msg) = error_msg {
            crate::error::emit(format!(
                "keychain remove '{}' failed: {msg}. \
                 hint: 'inspect keychain test' to verify backend reachability",
                args.namespace
            ));
            return Ok(ExitKind::Error);
        }

        if was_present {
            println!("SUMMARY: removed keychain entry for '{}'", args.namespace);
        } else {
            println!(
                "SUMMARY: no keychain entry for '{}' (no-op)",
                args.namespace
            );
        }
        println!("DATA:");
        println!("  audit_id:    {}", entry.id);
        println!("  was_present: {was_present}");
        println!(
            "NEXT:    inspect connect {} --save-passphrase   inspect keychain list",
            args.namespace
        );
        Ok(ExitKind::Success)
    }
}

mod test {
    use super::*;
    use crate::keychain::BackendStatus;

    pub fn run(args: KeychainTestArgs) -> Result<ExitKind> {
        let status = keychain::test_backend();
        let (label, reason) = match &status {
            BackendStatus::Available => ("available", None),
            BackendStatus::Unavailable(why) => ("unavailable", Some(why.clone())),
        };

        if args.format.is_json() {
            let mut s = format!(
                "{{\"schema_version\":1,\"status\":{lbl}",
                lbl = json_string(label),
            );
            if let Some(why) = reason.as_deref() {
                s.push_str(",\"reason\":");
                s.push_str(&json_string(why));
                s.push_str(",\"hint\":");
                s.push_str(&json_string(hint_for_reason(why)));
            }
            s.push('}');
            println!("{s}");
            return Ok(if matches!(status, BackendStatus::Available) {
                ExitKind::Success
            } else {
                ExitKind::Error
            });
        }

        match status {
            BackendStatus::Available => {
                println!("SUMMARY: keychain backend reachable");
                println!("DATA:");
                println!("  service: {}", crate::keychain::SERVICE);
                println!("  probe:   write + read + delete OK");
                println!("NEXT:    inspect connect <ns> --save-passphrase");
                Ok(ExitKind::Success)
            }
            BackendStatus::Unavailable(why) => {
                crate::error::emit(format!(
                    "keychain backend unreachable: {why}. hint: {}",
                    hint_for_reason(&why)
                ));
                Ok(ExitKind::Error)
            }
        }
    }

    fn hint_for_reason(reason: &str) -> &'static str {
        // Best-effort taxonomy. The keyring crate's error strings are
        // platform-specific so we match on substrings; an unrecognized
        // reason gets the generic hint.
        let r = reason.to_ascii_lowercase();
        if r.contains("dbus") || r.contains("session") {
            "no DBus session bus is running. Linux desktops launch one as part of the login \
             session; for headless / container hosts, prefer the env-var path \
             (key_passphrase_env / password_env) instead of --save-passphrase."
        } else if r.contains("keyring") || r.contains("collection") {
            "the OS keyring daemon may not be running. On GNOME, start `gnome-keyring-daemon`; \
             on KDE, ensure kwallet is unlocked."
        } else if r.contains("access denied") || r.contains("permission") {
            "the keychain refused access (likely a locked vault or denied prompt). Unlock the \
             keychain in the OS UI and retry."
        } else {
            "see: inspect help ssh — credential lifetime"
        }
    }
}
