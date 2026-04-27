# watch — block until a condition holds on a single target

`inspect watch` polls a target until a predicate matches or the
deadline expires. It always operates on a single resolved target —
multi-target waits belong in `inspect bundle`.

## Predicate kinds

Exactly one is required:

| flag                     | meaning                                                            |
| ------------------------ | ------------------------------------------------------------------ |
| `--until-cmd '<cmd>'`    | Run `<cmd>` on the target. Compare its stdout / exit code.         |
| `--until-log '<pat>'`    | Match `<pat>` against `docker logs --since <watch-start>`.         |
| `--until-sql '<sql>'`    | Run `psql -tAc <sql>` inside the target container; truthy = match. |
| `--until-http '<url>'`   | `curl <url>` from the target host. Match optional via `--match`.   |

## `--until-cmd` comparators

Without a comparator, exit code 0 is treated as a match.

| flag                  | match when                                                    |
| --------------------- | ------------------------------------------------------------- |
| `--equals VALUE`      | trimmed stdout equals `VALUE` literally                       |
| `--matches REGEX`     | trimmed stdout matches `REGEX` (extended)                     |
| `--gt N` / `--lt N`   | trimmed stdout, parsed as f64, > / < N                        |
| `--changes`           | first poll where stdout differs from the previous poll        |
| `--stable-for DUR`    | stdout has been the same for at least DUR (e.g. `30s`, `5m`)  |

## `--until-http --match` DSL

```text
status == 200
status != 200
status < 500
body contains "ready"
$.replication.lag < 5
```

`lhs` is one of `status`, `body`, or a JSON path (`$.foo.bar.0`).
`op` is one of `==`, `!=`, `<`, `>`, `contains`. Numeric comparison
when both sides parse as f64; otherwise lexicographic.

## Examples

```bash
# Wait for replica lag to drop under 100 rows.
inspect watch arte/replica \
  --until-sql 'select pg_stat_replication.replay_lag < interval ''1s''' \
  --psql-opts '-U postgres -d app' \
  --timeout 5m --reason 'INC-1234 cutover'

# Wait for a JSON config refresh to land in the running config.
inspect watch arte/api \
  --until-cmd 'cat /etc/api.cfg | grep ^version=' \
  --equals 'version=1.4.2' \
  --interval 5s --timeout 2m

# Wait for a health endpoint to return 200, then exit.
inspect watch arte/api --until-http http://localhost/health --match 'status == 200'

# Same against a self-signed staging endpoint.
inspect watch staging/api --until-http https://10.0.0.5/health \
  --insecure --match 'status == 200' --timeout 2m

# Wait for a deploy to settle (output unchanged for 30 seconds).
inspect watch arte/api --until-cmd 'docker ps --format "{{.Status}}" -f name=api' \
  --stable-for 30s --timeout 5m
```

## Exit codes

* `0` — predicate matched. Last value is echoed to stdout.
* `124` — `--timeout` reached without a match.
* `130` — interrupted (Ctrl-C).
* `2` — error (selector resolution, parse, runner failure, ...).

## Status line

By default `watch` rewrites a single status line in place when
stderr is a TTY. Use `--verbose` (or pipe stderr) for one line per
poll — useful for logs you want to keep.

## Audit

Each watch invocation writes one audit entry: `verb=watch`, `args=<predicate-label>`,
`exit=<exit-code>`, `duration_ms=<wall-time>`. Use `--reason` for free-form
context. Inside an `inspect bundle apply`, watch entries are tagged
with the bundle's `bundle_id` for easy filtering.

## See also

* `inspect help selectors` — how the target is resolved.
* `inspect help formats` — output formats for the matched value.
* `inspect help bundle` — for sequences that include waits.
