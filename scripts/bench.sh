#!/usr/bin/env bash
# Runs the whole benchmark harness locally (bench/README.md): builds Genea,
# the harness and the start floor in release mode, then runs every scenario
# and checks the Genea budgets. Exits non-zero if a budget is missed.
#
#   scripts/bench.sh                 # the full run, before a release or a Slint pin bump
#   scripts/bench.sh --quick         # a smoke test
#   scripts/bench.sh --only typing   # one scenario; see --help for the rest
#
# Needs the Typical workspace (bench/workspaces/typical/setup.sh). Cold
# starts run `sudo -n purge`, so this asks for your password once, up front;
# without it they are skipped. Leave the machine alone while it runs and keep
# Genea's windows in front: input and occlusion disturb the measurements.
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
cd "$repo"

if [[ ! -d bench/workspaces/out/typical ]]; then
  echo "error: no Typical workspace; run bench/workspaces/typical/setup.sh first" >&2
  exit 2
fi

cargo build --release --bin genea --bin genea-bench --bin genea-floor --bin genea-fake-lsp

if [[ " $* " != *" --no-cold "* && " $* " != *" --help "* ]] && sudo -v; then
  # Keep sudo's timestamp fresh for the whole run.
  ( while true; do sudo -n true; sleep 50; done ) &
  keep_alive=$!
  trap 'kill "$keep_alive" 2>/dev/null' EXIT
fi

# No display or idle sleep while measuring.
caffeinate -u -d -i target/release/genea-bench "$@"
