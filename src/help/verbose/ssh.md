
VERBOSE — SSH edge cases

CONTROL-SOCKET LIMITS
  OpenSSH refuses new multiplexed sessions when the master's
  MaxSessions threshold (sshd_config, default 10) is reached.
  inspect surfaces this as:

    error: ssh max sessions exceeded for arte
    see: inspect help ssh

  Resolution:
    1. Drop idle sessions:        inspect disconnect arte && inspect connect arte
    2. Lower fan-out concurrency: inspect fleet --concurrency 4 …
    3. Raise MaxSessions on the host (requires sshd reload).

  Default fleet concurrency is 8; on hosts with the OpenSSH default
  this leaves room for two ad-hoc sessions before contention.

CONTROLPERSIST AND TTL
  Persistent sockets are torn down by ssh itself when ControlPersist
  expires. inspect computes ControlPersist from `persist_ttl`:
    - default: 30m everywhere except Codespaces
    - Codespaces: 4h (auto-detected via $CODESPACES)
    - override:  set persist_ttl in ~/.inspect/servers.toml

  Stale sockets (master process gone but socket file present) are
  detected by `ssh -O check`; inspect removes them transparently on
  the next command.

PASSPHRASE-CACHING SAFETY
  inspect never writes passphrases to disk. The interactive prompt
  uses rpassword, which disables echo and zeroes the buffer on
  drop. CI flows must use either INSPECT_<NS>_KEY_PASSPHRASE_ENV
  (pointing at a separate env var name) or an unencrypted key — the
  latter only inside ephemeral runners.

  INSPECT_<NS>_KEY_INLINE accepts a base64-encoded private key for
  pure-env CI. inspect writes it to a tmpfs file at mode 600,
  consumes it, and deletes it. It is *never* persisted.

KNOWN-HOSTS POLICY
  inspect uses ~/.ssh/known_hosts. Unknown host keys cause an
  immediate connection failure — there is no `--accept-new` flag.
  Onboard hosts with: ssh-keyscan host >> ~/.ssh/known_hosts before
  the first `inspect connect`.

DEBUGGING A FAILED CONNECT
  $ inspect test arte --verbose      # full ssh diagnostic
  $ ssh -vvv -F ~/.inspect/ssh_config arte   # raw ssh transcript
  $ inspect connections              # any stuck sockets?

SEE ALSO
  inspect help ssh           the standard topic body
  inspect help fleet         concurrency vs MaxSessions
  inspect help discovery     what runs after a successful connect
