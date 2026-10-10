# Benchmark harness

`genea-bench` (`crates/genea-bench`) measures the Genea budgets (GLOSSARY.md;
spec #19, Benchmark harness) on the real `genea` binary, at p95, and fails
when one is missed. It runs locally before every release and every Slint pin
bump (ADR 0004); hosted CI has no suitable GPU or display.

## Running it

```sh
bench/workspaces/typical/setup.sh            # once: the Typical reference workspace (workspaces/README.md)
bench/workspaces/large/fetch.sh --ignore-scripts  # once: the Large one (vscode); its budgets are skipped without it
scripts/bench.sh                             # the whole harness: builds release binaries, runs everything
```

`scripts/bench.sh` builds `genea`, `genea-bench`, `genea-floor` and `genea-fake-lsp` in release
mode, asks for your password once so cold starts can run `sudo purge`, and
runs the harness under `caffeinate`. Options go through to `genea-bench`:

| Option | Effect |
| --- | --- |
| `--quick` | a smoke test with few runs; not a release gate |
| `--only start,typing,…` | only these scenarios: `start`, `typing`, `typing-silent-lsp`, `scroll`, `dead-keys`, `idle`, `open-1mb`, `open-100mb`, `finder`, `search`, `tree`, `external-change`, `memory`, `language-server` |
| `--no-cold` | skip cold starts (no `sudo`) |
| `--runs N`, `--cold-runs N` | start pairs, warm (default 30) and cold (default 10) |
| `--label L`, `--out DIR` | where results go (default `bench/results/<UTC time>`) |
| `--genea`, `--floor`, `--fake-lsp`, `--workspace`, `--large` | other binaries or workspaces |
| `--reference-machine` | this is the reference machine: the results say so (they say "a dev machine" otherwise) |

A full run takes about 20 minutes, most of it on Large. Leave the machine alone and keep Genea's
window in front and unobstructed: occluded windows are throttled, and input
disturbs idle measurements (the idle scenario notices and retries).

The exit code is 0 when every Genea budget measured was met, 1 when one was
missed or a scenario couldn't run, 2 for a usage error. A skipped budget
(cold starts without `sudo`, say) is listed as SKIP and doesn't fail the run.

## What it measures

Every scenario but the open ones launches Genea on the Typical workspace
with `packages/web/src/index.ts` open, at its default window size; the idle
budget runs resize it to 1200×800 pt once it shows. The open scenarios
launch Genea on generated TypeScript in `bench/workspaces/out/open-files`
(written on first use, beside the Typical workspace) with a small file open,
then ask it to open the file under test, in a fresh Genea per run.

The navigation and memory scenarios (ticket #63) run on both reference
workspaces: Typical with `packages/web/src/index.ts` open, Large with
`src/vs/editor/common/model/textModel.ts`. Each launches Genea and waits
until it has had nothing to do for a second (the project's files read)
before measuring. Their times, like a keystroke's, are Genea's work: from
the key, the request or the write on disk to the view state in Slint, plus
the frame that shows it, leaving out the wait for the display-link tick.

| Scenario | Budgets |
| --- | --- |
| `start` | warm start ≤ start floor + 50 ms; cold start after `purge` ≤ start floor + 100 ms (content visible, p95 minus the floor's p95, every Genea run paired with a floor run) |
| `typing` | keystroke to frame ≤ 8 ms (Genea's work, p95, 400 keys at 25/s); no main-thread stall > 16 ms |
| `typing-silent-lsp` | the same, in a copy of the file whose project's tsgo is `genea-fake-lsp` scripted never to answer (ticket #42: typing never waits on a language server) |
| `scroll` | ≤ 1 % dropped frames at the display's rate; frame work ≤ 8.3 ms (a 120 Hz frame, p95); no stall > 16 ms |
| `dead-keys` | dead-key compositions on the Norwegian layout come out right (the IME bridge end to end); no stall > 16 ms |
| `idle` | memory ≤ 100 MB (phys_footprint ≥ 2 s after the last frame, 1200×800 pt at 2×, p95); CPU ≈ 0 % (≤ 0.1 %) |
| `open-1mb` | opening a 1 MB file ≤ 50 ms (the `open` request to the end of the first frame showing it, p95, 20 runs) |
| `open-100mb` | opening a 100 MB file (a large file: no highlighting) shows its first screen ≤ 1 s (p95, 5 runs); no main-thread stall > 16 ms from the request until 300 ms after the whole file is in |
| `finder` | Go to File, each keystroke to its results shown ≤ 16 ms Typical, ≤ 50 ms Large (p95; three queries typed a key at a time, three rounds) |
| `search` | project search, request to first results shown ≤ 100 ms Typical, ≤ 250 ms Large; to complete ≤ 500 ms Typical, ≤ 3 s Large (p95, 20 searches for `invoice` and `createDecorator`) |
| `tree` | a folder expanded in the Files view ≤ 16 ms (both, p95; seven folders, parents first, ten rounds) |
| `external-change` | the open file rewritten and a file created at the root, outside Genea, shown in the editor and the tree ≤ 200 ms Typical, ≤ 500 ms Large (p95, 20 changes; from the writes, so the watcher's latency counts) |
| `memory` | memory with the workspace open ≤ 300 MB Typical, ≤ 1 GB Large (phys_footprint of Genea alone, ≥ 2 s after it went quiet with the language server ready, 1200×800 pt at 2×, p95); *target*: the language server ≤ 1.5 GB on Typical |
| `language-server` | Typical with its real tsgo. *Targets*, reported only: first diagnostics, cold, ≤ 2 s (launch to the error shown; after `purge` when cold runs are on and `sudo` works); diagnostics after an edit ≤ 300 ms (key to the problems shown); completions and go-to-definition ≤ 100 ms (timed against tsgo directly, because Genea has neither yet: #43, #44). *Budget*: a language server never blocks typing or rendering: the longest main-thread stall or keystroke while tsgo loads and checks ≤ 16 ms |

End-to-end targets show as MISS when missed and never fail the run.
`external-change` and `language-server` write into Typical (and Large) and
undo it afterwards; `git status` in the workspace shows anything a killed run
left behind.

The budget table is `crates/genea-bench/src/budgets.rs`. The start floor is
`genea-floor`, a bare winit window (the winit Slint links) that exits when
AppKit first reports it visible.

On a display below 120 Hz the scroll scenario checks dropped frames at the
display's rate plus frame work against a 120 Hz frame, and says so; 120 fps
itself needs a ProMotion display. The reference machine (spec #19, Further
Notes) isn't available yet, so results are from the dev machine, with the
start floor as the normaliser. Every result records the machine (model,
chip, cores, memory, macOS, displays and refresh rates) and whether it is
the reference machine, which only `--reference-machine` claims; the summary
starts with it.

## Results

Each run writes `bench/results/<label>.jsonl` and
`bench/results/<label>.summary.txt` (gitignored) and prints the summary.
JSONL lines have a `type`: `session` (machine, Genea commit, the Typical
and Large workspaces' fingerprints), `run` (one measured run), `result` (a scenario's
statistics), `check` (one budget: measured, limit, verdict), `error` and
`verdict`.

## How it works

Genea has an instrumentation journal, off unless `GENEA_JOURNAL=1` is set
(`crates/genea-view/src/journal.rs`). Off, it installs nothing. On, it
records run-loop activity (two CFRunLoop observers: main-thread stalls and
wake-ups), Slint's frames, key presses and IME commits as winit delivers
them, window events from outside, each sync of view state into Slint, the
first sync with file content, and the window becoming visible. It also opens
a control channel on stdin/stdout (`crates/genea-view/src/remote.rs`): the
harness sends primitive commands (`wait-content`, `key`, `place-caret`,
`scroll`, `resize`, `caret-line`, `open`, `editor`, `finder`, `search`,
`toggle-folder`, `expect`, `check`, `await`, `processes`, `info`, `journal`,
`quit`) and reads one JSON line back per command. The journal also records
each `open` request and the first sync showing that file.

To time anything else that shows in view state, the harness `expect`s a
condition on the first window's `ProjectView` (the finder showing the
results for a query, a search finished, a folder expanded, a line in an
editor, a number of problems in a file, …; `remote.rs` lists them), acts,
then `await`s it: Genea notes the end of the first sync whose view met it
and answers once a frame drawn after that has presented. The harness then
takes the reaction from the journal like a keystroke.

Keys are posted as CGEvent-backed `NSEvent`s into Genea's own event queue, so
they take AppKit's real text-input path (dead keys compose) without
Accessibility permission. Memory, CPU and wake-ups are read from outside with
`proc_pid_rusage`. Journal timestamps are `mach_absolute_time`, which the
harness shares, and the harness derives everything (`src/journal.rs` in the
harness): a frame runs from the display-link tick to the present returning;
a keystroke's work is key → view state in Slint, plus the frame that shows
it; a stall is the main thread's busy span from waking to waiting.

`GENEA_JOURNAL_TRACE=1` (with the journal on) prints winit's window events to
stderr.

## Adding a scenario

1. Add the budgets (or `Kind::Target` end-to-end targets, which are reported
   but never fail the run) to `crates/genea-bench/src/budgets.rs`.
2. Write `crates/genea-bench/src/scenarios/<name>.rs` with a type that
   implements `Scenario`, and list it in `scenarios::all()`
   (`scenarios/mod.rs` has the details).
3. If what you time shows in view state, `expect` a condition on it and
   `await` it (add a condition to `remote.rs` if none fits), as the
   navigation scenarios do. If Genea needs a new command, add it to
   `remote.rs`; if the journal needs a new mark, add it to `journal.rs` and
   to `Journal` in the harness.
