//! Non-interactive passphrase delivery via `SSH_ASKPASS`.
//!
//! When the user has configured `key_passphrase_env = "MY_VAR"`, we want
//! `ssh` to use that value without ever writing it to disk and without
//! requiring an interactive TTY (so CI works).
//!
//! Mechanism:
//!
//! 1. We write a tiny shell script to a temp file (mode 0700) whose body
//!    only echoes the value of an environment variable whose **name** is
//!    passed via `INSPECT_PASSPHRASE_VAR`. The script never embeds the
//!    secret itself.
//! 2. We export the user's passphrase env var (already in our environment),
//!    `SSH_ASKPASS=<script>`, and `SSH_ASKPASS_REQUIRE=force` so OpenSSH
//!    invokes the helper instead of prompting on the TTY.
//! 3. The script and its parent dir are deleted on drop.
//!
//! Caveat: `SSH_ASKPASS_REQUIRE=force` requires OpenSSH ≥ 8.4 (2020). On
//! older clients we fall back to setsid + lacking a TTY, which also makes
//! ssh use askpass.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use tempfile::TempDir;

/// Owns a temp directory containing the askpass script. The directory is
/// removed on drop.
pub struct AskpassScript {
    _dir: TempDir,
    script_path: PathBuf,
    var_name: String,
}

impl AskpassScript {
    /// Create a new askpass helper that reads the passphrase from the
    /// environment variable named `var_name`.
    pub fn new(var_name: &str) -> Result<Self> {
        if var_name.is_empty() || !is_safe_var_name(var_name) {
            return Err(anyhow!(
                "invalid passphrase env var name '{var_name}'; must be [A-Z_][A-Z0-9_]*"
            ));
        }
        let dir = tempfile::Builder::new()
            .prefix("inspect-askpass-")
            .tempdir()?;
        // tempdir() creates with 0700 already on unix.
        let script_path = dir.path().join("askpass.sh");

        // The script body intentionally contains no secret: it dereferences
        // an env var by indirection. The variable's name is provided via
        // `INSPECT_PASSPHRASE_VAR` so even the script content can be reused.
        let body = "#!/bin/sh\n\
            : \"${INSPECT_PASSPHRASE_VAR:?}\"\n\
            eval \"printf '%s\\n' \\\"\\${${INSPECT_PASSPHRASE_VAR}}\\\"\"\n";

        write_script_0700(&script_path, body)?;
        Ok(Self {
            _dir: dir,
            script_path,
            var_name: var_name.to_string(),
        })
    }

    #[cfg(test)]
    pub fn script_path(&self) -> &Path {
        &self.script_path
    }

    /// Environment variables to set for `ssh` so it uses this askpass.
    /// Caller must already have the passphrase variable populated in their
    /// process environment (we never accept the secret value through this
    /// API to avoid copies that escape `zeroize`).
    pub fn env_vars(&self) -> Vec<(OsString, OsString)> {
        vec![
            (
                OsString::from("SSH_ASKPASS"),
                OsString::from(self.script_path.as_os_str()),
            ),
            (
                OsString::from("SSH_ASKPASS_REQUIRE"),
                OsString::from("force"),
            ),
            (
                OsString::from("INSPECT_PASSPHRASE_VAR"),
                OsString::from(&self.var_name),
            ),
            // Detach from controlling terminal; OpenSSH otherwise prefers the
            // tty for prompts. Setting DISPLAY ensures older OpenSSH builds
            // (< 8.4) without SSH_ASKPASS_REQUIRE still fall through to the
            // helper.
            (OsString::from("DISPLAY"), OsString::from(":0")),
        ]
    }
}

fn write_script_0700(path: &Path, body: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o700)
            .open(path)?;
        f.write_all(body.as_bytes())?;
        f.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let mut f = std::fs::File::create(path)?;
        f.write_all(body.as_bytes())?;
    }
    Ok(())
}

fn is_safe_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_var_names() {
        assert!(AskpassScript::new("").is_err());
        assert!(AskpassScript::new("bad name").is_err());
        assert!(AskpassScript::new("9LEAD").is_err());
        assert!(AskpassScript::new("rm -rf /").is_err());
    }

    #[test]
    fn creates_executable_script() {
        let s = AskpassScript::new("MY_PASS").expect("create");
        let meta = std::fs::metadata(s.script_path()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(meta.permissions().mode() & 0o777, 0o700);
        }
        let body = std::fs::read_to_string(s.script_path()).unwrap();
        assert!(body.starts_with("#!/bin/sh"));
        assert!(!body.contains("MY_PASS_VALUE"));
    }
}
