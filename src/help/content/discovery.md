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
  (rg, sed). `jq` is also probed but is **optional** from v0.1.3
  onward — every recipe in this manual works via `--select` on
  the inspect binary itself (see `inspect help select`).

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

  v0.1.2 (B4) introduced a structured `DriftDiff` carrying
  containers added / removed / image-changed.

  L10 (v0.1.3) extends `DriftDiff` with a `port_changes` array.
  Four kinds:

    added     — port present in live, absent in cached
    removed   — port present in cached, absent in live
    bind      — same (container_port, proto), different host bind
                (e.g. 5432:5432 → 5433:5432 to dodge a collision)
    proto     — same (host, container_port), different proto
                (e.g. a DNS service flipped from /tcp to /udp)

  The cheap probe captures `{{.Ports}}` per container in the same
  ssh round-trip; the parser in `discovery::ports_parse` handles
  IPv4 + IPv6 binds, ranges (`8000-8002->8000-8002/tcp` expands to
  3 records), unbound exposed ports (`5432/tcp` records `host=0`),
  and comma-separated lists. Container-level adds / removes do
  NOT also fan their per-port deltas into `port_changes` — that
  would double-count the operator's intent.

  `inspect setup --check-drift` text output gains a port block:

    ⚓2 port-level changes:
      db   bind  (5432:5432/tcp → 5433:5432/tcp)
      dns  proto (53:53/tcp → 53:53/udp)

  `--json` envelope gains `port_changes: [{container, kind,
  before, after}]` so agents branch on `kind` without re-parsing
  the human form.

REFRESH
  Full re-discovery only on explicit `inspect setup <ns>` or when
  the cache TTL expires (default 7 days). Local edits to the profile
  (groups, aliases) are preserved across re-discovery.

REMOTE TOOLING
  Discovery probes for rg, jq (optional, F19), journalctl, and sed
  on the remote. This determines filter pushdown strategy:
    rg available    fast remote regex filtering
    grep only       slower fallback (with hint to install rg)
    journalctl      used for containers with the journald log driver
    sed             used for remote in-place edits
    jq              informational only — inspect's own `--select`
                    flag (F19) covers every recipe in this manual

SEE ALSO
  inspect help ssh           connection and credential management
  inspect help selectors     how discovery feeds service resolution
  inspect help quickstart    first-time setup
