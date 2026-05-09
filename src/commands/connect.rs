//! `inspect connect <ns>` — open or reuse a persistent SSH master.

use std::collections::BTreeMap;

use anyhow::Context;

use serde_json::{json, Value};

use crate::cli::ConnectArgs;
use crate::config::file as config_file;
use crate::config::namespace::{is_valid_env_key, validate_namespace_name};
use crate::config::resolver;
use crate::error::ExitKind;
use crate::safety::audit::{AuditEntry, AuditStore, Revert};
use crate::ssh::master::AuthSelection;
use crate::ssh::{self, SshTarget};
use crate::verbs::output::{NextStep, OutputDoc};

pub fn run(args: ConnectArgs) -> anyhow::Result<ExitKind> {
    validate_namespace_name(&args.namespace)?;

    // P8-C fix (v0.1.3): stamp the F18 transcript with the resolved
    // namespace immediately so every `inspect connect <ns>`
    // invocation lands in the per-ns transcript file, the same way
    // every namespace-resolving verb that flows through
    // `runtime::resolve_target` already does. Pre-fix, the connect
    // command never called `transcript::set_namespace`, so its
    // output never produced a fenced block — leaving an undocumented
    // hole in the F18 contract that surfaced during the release
    // smoke as P8-C ("history show --audit-id <connect_id>: 0
    // blocks match"). The set call must be early enough to cover
    // the `--show` / `--set-env` / `--unset-env` early-return paths
    // too — those are still namespace-scoped invocations.
    crate::transcript::set_namespace(&args.namespace);

    // F12 (v0.1.3): env-overlay management subcommands. These are
    // mutually exclusive with the connect-master path: `--show`,
    // `--set-path`, `--set-env`, `--unset-env` operate on
    // `~/.inspect/servers.toml` and never spawn ssh. `--detect-path`
    // does require an open session and is handled after the master
    // comes up.
    let mutates_overlay =
        args.set_path.is_some() || !args.set_env.is_empty() || !args.unset_env.is_empty();
    if args.show {
        return run_show_overlay(&args);
    }
    if mutates_overlay {
        return run_mutate_overlay(&args);
    }

    let resolved = resolver::resolve(&args.namespace)?;
    resolved.config.validate(&resolved.name)?;
    let target = SshTarget::from_resolved(&resolved)?;

    // L4 (v0.1.3): password-auth namespaces get a 12h ControlPersist
    // default and a 24h cap on operator-supplied --ttl. Per-namespace
    // session_ttl slots between the env override and the auth-mode
    // default.
    let password_auth = resolved.config.auth.as_deref() == Some("password");
    let (ttl, ttl_source) = ssh::ttl::resolve_with_ns(
        args.ttl.as_deref(),
        resolved.config.session_ttl.as_deref(),
        Some(password_auth),
    )?;

    let allow_interactive = !args.non_interactive;
    let passphrase_env = if args.interactive {
        None
    } else {
        resolved.config.key_passphrase_env.as_deref()
    };
    // L4 (v0.1.3): in password mode, --interactive forces the prompt
    // path the same way it forces the passphrase prompt for keys.
    let password_env = if args.interactive {
        None
    } else {
        resolved.config.password_env.as_deref()
    };

    let outcome = ssh::start_master(
        &resolved.name,
        &target,
        &ttl,
        AuthSelection {
            passphrase_env,
            allow_interactive,
            skip_existing_mux_check: args.no_existing_mux,
            password_auth,
            password_env,
            save_to_keychain: args.save_passphrase,
        },
    )
    .with_context(|| format!("connect '{}'", resolved.name))?;

    // P8-C fix (v0.1.3): emit a structured audit entry for the
    // connect itself so the F18 transcript footer carries an
    // `audit_id=<id>` cross-link (matching every other
    // namespace-resolving verb). The entry is also independently
    // useful: it lets `audit grep verb=connect` enumerate every
    // master-spawn event for forensics, and it gives `revert <id>
    // --dry-run` a manual-inverse to print (`inspect disconnect
    // <ns>`). Revert is `Unsupported` because the inverse is itself
    // an `inspect` invocation that the local `revert` runner cannot
    // dispatch on a remote target — the operator has to run it
    // themselves. Failure to write the audit is intentionally
    // best-effort: `inspect connect` is fundamentally about session
    // ergonomics, and a momentary audit-store failure (e.g. lazy GC
    // mid-flight) must not turn a successful connect into a
    // user-visible error.
    if let Ok(store) = AuditStore::open() {
        let mut e = AuditEntry::new("connect", &resolved.name);
        e.args = format!(
            "auth={auth},ttl={ttl},ttl_source={src},interactive={interactive},save={save}",
            auth = outcome.auth_mode.label(),
            ttl = outcome.ttl,
            src = ttl_source.label(),
            interactive = args.interactive,
            save = args.save_passphrase,
        );
        e.revert = Some(Revert::unsupported(format!(
            "inspect disconnect {}",
            resolved.name
        )));
        let _ = store.append(&e);
    }

    // F12 (v0.1.3): `--detect-path` needs the master open before it
    // can ssh. Run it after start_master succeeds; on any failure
    // (probe error, operator declined) we still report the connect
    // as successful and the overlay as unchanged.
    if args.detect_path {
        match run_detect_path(&resolved.name, &target) {
            Ok(detected) => {
                if let Some(msg) = detected {
                    eprintln!("{msg}");
                }
            }
            Err(e) => {
                eprintln!("note: --detect-path failed: {e}");
            }
        }
    }

    if args.format.is_json() {
        // P0.6 sweep (v0.1.3): emit the L7 standard envelope so the
        // connect-cluster matches every other JSON-emitting verb's
        // shape (`{schema_version, summary, data, next, meta}`).
        // Pre-fix this site (along with `connections`, `disconnect`,
        // `disconnect-all`) emitted a flat
        // `{schema_version, namespace, auth, socket, ttl, ttl_source}`
        // shape — an L7-discipline gap surfaced during the SMOKE
        // P1.5 live run on 2026-05-09.
        let socket_value: Value = outcome
            .socket
            .as_ref()
            .map(|p| Value::String(p.display().to_string()))
            .unwrap_or(Value::Null);
        let summary = format!(
            "'{}' connected via {} (ttl {}, {})",
            resolved.name,
            outcome.auth_mode.label(),
            outcome.ttl,
            ttl_source.label()
        );
        let data = json!({
            "namespace": resolved.name,
            "auth": outcome.auth_mode.label(),
            "socket": socket_value,
            "ttl": outcome.ttl,
            "ttl_source": ttl_source.label(),
        });
        let mut doc = OutputDoc::new(summary, data);
        doc.push_next(NextStep::new("inspect connections", "list active masters"));
        doc.push_next(NextStep::new(
            format!("inspect disconnect {}", resolved.name),
            "close this master",
        ));
        return doc.print_json(args.format.select_spec());
    }

    println!(
        "SUMMARY: '{}' connected via {} (ttl {}, {})",
        resolved.name,
        outcome.auth_mode.label(),
        outcome.ttl,
        ttl_source.label()
    );
    println!("DATA:");
    println!("  host:   {}@{}:{}", target.user, target.host, target.port);
    if let Some(sock) = &outcome.socket {
        println!("  socket: {}", sock.display());
    } else {
        println!("  socket: (delegated to user's existing ControlMaster)");
    }
    println!(
        "NEXT:    inspect connections    inspect disconnect {}",
        resolved.name
    );
    Ok(ExitKind::Success)
}

/// F12 (v0.1.3): print the current env overlay for `<ns>` and exit.
/// The configured map is read from `~/.inspect/servers.toml` (the
/// authoritative on-disk source — we deliberately do NOT include
/// env-var overrides, because the spec scopes overlay management to
/// config-file state). Empty overlay renders as the explicit literal
/// `(none configured)` so an absent map is distinguishable from an
/// empty one.
fn run_show_overlay(args: &ConnectArgs) -> anyhow::Result<ExitKind> {
    let namespace = &args.namespace;
    let servers = config_file::load().context("loading servers.toml")?;
    let cfg = servers.namespaces.get(namespace);
    let overlay: BTreeMap<String, String> = cfg.and_then(|c| c.env.clone()).unwrap_or_default();
    if args.format.is_json() {
        // P0.6 sweep (v0.1.3): L7 envelope. Pre-fix this site emitted
        // a flat `{schema_version, namespace, env_overlay}` shape.
        let summary = format!(
            "env overlay for '{}' ({} entr{})",
            namespace,
            overlay.len(),
            if overlay.len() == 1 { "y" } else { "ies" }
        );
        let env_overlay: serde_json::Map<String, Value> = overlay
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect();
        let data = json!({
            "namespace": namespace,
            "env_overlay": Value::Object(env_overlay),
        });
        let mut doc = OutputDoc::new(summary, data);
        doc.push_next(NextStep::new(
            format!("inspect connect {namespace} --set-env KEY=VALUE"),
            "add or update an entry",
        ));
        doc.push_next(NextStep::new(
            format!("inspect connect {namespace} --unset-env KEY"),
            "remove an entry",
        ));
        return doc.print_json(args.format.select_spec());
    }
    println!(
        "SUMMARY: env overlay for '{}' ({} entr{})",
        namespace,
        overlay.len(),
        if overlay.len() == 1 { "y" } else { "ies" }
    );
    println!("DATA:");
    if overlay.is_empty() {
        println!("  (none configured)");
    } else {
        for (k, v) in &overlay {
            println!("  {k}={v}");
        }
    }
    println!(
        "NEXT:    inspect connect {namespace} --set-env KEY=VALUE   inspect connect {namespace} --unset-env KEY",
    );
    Ok(ExitKind::Success)
}

/// F12 (v0.1.3): apply `--set-path` / `--set-env` / `--unset-env`
/// against `~/.inspect/servers.toml` and persist atomically. The
/// namespace must already exist (we never create it implicitly here —
/// use `inspect add` for that, since it requires `host`/`user`).
fn run_mutate_overlay(args: &ConnectArgs) -> anyhow::Result<ExitKind> {
    let mut servers = config_file::load().context("loading servers.toml")?;
    if !servers.namespaces.contains_key(&args.namespace) {
        crate::error::emit(format!(
            "namespace '{}' is not configured. → run 'inspect add {0}' first",
            args.namespace
        ));
        return Ok(ExitKind::Error);
    }

    // Validate every input before touching the on-disk state, so a
    // typo in entry 3 of 5 doesn't leave a half-applied config.
    let mut to_set: Vec<(String, String)> = Vec::new();
    if let Some(path_value) = &args.set_path {
        to_set.push(("PATH".to_string(), path_value.clone()));
    }
    for raw in &args.set_env {
        let (k, v) = crate::exec::env_overlay::parse_kv(raw)?;
        to_set.push((k, v));
    }
    for k in &args.unset_env {
        if !is_valid_env_key(k) {
            crate::error::emit(format!(
                "--unset-env key '{k}' must match [A-Za-z_][A-Za-z0-9_]*"
            ));
            return Ok(ExitKind::Error);
        }
    }

    // Mutate. BTreeMap-insert semantics give us idempotency for free:
    // re-running with the same KEY=VALUE is a no-op; running with a
    // different value overwrites; --unset-env removes a present entry
    // and is silent for an already-absent one.
    let cfg = servers
        .namespaces
        .get_mut(&args.namespace)
        .expect("checked above");
    let map = cfg.env.get_or_insert_with(BTreeMap::new);
    let mut changed = false;
    for (k, v) in &to_set {
        let prev = map.insert(k.clone(), v.clone());
        if prev.as_deref() != Some(v.as_str()) {
            changed = true;
        }
    }
    for k in &args.unset_env {
        if map.remove(k).is_some() {
            changed = true;
        }
    }
    // Tidy up: if the resulting map is empty, drop the field rather
    // than leaving an empty `[namespaces.<ns>.env]` block in TOML.
    if map.is_empty() {
        cfg.env = None;
    }

    if changed {
        config_file::save(&servers).context("writing servers.toml")?;
    }

    if args.format.is_json() {
        // P0.6 sweep (v0.1.3): L7 envelope.
        let summary_bits = {
            let mut bits: Vec<String> = Vec::new();
            if !to_set.is_empty() {
                bits.push(format!("set {}", to_set.len()));
            }
            if !args.unset_env.is_empty() {
                bits.push(format!("unset {}", args.unset_env.len()));
            }
            if bits.is_empty() {
                "no-op".to_string()
            } else {
                bits.join(", ")
            }
        };
        let summary = format!(
            "env overlay for '{}' updated ({}){}",
            args.namespace,
            summary_bits,
            if changed { "" } else { " — already applied" }
        );
        let data = json!({
            "namespace": args.namespace,
            "changed": changed,
            "applied": to_set.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>(),
            "unset": args.unset_env.clone(),
        });
        let mut doc = OutputDoc::new(summary, data);
        doc.push_next(NextStep::new(
            format!("inspect connect {} --show", args.namespace),
            "review the resulting overlay",
        ));
        return doc.print_json(args.format.select_spec());
    }
    let mut summary_bits: Vec<String> = Vec::new();
    if !to_set.is_empty() {
        summary_bits.push(format!("set {}", to_set.len()));
    }
    if !args.unset_env.is_empty() {
        summary_bits.push(format!("unset {}", args.unset_env.len()));
    }
    println!(
        "SUMMARY: env overlay for '{}' updated ({}){}",
        args.namespace,
        if summary_bits.is_empty() {
            "no-op".to_string()
        } else {
            summary_bits.join(", ")
        },
        if changed { "" } else { " — already applied" }
    );
    println!("NEXT:    inspect connect {} --show", args.namespace);
    Ok(ExitKind::Success)
}

/// F12 (v0.1.3): probe the remote login PATH vs. the non-login PATH
/// and, when they differ, prompt the operator (tty only) to pin the
/// merged value into `[namespaces.<ns>.env].PATH`. Non-tty invocation
/// auto-declines: never write config without explicit confirmation.
///
/// Returns `Ok(Some(message))` to show on success (whether the
/// overlay was updated or not), `Ok(None)` when the PATHs match
/// (nothing to do), or `Err(_)` on probe failure.
fn run_detect_path(namespace: &str, target: &SshTarget) -> anyhow::Result<Option<String>> {
    use crate::ssh::exec::{run_remote, RunOpts};
    // Both probes go through the same master we just opened.
    let login = run_remote(
        namespace,
        target,
        "bash -lc 'printf %s \"$PATH\"'",
        RunOpts::with_timeout(15),
    )
    .context("probing remote login PATH")?;
    let nonlogin = run_remote(
        namespace,
        target,
        "printf %s \"$PATH\"",
        RunOpts::with_timeout(15),
    )
    .context("probing remote non-login PATH")?;
    if !login.ok() || !nonlogin.ok() {
        return Err(anyhow::anyhow!(
            "remote PATH probe failed (login exit {}, nonlogin exit {})",
            login.exit_code,
            nonlogin.exit_code
        ));
    }
    let login_path = login.stdout.trim().to_string();
    let nonlogin_path = nonlogin.stdout.trim().to_string();
    if login_path == nonlogin_path || login_path.is_empty() {
        return Ok(Some(
            "note: --detect-path: remote login and non-login PATH match; nothing to pin"
                .to_string(),
        ));
    }
    let added: Vec<&str> = login_path
        .split(':')
        .filter(|seg| !seg.is_empty() && !nonlogin_path.split(':').any(|s| s == *seg))
        .collect();
    let summary = if added.is_empty() {
        "note: --detect-path: remote login PATH differs but adds no new entries (re-orders only); not pinning"
            .to_string()
    } else {
        format!(
            "note: --detect-path: remote login PATH adds: {} — pin these for {}? [y/N]",
            added.join(", "),
            namespace
        )
    };
    if added.is_empty() {
        return Ok(Some(summary));
    }
    if !is_local_stdin_tty() {
        return Ok(Some(format!(
            "{summary}\nnote: stdin is not a tty; auto-declining (no config changes). \
             Re-run interactively or pass --set-path to apply."
        )));
    }
    eprint!("{summary} ");
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .context("reading detect-path confirmation")?;
    if !matches!(answer.trim(), "y" | "Y" | "yes" | "YES") {
        return Ok(Some("note: --detect-path declined by operator".to_string()));
    }
    // Operator said yes: write the merged login PATH (which already
    // contains the non-login entries plus the additions) as the
    // overlay value. We do NOT use a relative diff because the order
    // of entries matters and the login order is what the operator
    // saw.
    let mut servers = config_file::load().context("loading servers.toml")?;
    let cfg = servers.namespaces.entry(namespace.to_string()).or_default();
    let map = cfg.env.get_or_insert_with(BTreeMap::new);
    map.insert("PATH".to_string(), login_path.clone());
    config_file::save(&servers).context("writing servers.toml")?;
    Ok(Some(format!(
        "note: --detect-path: pinned PATH={} for {}",
        login_path, namespace
    )))
}

fn is_local_stdin_tty() -> bool {
    #[cfg(unix)]
    {
        // Safety: STDIN_FILENO (0) is a hosted-process invariant.
        unsafe { libc::isatty(0) == 1 }
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// F13 (v0.1.3): re-establish the persistent master socket for an
/// already-resolved namespace. Called by the dispatch wrapper when a
/// transport-stale failure is detected. Honors the same auth path as
/// interactive `inspect connect <ns>` — passphrase from
/// `key_passphrase_env` when set; otherwise interactive prompt when
/// stdin is a tty; otherwise refuses and returns Err so the caller
/// can surface `Transport::AuthFailed` exit code 14.
pub fn reauth_namespace(namespace: &str) -> anyhow::Result<()> {
    let resolved = resolver::resolve(namespace)?;
    resolved.config.validate(&resolved.name)?;
    let target = SshTarget::from_resolved(&resolved)?;
    let password_auth = resolved.config.auth.as_deref() == Some("password");
    let (ttl, _ttl_source) = ssh::ttl::resolve_with_ns(
        None,
        resolved.config.session_ttl.as_deref(),
        Some(password_auth),
    )?;
    let allow_interactive = is_local_stdin_tty();
    let passphrase_env = resolved.config.key_passphrase_env.as_deref();
    let password_env = resolved.config.password_env.as_deref();
    // Tear down whatever is left of the dead master so start_master
    // re-opens a fresh ControlPersist channel.
    let socket = ssh::socket_path(&resolved.name);
    let _ = ssh::exit_master(&socket, &target);
    ssh::start_master(
        &resolved.name,
        &target,
        &ttl,
        AuthSelection {
            passphrase_env,
            allow_interactive,
            skip_existing_mux_check: false,
            password_auth,
            password_env,
            // F13 reauth never saves: the original `inspect connect`
            // already chose whether to save (or not), and silently
            // re-saving on every reauth would be surprising.
            save_to_keychain: false,
        },
    )
    .map(|outcome| {
        // F13 (v0.1.3, smoke-driven): operator notice on successful
        // reauth. Without this, the only feedback after the prompt
        // is the verb's normal output — easy to miss that the
        // session was actually re-established. The TTL echoes the
        // ControlPersist budget so operators know how long the
        // recovered session will last.
        eprintln!(
            "note: session for '{}' re-established (ttl {})",
            resolved.name, outcome.ttl,
        );
    })
    .with_context(|| format!("reauth '{}'", resolved.name))
}
