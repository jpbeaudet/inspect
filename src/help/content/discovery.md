DISCOVERY — Auto-discovery, profiles, drift detection

EXAMPLES
  $ inspect setup arte                       # full discovery scan
  $ inspect status arte                      # uses cached profile
  $ inspect profile arte                     # print the cached profile

DESCRIPTION
  `inspect setup <ns>` connects to the server, scans everything
  running, and produces a profile cached at
  ~/.inspect/profiles/<ns>.yaml. The profile feeds selector
  resolution, group expansion, and the connectivity matrix.

  Discovery scans: docker ps/inspect, volumes, networks, images,
  listening ports (TCP via ss -tlnp / netstat -tlnp; UDP via
  ss -ulnp / netstat -ulnp — L9, v0.1.3), systemd units, health
  endpoints, log driver configuration, and remote tooling
  (rg, jq, sed).

UDP LISTENERS (L9, v0.1.3)
  The host-listener probe scans both TCP and UDP. Pre-L9 only TCP
  was surfaced, so DNS forwarders, mDNS responders, syslog
  receivers (`:514/udp`), IPSec daemons, and WireGuard endpoints
  were invisible to `inspect ports` and `inspect status`. v0.1.3
  fixes that — every host listener record carries an explicit
  `proto: tcp|udp`. Filter with `inspect ports <sel> --proto udp`
  (or `--proto tcp`); default `all` shows both. UDP listeners
  shown by `ss -uln` are *bound sockets*, not "the service is
  actually receiving traffic" — operators chasing dead UDP
  services still need a real probe (e.g., `dig @host` for DNS).

DRIFT DETECTION
  Every command runs an async drift check in the background. If the
  running container set differs from the cached profile, a warning
  appears on stderr (it never blocks the foreground command). Run
  `inspect setup <ns>` to refresh.

REFRESH
  Full re-discovery only on explicit `inspect setup <ns>` or when
  the cache TTL expires (default 7 days). Local edits to the profile
  (groups, aliases) are preserved across re-discovery.

REMOTE TOOLING
  Discovery probes for rg, jq, journalctl, and sed on the remote.
  This determines filter pushdown strategy:
    rg available    fast remote regex filtering
    grep only       slower fallback (with hint to install rg)
    journalctl      used for containers with the journald log driver
    sed             used for remote in-place edits

SEE ALSO
  inspect help ssh           connection and credential management
  inspect help selectors     how discovery feeds service resolution
  inspect help quickstart    first-time setup
