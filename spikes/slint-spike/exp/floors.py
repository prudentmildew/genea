#!/usr/bin/env python3
# PROTOTYPE (#18): start-time floors. Runs floor binaries N times (optionally after
# `purge`) and reports p50/p95 of each mark (ms since process start).
#   exp/floors.py [--runs N] [--cold]
import json, os, subprocess, sys, time, statistics
here = os.path.dirname(os.path.abspath(__file__)); rel = os.path.join(here, '..', 'target', 'release')
runs = int(sys.argv[sys.argv.index('--runs') + 1]) if '--runs' in sys.argv else 20
cold = '--cold' in sys.argv
def pct(xs, p):
    xs = sorted(xs); return xs[min(len(xs) - 1, int(round(p / 100 * (len(xs) - 1))))]
for name, cmd in [('floor-appkit', [os.path.join(rel, 'floor-appkit')]),
                  ('floor-winit', [os.path.join(rel, 'floor'), 'winit']),
                  ('floor-wgpu', [os.path.join(rel, 'floor'), 'wgpu'])]:
    subprocess.run(cmd, capture_output=True)
    rs = []
    for _ in range(runs):
        if cold:
            subprocess.run(['sudo', '-n', 'purge'], check=True); time.sleep(2)
        out = subprocess.run(cmd, capture_output=True, text=True).stdout.strip().splitlines()[-1]
        rs.append(json.loads(out))
    res = {'name': name + ('-cold' if cold else ''), 'runs': runs}
    for k in rs[0]:
        xs = [r[k] for r in rs if k in r]
        res[k] = {'p50': round(statistics.median(xs), 1), 'p95': round(pct(xs, 95), 1)}
    print(json.dumps(res))
    with open(os.path.join(here, 'results.jsonl'), 'a') as f: f.write(json.dumps(res) + '\n')
