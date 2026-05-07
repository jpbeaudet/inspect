EXAMPLES — Worked examples and translation guide

TRANSLATION GUIDE (you know X — here's inspect)

  grep -i "error" file.log
    $ inspect grep "error" arte/atlas:/var/log/atlas.log -i

  stern --since 30m pulse
    $ inspect logs arte/pulse --since 30m

  kubectl logs <pod> --since=30m | grep -i error
    $ inspect grep "error" arte/pulse --since 30m -i

  ssh box "docker logs pulse --since 30m | grep error"
    $ inspect grep "error" arte/pulse --since 30m

  ssh box "sudo sed -i 's/old/new/' /etc/foo.conf"
    $ inspect edit arte/_:/etc/foo.conf 's/old/new/' --apply

  scp ./file.conf box:/etc/file.conf
    $ inspect cp ./file.conf arte/_:/etc/file.conf --apply

  ssh box "docker restart pulse"
    $ inspect restart arte/pulse --apply

  Loki LogQL: {job="varlogs"} |= "error"
    $ inspect search '{server="arte", source="logs"} |= "error"'

WORKFLOW EXAMPLES

  # Find errors and restart affected services
  $ inspect search '{source="logs"} |= "OOM"' --since 5m --json \
      --select '.service' --select-raw | sort -u \
      | xargs -I{} inspect restart arte/{} --apply

  # Push a config fix across all prod atlas instances (preview, then apply)
  $ inspect edit '*/atlas:/etc/atlas.conf' 's|timeout=30|timeout=60|'
  $ inspect edit '*/atlas:/etc/atlas.conf' 's|timeout=30|timeout=60|' --apply

  # Mixed sources: same pattern in logs AND a config file
  $ inspect search '{server="arte", service="pulse", source="logs"} or {server="arte", service="atlas", source="file:/etc/atlas.conf"} |= "milvus"' --since 30m

  # Error rate per service (metric query)
  $ inspect search 'sum by (service) (count_over_time({server="arte", source="logs"} |= "error" [5m]))'

  # Export fleet status as Markdown for a GitHub issue
  $ inspect fleet --ns 'prod-*' status --md

  # Pipe to fzf for interactive service selection
  $ inspect ps arte --format '{{.service}}'

SEE ALSO
  inspect help quickstart    getting started
  inspect help search        LogQL syntax reference
  inspect help write         write verb examples
  inspect help formats       output format options
  inspect help selectors     selector grammar
