SELECTORS — How to address servers, services, and files

EXAMPLES
  $ inspect status arte                          # all services on server 'arte'
  $ inspect logs arte/pulse                      # one service
  $ inspect ps arte/pulse,atlas                  # two services
  $ inspect status arte/storage                  # a named group from the profile
  $ inspect status 'prod-*/storage'              # storage group on every prod-* server
  $ inspect cat arte/atlas:/etc/atlas.conf       # a file inside a container
  $ inspect cat arte/_:/var/log/syslog           # a host-level file (no container)
  $ inspect logs @plogs                          # a saved alias (see: inspect help aliases)

DESCRIPTION
  Selectors address one or more targets (server, service, file). Every
  read and write verb takes a selector as its primary argument. The
  same grammar drives `inspect search` label matchers, just spelled
  with `{label="value"}`.

GRAMMAR
  <selector> ::= <server>[/<service>][:<path>]  |  @<alias>

  server:   name | name,name | 'glob-*' | all | '~exclude'
  service:  name | name,name | 'glob-*' | '/regex/' | group | '*' | '~exclude' | _
  path:     /path/to/file | '/path/*.glob'

  _ means "host-level" — for ports, host files, and systemd units.

RESOLUTION ORDER
  1. Container short name (pulse, atlas)
  2. Profile aliases (db -> postgres)
  3. Profile groups (storage -> [postgres, milvus, redis, minio])
  4. Globs and regex
  5. Subtractive (~name)

  If a name matches both a service and a group, the service wins
  (with a warning).

EMPTY RESOLUTION
  If a selector matches nothing, inspect lists available servers,
  services, groups, and aliases. Never a silent no-op.

QUOTING
  Globs and regex must be single-quoted to keep the shell from
  expanding them: 'prod-*', '/^atlas-/'. The colon path separator
  does not need quoting unless the path itself has special chars.

SEE ALSO
  inspect help aliases       save and reuse selectors
  inspect help search        selectors inside LogQL queries
  inspect help fleet         multi-server selectors
  inspect help examples      worked selector recipes
