SSH — Persistent connections, ControlMaster, passphrases, password auth (L4)

EXAMPLES
  $ inspect connect arte                     # one passphrase for the session
  $ inspect connect legacy-box               # password auth (L4) — one prompt, 12h reuse
  $ inspect connections                      # list active sessions (auth, ttl, expires_in)
  $ inspect ssh add-key legacy-box --apply   # migrate off password to keys
  $ inspect disconnect arte                  # close one
  $ inspect disconnect-all                   # close all

DESCRIPTION
  inspect uses OpenSSH ControlMaster multiplexing. The first
  connection prompts for a passphrase or password (if the key is
  encrypted, or if the namespace uses password auth); subsequent
  commands reuse the session via a control socket.

CREDENTIAL RESOLUTION (in order)
  Key auth (default, `auth = "key"` or unset):
    1. Existing inspect-managed control socket (alive)   reuse
    2. User's ~/.ssh/config ControlMaster (alive)        reuse
    3. ssh-agent with key loaded                         use
    4. key_passphrase_env (servers.toml) set             read from env
    5. Interactive prompt (rpassword)

  Password auth (L4, `auth = "password"`):
    1. Existing inspect-managed control socket (alive)   reuse
    2. User's ~/.ssh/config ControlMaster (alive)        reuse
    3. password_env (servers.toml) set                   read from env
    4. Interactive prompt — up to 3 attempts then abort with
       a chained hint to `inspect help ssh`.
    Key auth is force-disabled at the ssh layer
    (PubkeyAuthentication=no) so an agent-loaded key cannot
    pre-empt the operator's intent to authenticate by password.

CONFIGURATION
  Environment variables (primary):
    INSPECT_<NS>_HOST, _USER, _KEY_PATH, _PORT
    INSPECT_<NS>_KEY_PASSPHRASE_ENV
    INSPECT_<NS>_KEY_INLINE (base64, CI only)

  Config file: ~/.inspect/servers.toml (mode 600)

  Per-server fields (servers.toml):
    auth          = "key" (default) | "password"
    password_env  = "VAR_NAME"      # password mode only; never the value itself
    session_ttl   = "12h"           # ControlPersist override; capped at 24h
    key_path      = "..."           # key auth
    key_passphrase_env = "VAR_NAME" # key auth

  Per-server defaults: persist = true (always),
                       persist_ttl = "30m" local / "4h" Codespaces (key auth)
                       session_ttl = "12h" (password auth, L4)

  L4 cap: when `auth = "password"`, any TTL longer than 24h is
  rejected — including operator-supplied `--ttl 48h legacy-box`.
  The cap exists so a forgotten laptop does not hold a live remote
  session indefinitely.

CONTROL SOCKETS
  Location:  ~/.inspect/sockets/<ns>.sock (mode 600)
  Lifecycle: created on connect, removed on disconnect or TTL expiry
  Stale sockets are auto-detected and cleaned up on the next
  command.

PASSWORD AUTH (L4, v0.1.3)
  Operator path on a legacy or locked-down host that does not
  accept keys:

    1. Add the namespace with `auth = "password"` and (optionally)
       `password_env = "<VAR>"` in ~/.inspect/servers.toml. Set
       `session_ttl = "12h"` (or up to "24h") if you want longer
       than the default.
    2. Run `inspect connect <ns>`. inspect prompts once (or reads
       the env var) and opens a persistent ControlMaster.
    3. Every subsequent verb rides the master without re-prompting
       until TTL expiry or `inspect disconnect <ns>`.
    4. When ready, run `inspect ssh add-key <ns> --apply` over the
       open session. The verb generates an ed25519 keypair (or
       takes one via `--key`), installs the public half on the
       remote `authorized_keys` (idempotent), and offers to flip
       the namespace to `auth = "key"` so future connects skip the
       password.

  inspect emits a one-time warning on the first password connect
  per namespace: `password auth is less secure than key-based`.
  The marker `~/.inspect/.password_warned/<ns>` is touched after
  the warning fires; running `ssh add-key` to flip the namespace
  off password auth clears the marker so re-onboarding re-warns.

INSPECT SSH ADD-KEY (L4, v0.1.3)
  Audited write verb. Requires `--apply` to perform; without it,
  prints a deterministic dry-run preview.

  Flags:
    --key <path>           reuse an existing key instead of generating
    --no-rewrite-config    skip the auth-flip prompt
    --apply                perform the install + audit-log entry
    --reason <text>        attached to the audit entry (≤240 chars)

  Audit shape:
    verb=ssh.add-key, target=<ns>, args="[key_path=...] \
    [generated=true|false] [installed=true] \
    [config_rewritten=true|false]"
    revert.kind=command_pair (manual remove from authorized_keys)

CREDENTIAL LIFETIME (L2, v0.1.3)
  Three options for how long a credential survives. The default
  is recommended for almost everyone; the keychain path is the
  explicit opt-in for operators who want survival across reboots.

  (1) DEFAULT — ssh-agent + ControlMaster (one prompt per shell session)
      The first `inspect connect <ns>` prompts; subsequent verbs
      ride the ControlMaster socket without re-prompting until
      the configured TTL expires or the operator runs `inspect
      disconnect <ns>`. Logout / reboot clears the agent; the
      next session prompts once again. Most operators want this.

  (2) `--save-passphrase` (or `--save-password`) — OS KEYCHAIN
      `inspect connect <ns> --save-passphrase` prompts once,
      opens the master, AND saves the credential to the OS
      keychain under service `inspect-cli`, account `<ns>`.
      Subsequent `inspect connect <ns>` in fresh shell sessions
      auto-retrieve from the keychain — but only for namespaces
      previously saved (no implicit cross-namespace lookup).
      Survives reboots. Backed by macOS Keychain Services /
      Windows Credential Manager / Linux Secret Service (GNOME
      Keyring / KDE Wallet).

      Headless / CI hosts where the keychain backend is
      unavailable warn once and fall back to per-session prompt;
      `inspect keychain test` is the explicit probe.

      Manage with: `inspect keychain list / remove <ns> / test`.

  (3) `key_passphrase_env` / `password_env` — ENV VAR
      For CI / scripted use: configure the namespace's
      `key_passphrase_env` (key auth) or `password_env`
      (password auth) in `~/.inspect/servers.toml` and export
      the variable. The credential is read at connect time.
      Inspect never copies the value or writes it to disk.

  RESOLUTION ORDER per credential type:
    Key auth:      socket → user mux → ssh-agent → key_passphrase_env
                   → OS keychain → interactive prompt
    Password auth: socket → user mux → password_env → OS keychain
                   → interactive prompt (3 attempts)

SECURITY
  Passwords and passphrases are never written to inspect's own
  files; the only persistent path is the OS keychain (option 2
  above), and only when explicitly opted into. Keys are never
  inlined on disk (env only). No auto-trust of unknown host
  keys. Socket mode 600. Sockets are never shared across users.
  Password auth uses the same SSH_ASKPASS pipeline as passphrase
  delivery — the secret stays in process memory and is wiped
  immediately after the ssh master starts.

SEE ALSO
  inspect help quickstart    first-time setup
  inspect help discovery     what runs after a successful connect
  inspect help fleet         multi-server connection management
  inspect help safety        audit log + revert contract
