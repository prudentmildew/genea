#!/usr/bin/env python3
# PROTOTYPE (#18): run the spike under env configs; warm start (n runs) + idle footprint.
#   exp/run.py NAME [KEY=VAL ...] [--runs N] [--idle] [--cold]
import json, os, subprocess, sys, time, statistics, tempfile
here = os.path.dirname(os.path.abspath(__file__))
exe = os.path.join(here, '..', 'target', 'release', os.environ.get('SPIKE_BIN', 'slint-spike'))
args = sys.argv[1:]; name = args.pop(0)
runs = 20; idle = '--idle' in args; cold = '--cold' in args
if '--runs' in args: runs = int(args[args.index('--runs') + 1])
env = dict(os.environ)
for a in args:
    if '=' in a: k, v = a.split('=', 1); env[k] = v
def start():
    out = tempfile.mktemp(suffix='.json')
    subprocess.run([exe, 'bench', 'start', '--out', out], env=env, check=True, stderr=subprocess.DEVNULL)
    d = json.load(open(out)); os.remove(out); return d
def pct(xs, p):
    xs = sorted(xs); return xs[min(len(xs) - 1, int(round(p / 100 * (len(xs) - 1))))]
res = {'name': name, 'env': {k: v for k, v in env.items() if k.startswith(('SPIKE_', 'SLINT_'))}}
if runs:
    start()
    rs = []
    for _ in range(runs):
        if cold:
            subprocess.run(['sudo', '-n', 'purge'], check=True); time.sleep(2)
        rs.append(start())
    for key in ['start_to_content_visible_ms', 'start_to_visible_ms', 'start_to_first_frame_ms', 'main_to_first_frame_ms', 'first_before_rendering_ms', 'first_draw_ms']:
        xs = [r[key] for r in rs]
        res[key] = {'p50': round(statistics.median(xs), 1), 'p95': round(pct(xs, 95), 1)}
if idle:
    e = dict(env); e['SPIKE_IDLE_SECONDS'] = '16'
    p = subprocess.Popen([exe, 'bench', 'idle'], env=e, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(12)
    fp = subprocess.run(['footprint', '-p', str(p.pid)], capture_output=True, text=True).stdout
    p.wait()
    res['footprint'] = [l.strip() for l in fp.splitlines() if 'Footprint:' in l or 'graphics' in l or 'IOSurface' in l or 'Malloc Small' in l or 'phys_footprint' in l]
print(json.dumps(res))
with open(os.path.join(here, 'results.jsonl'), 'a') as f: f.write(json.dumps(res) + '\n')
