#!/bin/sh
# PROTOTYPE (#18): cold-start experiments. Each launch follows `sudo purge`, so the
# binary, fonts and caches come from disk. Needs sudo in this terminal; takes ~6 min.
# Don't touch the machine while it runs. Results append to exp/results.jsonl.
set -e
cd "$(dirname "$0")/.."
sudo -v
( while true; do sudo -n true; sleep 50; done ) & KEEP=$!
trap 'kill $KEEP' EXIT
M=SLINT_DEFAULT_FONT=/System/Library/Fonts/Menlo.ttc
caffeinate -u -d -i python3 -I exp/floors.py --runs 10 --cold
caffeinate -u -d -i python3 -I exp/run.py cold-baseline --cold --runs 10
caffeinate -u -d -i python3 -I exp/run.py cold-no-system-fonts SPIKE_NO_SYSTEM_FONTS=1 $M --cold --runs 10
# One traced cold launch per config, for the phase breakdown.
for cfg in "" "SPIKE_NO_SYSTEM_FONTS=1 $M"; do
  sudo -n purge; sleep 2
  echo "== cold trace: ${cfg:-baseline}" >> exp/cold-traces.txt
  env $cfg SPIKE_TRACE=1 ./target/release/slint-spike bench start 2>&1 | grep -E ' ms |start_to|first_draw' >> exp/cold-traces.txt
done
echo done
