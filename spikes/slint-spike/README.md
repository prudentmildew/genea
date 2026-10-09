# Slint spike (PROTOTYPE, throwaway)

> Extended on branch `prototype/slint-budgets` for **Are the start-time and idle-memory budgets achievable, or should they be revised?** (prudentmildew/genea#18). See [Start time and idle memory](#start-time-and-idle-memory-18) below.

Answers **Does Slint meet Genea's budgets?** (prudentmildew/genea#15). It lives only on the `prototype/slint-spike` branch and is never merged. The verdict is recorded on the issue.

It's the GPUI spike (branch `prototype/gpui-spike`) ported to Slint `1.18.1`, using the winit backend and the Skia renderer, which draws to Metal through wgpu 30. It has the same ropey buffer, tree-sitter TypeScript highlighting of the visible lines, one cursor, pixel scrolling and blinking caret. The editor surface is Genea's own (`ui/editor.slint`), not Slint's `TextEdit`. Each highlight run is one Slint `Text`, laid out by Rust on a Menlo monospace grid. Lines sit in a ring of slots (`slot = line % slots`), so scrolling only replaces the slots of lines coming into view. `syntax.rs`, `sys.rs`, `synth.rs`, `suite.rs` and the fixtures are the GPUI spike's, unchanged.

## Run

Needs Rust (pinned by `rust-toolchain.toml`). The first build downloads Skia's prebuilt binaries (`skia-bindings`).

```sh
./fetch-fixtures.sh                      # checker.ts, typical.ts, onemb*.ts, huge.ts (~100 MB)
cargo build --release

# Feel check: type and trackpad-scroll in a real TS file
./target/release/slint-spike             # opens fixtures/checker.ts
./target/release/slint-spike path/to/file.ts

# Benchmarks (keep the display awake and don't touch the machine while they run)
caffeinate -u -d -i ./target/release/slint-spike suite --label raw

# Cold start needs `purge`, which needs sudo in the same terminal
sudo -v && caffeinate -u -d -i ./target/release/slint-spike suite --label cold

# Licence gate
cargo deny check licenses
```

Results are written to `results/<label>.json`.

## How things are measured

Slint has nothing like GPUI's foreground journal, so the spike keeps its own (`metrics.rs`). The same benchmarks and the same `proc_pid_rusage` accounting are used, so the two spikes compare like for like.

- **Main thread**: two CFRunLoop observers record every run-loop activity. A *busy span* runs from waking up to going back to sleep. The longest busy span is the "longest stall". This is stricter than GPUI's figure, which timed single events, because one busy span can hold several events.
- **Frames**: Slint's rendering notifier (`BeforeRendering`/`AfterRendering`; it needs the `unstable-wgpu-30` feature on Metal) marks the draw. A frame runs from the start of the run-loop segment its `BeforeRendering` falls in (the display-link tick, before waiting for a drawable) to the end of the segment its `AfterRendering` falls in (after Skia's flush and wgpu's `present` returned). Setting a rendering notifier turns Slint's partial rendering off, but the wgpu surface never uses partial rendering anyway.
- **Keystrokes**: the same synthetic `NSEvent`s as the GPUI spike, posted into the app's own queue: AppKit → winit → Slint → `FocusScope`. The winit event filter timestamps each key press. The edit log splits each keystroke into rope edit, tree-sitter reparse and view sync (pushing the visible lines to Slint), and then the Slint frame.
- **Scrolling**: a fixed number of pixels per frame, steady (30 px) and fling (200 px). The next step's view sync runs in a zero-delay timer after each `AfterRendering`. It's reported separately as `view_sync_ms`, because GPUI did this work inside its draw.
- **Start**, **idle**, **visibility**: as in the GPUI spike.

## Findings

The verdict and full numbers are on the issue. `results/raw.json` is the unconstrained run (Apple M5, 60 Hz display). `results/cold.json` is the same suite again, plus cold starts after `purge`. In short:

- **Idle is on demand.** Slint's `CADisplayLink` pauses when nothing is dirty. With the caret blink off, idle is 0.00 % CPU and 0.5 wake-ups/s. The caret blink is the only wake-up source, but every blink redraws the whole window (the wgpu surface has no partial rendering): about 0.8 % CPU and ~10 wake-ups per blink.
- **Keystroke → frame** p95 is 10.6 ms with the synchronous reparse, and 4.2 ms if the reparse were off the frame path. The Slint frame itself is p95 ~3.5 ms (draw ~2.5 ms).
- **Scrolling** at 60 Hz: 0 % late. Per-frame work p95 is ~3.2 ms steady and ~5.3 ms fling, plus ≤ 0.5 ms view sync.
- **Cold start** p95 is 963 ms (p50 861 ms), after `purge`. The binary is 22 MB, including prebuilt Skia.
- **Warm start** p95 is 198 ms. `BackendSelector::select` takes ~50 ms (winit event loop and NSApplication). Window, wgpu and Skia setup up to the first frame tick take ~95 ms, and the first draw (fonts, shaping) ~21 ms.
- **Idle memory** is 112 MB, 65 MB before the second frame: the same shape as GPUI. `footprint` attributes ~83 MB to graphics: ~47 MB GPU-private, 30 MB drawable IOSurfaces and 6 MB IOAccelerator. Setting the `CAMetalLayer` to two drawables (`SPIKE_TWO_DRAWABLES=1`) didn't change it.
- **Licences**: `cargo deny check licenses` passes. Slint's crates are `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0`, and only the royalty-free licence is allowed in `deny.toml`. `r-efi` has LGPL only as an OR alternative. There are no MPL exceptions. The prebuilt Skia binary is BSD-3 and includes ICU data (Unicode licence).
- **Feel**: typing and trackpad scrolling, including momentum, felt native, the same as GPUI (the user's judgement).

## Start time and idle memory (#18)

Branch `prototype/slint-budgets`, Apple M5 MacBook Air, 60 Hz. Warm figures use n = 20, cold figures n = 10 after `purge`. The verdict is on the issue.

### What was added

- `src/bin/floor.rs` gives two floors: `floor winit` is a bare winit window, and `floor wgpu` adds a wgpu Metal surface and clears it once. `floor-appkit/main.swift` is a plain AppKit window (built to `target/release/floor-appkit` with `swiftc -O floor-appkit/main.swift -o target/release/floor-appkit`). Each one prints its milestones in ms since process start, including `visible`, when AppKit first reports the window visible.
- The spike records `visible` too. The start bench now reports `start_to_content_visible_ms`, the later of the first frame being presented and the window being visible.
- Experiment knobs: `SPIKE_WINDOW=WxH`, `SPIKE_FILE`, `SPIKE_BLINK_STOP_AFTER=S`, `SPIKE_PREWARM=1` (a CoreText warm-up on a background thread), and the patched crates (`patches/README.md`). With `SPIKE_TRACE=1`, every trace line also prints the footprint.
- `exp/run.py` (warm or cold starts plus the idle footprint for one configuration), `exp/floors.py` and `exp/cold.sh`. Raw results are in `exp/results.jsonl` and `exp/cold-traces.txt`.

### Start time

| p50 / p95 (ms) | warm | cold (`purge`) |
|---|---|---|
| Plain AppKit window visible | 154 / 163 | 620 / 704 |
| Bare winit window visible | 120 / 127 | 621 / 736 |
| Raw wgpu, first present | 138 / 147 | 634 / 762 |
| Spike: content visible | 180–190 / 196–200 | 849 / 952 |
| Spike without the system-font scan | 157 / 176 | 663 / 742 |

- **The floor is AppKit and WindowServer.** A winit window with nothing drawn becomes visible at ~120 ms warm. In that time, `NSApplication` init takes ~30 ms, launch ~20 ms, window creation ~27 ms, and WindowServer takes ~30 ms to show the window. A plain Swift AppKit app is no faster. `purge` evicts AppKit too (`NSApplication` init goes from ~30 to ~200 ms), so the cold floor is ~620–750 ms.
- **Slint's system-font scan** (`fontique`, inside `BackendSelector::select`) costs ~25 ms warm and ~185 ms cold. Without it, the spike sits at the cold floor and ~40–50 ms above the warm one.
- **The first draw takes ~21 ms warm and ~38 ms cold.** About half of it is CoreText rasterizing the first glyphs, mostly a one-time load of font language metadata. A background CoreText warm-up doesn't help, because Skia rasterizes from an in-memory copy of the font, not the system font.
- **The occlusion gate isn't a lever.** wgpu skips drawing until AppKit reports the window visible. With the gate bypassed, the frame is ready ~35 ms earlier but the window appears ~20 ms later, so content shows up at the same time.

### Idle memory

| Idle footprint | MB |
|---|---|
| Bare winit window | 19 |
| Raw wgpu, one frame presented | 42 |
| Spike, caret blinking | 111–112 |
| Spike, caret not blinking (or blink stopped ≥ 1 s ago) | **65–66** |

- **The blink is the miss.** About 45–60 MB of GPU driver memory (`footprint`: "Owned physical footprint (unmapped) (graphics)") stays dirty while frames keep being rendered. About 1 s after the last frame, the driver marks it reclaimable and it drops out of the footprint. A 530 ms blink keeps it resident. With `SPIKE_BLINK_STOP_AFTER=10`, the footprint went 112 → 69 → 65 MB within 10 s of the blink stopping. This memory isn't Skia's resource cache, which holds ~4 MB. Capping or purging that cache did nothing, or made things worse.
- **Drawables scale with window size**: two drawables take 10, 30 and 57 MB at 600×400, 1200×800 and 1800×1100 pt. Everything else stays flat. Reducing the frame latency to 1 didn't change it.
- What remains at 66 MB: drawables 30 MB, malloc ~16 MB, graphics ~6 MB, the rest images and data.
