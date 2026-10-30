#!/usr/bin/env bash
# Run a test on valencia in an isolated copy of the repo, never on the laptop
# and never in ~/hackathon (that tree backs the live flood-api service).
#
#   code/testrun.sh <name> <command...>      e.g. code/testrun.sh s1 .venv/bin/python code/evaluate.py
#   code/testrun.sh --status                 who holds which slot, free RAM
#   code/testrun.sh --stop <name>            kill a running job (e.g. before restarting a server)
#   code/testrun.sh --pull <name> <dir>      copy remote code/outputs back to a local dir
#
# Limits: 2 slots at once (flock), 5 GB RAM + 3 cores per job (cgroup), 2 h timeout (TESTRUN_TIMEOUT=12h to override).
# TEST_PORT=8081/8082 (by slot) is exported for jobs that start a server.
set -euo pipefail

HOST=valencia
BASE=hackathon-test
LIVE=hackathon
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

if [[ "${1:-}" == "--status" ]]; then
  ssh "$HOST" "free -h | sed -n 2p; for s in 1 2; do f=~/$BASE/.slot\$s; printf 'slot%s: ' \$s; flock -n \$f true 2>/dev/null && echo free || cat \$f.who; done"
  exit 0
fi
if [[ "${1:-}" == "--stop" ]]; then
  ssh "$HOST" "pkill -f '$BASE/$2/' || true"
  exit 0
fi
if [[ "${1:-}" == "--pull" ]]; then
  rsync -a "$HOST:$BASE/$2/code/outputs/" "$3/"
  exit 0
fi

name="$1"; shift
[[ "$name" =~ ^[A-Za-z0-9_-]+$ ]] || { echo "bad name: $name" >&2; exit 2; }
dest="$BASE/$name"

ssh "$HOST" "mkdir -p ~/$dest && ln -sfn ~/$LIVE/working_data ~/$dest/working_data && ln -sfn ~/$LIVE/.venv ~/$dest/.venv"
rsync -a --delete \
  --exclude .git --exclude .venv --exclude working_data --exclude 'flood_api/target' \
  --exclude 'code/outputs' --exclude __pycache__ --exclude '*.zip' \
  --exclude AI-Challenge --exclude policy_api --exclude docs \
  "$ROOT/" "$HOST:$dest/"

cmd=$(printf '%q ' "$@")
ssh "$HOST" "cd ~/$dest && for s in 1 2; do
    exec 9>~/$BASE/.slot\$s
    if flock -n 9; then
      echo '$name $(date +%H:%M)' > ~/$BASE/.slot\$s.who
      export TEST_PORT=\$((8080 + s)) CARGO_TARGET_DIR=~/$dest/flood_api/target
      systemd-run --user --scope -q -p MemoryMax=5G -p MemorySwapMax=0 -p CPUQuota=300% \
        timeout ${TESTRUN_TIMEOUT:-2h} nice -n 10 $cmd; rc=\$?
      : > ~/$BASE/.slot\$s.who; exit \$rc
    fi
  done
  echo 'both slots busy, run: code/testrun.sh --status' >&2; exit 75"
