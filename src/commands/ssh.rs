//! `inspect ssh ...` — SSH-related management subcommands.
//!
//! introduces the first sub-verb: `inspect ssh add-key
//! <ns>`, the audited migration path off password-only legacy
//! servers. Future SSH management verbs (key rotation, agent
//! priming, etc.) plug into the same dispatch.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{anyhow, Context, Result};

use crate::cli::{SshAddKeyArgs, SshArgs, SshSubcommand};
use crate::config::file as config_file;
use crate::config::namespace::validate_namespace_name;
use crate::config::resolver;
use crate::error::ExitKind;
use crate::safety::audit::{AuditEntry, AuditStore, Revert};
use crate::ssh::exec::{run_remote, RunOpts};
use crate::ssh::master::{check_socket, password_warned_path, socket_path, MasterStatus};
use crate::ssh::SshTarget;
use crate::verbs::quote::shquote;

pub fn run(args: SshArgs) -> Result<ExitKind> {
    match args.command {
        SshSubcommand::AddKey(a) => add_key::run(a),
    }
}

mod add_key {
    use super::*;

    pub fn run(args: SshAddKeyArgs) -> Result<ExitKind> {
        validate_namespace_name(&args.namespace)?;
        let resolved = resolver::resolve(&args.namespace)?;
        resolved.config.validate(&resolved.name)?;
        let target = SshTarget::from_resolved(&resolved)?;

        let key_path = match &args.key {
            Some(p) => p.clone(),
            None => default_key_path(&args.namespace)?,
        };
        let pub_path = pubkey_path(&key_path);
        let supplied_key = args.key.is_some();

        // Reject `--key <path>` when the public half is missing — the
        // verb refuses to ssh-keygen against an operator-supplied key,
        // since silently regenerating their key material would be
        // surprising.
        if supplied_key && !pub_path.exists() {
            crate::error::emit(format!(
                "--key '{}' has no matching public key at '{}'; \
                 generate the .pub half locally first (ssh-keygen -y -f <key> > <key>.pub) \
                 or omit --key to let inspect generate a fresh ed25519 keypair",
                key_path.display(),
                pub_path.display(),
            ));
            return Ok(ExitKind::Error);
        }

        let already_password_auth = resolved.config.auth.as_deref() == Some("password");

        if !args.apply {
            print_dry_run(&args, &key_path, &pub_path, already_password_auth);
            return Ok(ExitKind::Success);
        }

        // --apply: a live ssh session is required (we install over
        // the open master so the operator's password is entered
        // exactly once during the migration).
        let socket = socket_path(&args.namespace);
        if !matches!(check_socket(&socket, &target), MasterStatus::Alive) {
            crate::error::emit(format!(
                "namespace '{}' has no live ssh session; \
                 run 'inspect connect {0}' first so the install can ride the open master. \
                 see: inspect help ssh",
                args.namespace
            ));
            return Ok(ExitKind::Error);
        }

        // Generate the keypair if needed.
        let generated = if pub_path.exists() && key_path.exists() {
            false
        } else {
            generate_ed25519(&key_path, &args.namespace)?;
            true
        };
        let pubkey_line = read_pubkey(&pub_path)?;

        // Install on the remote (idempotent).
        let started = Instant::now();
        let install_out = install_pubkey(&args.namespace, &target, &pubkey_line)
            .context("installing public key on remote")?;
        if !install_out.installed_or_present {
            crate::error::emit(format!(
                "key install on '{}' did not verify: \
                 the key line was not present in ~/.ssh/authorized_keys after the write. \
                 see: inspect help ssh",
                args.namespace,
            ));
            return Ok(ExitKind::Error);
        }
        let install_dur = started.elapsed().as_millis() as u64;

        // Optionally rewrite servers.toml.
        let mut config_rewritten = false;
        if !args.no_rewrite_config && already_password_auth {
            config_rewritten = maybe_rewrite_config(&args.namespace, &key_path)?;
        }

        // Audit-log the run.
        let mut entry = AuditEntry::new("ssh.add-key", &args.namespace);
        entry.exit = 0;
        entry.duration_ms = install_dur;
        entry.reason = crate::safety::validate_reason(args.reason.as_deref())?;
        entry.applied = Some(true);
        entry.args = format!(
            "[key_path={path}] [generated={gen}] [installed=true] [config_rewritten={rew}]",
            path = key_path.display(),
            gen = generated,
            rew = config_rewritten,
        );
        // There
        // is no clean automatic inverse for `ssh.add-key` — revoking
        // a deployed public key is an operator decision (it may have
        // already been used to bootstrap further automation). Use
        // `Unsupported` rather than `command_pair` so
        // `inspect revert <add-key-audit-id>` refuses loudly with
        // the manual remote command in the preview, instead of
        // silently dispatching the forward CLI wrapper. Matches the
        // "Never silently no-op".
        entry.revert = Some(Revert::unsupported(format!(
            "manual revoke required: ssh {ns} -- 'sed -i \"\\\\|{line}|d\" ~/.ssh/authorized_keys'",
            ns = args.namespace,
            line = pubkey_line.replace('|', "\\|"),
        )));

        let store = AuditStore::open()?;
        store.append(&entry)?;

        // Clear the password-warned marker so a future re-onboarding
        // that flips back to password auth re-warns once.
        if config_rewritten {
            let _ = fs::remove_file(password_warned_path(&args.namespace));
        }

        // Output summary.
        if args.format.is_json() {
            println!(
                "{{\"schema_version\":1,\"namespace\":{ns},\
                 \"key_path\":{path},\"generated\":{gen},\
                 \"installed\":true,\"config_rewritten\":{rew},\
                 \"audit_id\":{aid}}}",
                ns = json_string(&args.namespace),
                path = json_string(&key_path.display().to_string()),
                gen = generated,
                rew = config_rewritten,
                aid = json_string(&entry.id),
            );
        } else {
            println!(
                "SUMMARY: ssh.add-key on '{}' — installed=true generated={gen} config_rewritten={rew}",
                args.namespace,
                gen = generated,
                rew = config_rewritten,
            );
            println!("DATA:");
            println!("  key_path:   {}", key_path.display());
            println!("  pubkey:     {}", pub_path.display());
            println!("  audit_id:   {}", entry.id);
            if config_rewritten {
                println!("NEXT:    inspect connect {} (now key auth)", args.namespace);
            } else if already_password_auth {
                println!(
                    "NEXT:    edit ~/.inspect/servers.toml: set auth = \"key\", key_path = \"{}\"",
                    key_path.display()
                );
            } else {
                println!(
                    "NEXT:    inspect connect {} --json    (verify still authenticates)",
                    args.namespace
                );
            }
        }
        Ok(ExitKind::Success)
    }

    fn print_dry_run(
        args: &SshAddKeyArgs,
        key_path: &Path,
        pub_path: &Path,
        already_password_auth: bool,
    ) {
        let exists = key_path.exists() && pub_path.exists();
        let action = if exists {
            format!("would install existing {}", pub_path.display())
        } else {
            format!(
                "would generate ed25519 keypair at {} and install {}",
                key_path.display(),
                pub_path.display()
            )
        };
        let flip = if args.no_rewrite_config {
            "would NOT rewrite servers.toml (--no-rewrite-config)"
        } else if !already_password_auth {
            "would NOT rewrite servers.toml (auth is already \"key\")"
        } else {
            "would prompt to rewrite servers.toml: auth=\"key\", drop password_env/session_ttl"
        };
        println!(
            "DRY-RUN: inspect ssh add-key {} would: {}; {}",
            args.namespace, action, flip
        );
        println!("hint: re-run with --apply to perform.");
    }

    fn maybe_rewrite_config(namespace: &str, key_path: &Path) -> Result<bool> {
        // Non-tty: auto-decline.
        if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            eprintln!(
                "note: stdin is not a tty; auto-declining the auth-flip prompt. \
                 Re-run interactively or edit ~/.inspect/servers.toml by hand."
            );
            return Ok(false);
        }
        eprint!(
            "Flip namespace '{namespace}' to auth=\"key\" with key_path=\"{}\" \
             and drop password_env/session_ttl? [y/N] ",
            key_path.display()
        );
        let mut answer = String::new();
        std::io::stdin()
            .read_line(&mut answer)
            .context("reading auth-flip confirmation")?;
        if !matches!(answer.trim(), "y" | "Y" | "yes" | "YES") {
            eprintln!("note: auth-flip declined; servers.toml unchanged");
            return Ok(false);
        }
        let mut servers = config_file::load().context("loading servers.toml")?;
        let cfg = servers.namespaces.get_mut(namespace).ok_or_else(|| {
            anyhow!("namespace '{namespace}' vanished from servers.toml mid-flip")
        })?;
        cfg.auth = Some("key".to_string());
        cfg.key_path = Some(key_path.display().to_string());
        cfg.password_env = None;
        cfg.session_ttl = None;
        config_file::save(&servers).context("writing servers.toml")?;
        eprintln!(
            "note: servers.toml updated — auth=\"key\", key_path=\"{}\"",
            key_path.display()
        );
        Ok(true)
    }
}

fn default_key_path(namespace: &str) -> Result<PathBuf> {
    let home = dirs_home().ok_or_else(|| anyhow!("could not determine $HOME"))?;
    Ok(home
        .join(".ssh")
        .join(format!("inspect_{namespace}_ed25519")))
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn pubkey_path(key_path: &Path) -> PathBuf {
    let mut p = key_path.as_os_str().to_owned();
    p.push(".pub");
    PathBuf::from(p)
}

fn generate_ed25519(key_path: &Path, namespace: &str) -> Result<()> {
    if let Some(parent) = key_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    let comment = format!("inspect:{namespace}");
    let status = Command::new("ssh-keygen")
        .arg("-t")
        .arg("ed25519")
        .arg("-N")
        .arg("")
        .arg("-C")
        .arg(&comment)
        .arg("-f")
        .arg(key_path)
        .status()
        .with_context(|| "invoking ssh-keygen (is openssh-client installed?)")?;
    if !status.success() {
        return Err(anyhow!(
            "ssh-keygen exited with status {} while generating {}",
            status.code().unwrap_or(-1),
            key_path.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(key_path, fs::Permissions::from_mode(0o600));
        let _ = fs::set_permissions(pubkey_path(key_path), fs::Permissions::from_mode(0o644));
    }
    Ok(())
}

fn read_pubkey(pub_path: &Path) -> Result<String> {
    let raw =
        fs::read_to_string(pub_path).with_context(|| format!("reading {}", pub_path.display()))?;
    let line = raw.lines().next().unwrap_or("").trim().to_string();
    if line.is_empty() {
        return Err(anyhow!("public key file '{}' is empty", pub_path.display()));
    }
    Ok(line)
}

struct InstallOutcome {
    installed_or_present: bool,
}

fn install_pubkey(
    namespace: &str,
    target: &SshTarget,
    pubkey_line: &str,
) -> Result<InstallOutcome> {
    // Idempotent install: append-if-absent. Matches the ssh-copy-id
    // contract but rides the open master rather than re-authing.
    // The remote shell:
    //   1. ensures ~/.ssh exists with mode 0700,
    //   2. ensures ~/.ssh/authorized_keys exists with mode 0600,
    //   3. appends the public key line iff grep -F -x does not find
    //      it already.
    let quoted_line = shquote(pubkey_line);
    let install_cmd = format!(
        "set -e; \
         umask 077; \
         mkdir -p ~/.ssh; \
         chmod 700 ~/.ssh; \
         touch ~/.ssh/authorized_keys; \
         chmod 600 ~/.ssh/authorized_keys; \
         if ! grep -F -x -q -- {quoted} ~/.ssh/authorized_keys; then \
           printf '%s\\n' {quoted} >> ~/.ssh/authorized_keys; \
         fi",
        quoted = quoted_line,
    );
    let install_out = run_remote(namespace, target, &install_cmd, RunOpts::with_timeout(30))?;
    if !install_out.ok() {
        return Err(anyhow!(
            "remote install command exited {}: {}",
            install_out.exit_code,
            install_out.stderr.trim()
        ));
    }

    // Verify the line is now present.
    let verify_cmd = format!(
        "grep -F -x -q -- {quoted} ~/.ssh/authorized_keys && echo present || echo missing",
        quoted = quoted_line,
    );
    let verify_out = run_remote(namespace, target, &verify_cmd, RunOpts::with_timeout(15))?;
    Ok(InstallOutcome {
        installed_or_present: verify_out.stdout.trim() == "present",
    })
}

fn json_string(s: &str) -> String {
    crate::commands::list::json_string(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pubkey_path_appends_pub() {
        let key = PathBuf::from("/tmp/inspect_arte_ed25519");
        assert_eq!(
            pubkey_path(&key),
            PathBuf::from("/tmp/inspect_arte_ed25519.pub")
        );
    }

    #[test]
    fn default_key_path_uses_home_and_ns() {
        let saved = std::env::var_os("HOME");
        std::env::set_var("HOME", "/home/op");
        let p = default_key_path("legacy-box").unwrap();
        assert_eq!(p, PathBuf::from("/home/op/.ssh/inspect_legacy-box_ed25519"));
        match saved {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
}
