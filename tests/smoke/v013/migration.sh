#!/usr/bin/env bash
# v0.1.3 smoke F14 / L11 script-mode payload.
#
# Exercises the cross-layer-quoting hell that F14 was built to kill:
#   - shell -> ssh -> bash -s -> docker exec -i
#   - embedded `sh -c "..."` heredoc (the psql/cypher-shell shape)
#   - embedded `python3 -c "..."` block (the python-c shape)
#
# The script is shipped as the body of `inspect run --file` and must
# never be re-quoted on the local side. Sandbox container name is
# passed as $1 via the `inspect run -- <ctr>` positional arg.
#
# This script is invoked by docs/SMOKE_v0.1.3.md and is committed for
# reuse in v0.1.4+ smoke runs.

set -euo pipefail

CTR="${1:?usage: migration.sh <sandbox-container-name>}"

echo "::: smoke F14: starting against ${CTR}"
echo "::: smoke F14: phase 1 -- direct docker exec (no quoting layer)"
docker exec "${CTR}" sh -c 'echo "phase1: hello from inside the sandbox"'

echo "::: smoke F14: phase 2 -- embedded sh -c heredoc (psql shape)"
docker exec -i "${CTR}" sh <<'NESTED_SH'
echo "phase2: nested sh heredoc executing"
sh -c "echo 'phase2: nested sh -c with single quotes inside double works: \$HOME'"
echo "phase2: backticks `date -u +%Y` and \$dollar literal both intact"
NESTED_SH

echo "::: smoke F14: phase 3 -- embedded python3 -c (alpine: install first)"
if docker exec "${CTR}" sh -c 'command -v python3 >/dev/null 2>&1'; then
    docker exec "${CTR}" python3 -c "
import sys
print('phase3: python3 -c block running, args=' + repr(sys.argv))
print('phase3: nested-quote ok:  ' + \"'single inside double'\")
"
else
    echo "phase3: python3 not present in nginx:alpine -- skipping"
    echo "phase3: (the F14 contract is that the BODY survives the quoting"
    echo "phase3:  layers; whether the remote interpreter exists is a"
    echo "phase3:  separate concern. The 'sh' phases above prove the body"
    echo "phase3:  arrived intact.)"
fi

echo "::: smoke F14: phase 4 -- multi-line backslash-continuation"
docker exec "${CTR}" sh -c "echo phase4: backslash \
                                  continuation \
                                  survived"

echo "::: smoke F14: phase 5 -- exit code passthrough"
docker exec "${CTR}" sh -c 'exit 0'

echo "::: smoke F14: complete -- script body survived all layers"
