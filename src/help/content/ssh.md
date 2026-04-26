SSH — Persistent connections, ControlMaster, passphrases

EXAMPLES
  $ inspect connect arte                     # one passphrase for the session
  $ inspect connections                      # list active sessions
  $ inspect disconnect arte                  # close one
  $ inspect disconnect-all                   # close all

DESCRIPTION
  inspect uses OpenSSH ControlMaster multiplexing. The first
  connection prompts for a passphrase (if the key is encrypted);
  subsequent commands reuse the session via a control socket.

CREDENTIAL RESOLUTION (in order)
  1. Existing inspect-managed control socket (alive)  reuse
  2. User's ~/.ssh/config ControlMaster (alive)       reuse
  3. ssh-agent with key loaded                        use
  4. INSPECT_<NS>_KEY_PASSPHRASE_ENV set              read from env
  5. Interactive prompt (rpassword)

CONFIGURATION
  Environment variables (primary):
    INSPECT_<NS>_HOST, _USER, _KEY_PATH, _PORT
    INSPECT_<NS>_KEY_PASSPHRASE_ENV
    INSPECT_<NS>_KEY_INLINE (base64, CI only)

  Config file: ~/.inspect/servers.toml (mode 600)

  Per-server: persist = true (default),
              persist_ttl = "4h" on Codespaces / "30m" otherwise.

CONTROL SOCKETS
  Location:  ~/.inspect/sockets/<ns>.sock (mode 600)
  Lifecycle: created on connect, removed on disconnect or TTL expiry
  Stale sockets are auto-detected and cleaned up on the next
  command.

SECURITY
  Passphrases are never written to disk. Keys are never inlined on
  disk (env only). No password auth. No auto-trust of unknown host
  keys. Socket mode 600. Sockets are never shared across users.

SEE ALSO
  inspect help quickstart    first-time setup
  inspect help discovery     what runs after a successful connect
  inspect help fleet         multi-server connection management
