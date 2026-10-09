# Slint spike (PROTOTYPE, throwaway)

> **Branch `prototype/slint-ime`** extends this spike for **Can Genea's Slint editor surface take marked text?** (prudentmildew/genea#17). See [IME bridge](#ime-bridge-genea17) below.

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

## IME bridge (genea#17)

The editor's `FocusScope` is replaced by `ime`, a transparent, focused `TextInput` kept at the composition start (`ui/editor.slint`). Genea reads its `preedit-text`, splices it into the caret line's runs and draws it in blue with an underline (`splice_preedit` in `editor.rs`), and inserts each commit into the rope. After each commit the `TextInput`'s text is emptied. Its `key-pressed` accepts every key, so its own shortcuts (undo, select-all, arrows) never run. Grid columns now use `unicode-width`.

```sh
SPIKE_IME_LOG=1 ./target/release/slint-spike path/to/file.ts   # key/IME events on stderr and in the status bar
```

Findings (Norwegian layout, events synthesized with `osascript`):

- **Dead keys work.** ´e → é, ¨u → ü, ¨a → ä, ´ space → ´, ⇧´ space → `` ` ``, ⇧¨ space → ^, ⌥¨ space → ~. Each arrives as winit `Preedit` and then `Commit`.
- **Without the bridge they break.** The original spike (`FocusScope`, IME never enabled) types a space for ⇧´ space and a plain e for ´e.
- **Re-focus once the window exists.** The `TextInput` takes focus before the winit window exists, and Slint drops the IME enable request then. A zero-delay timer re-focuses it once the event loop runs.
- **winit only reports composition when there is marked text.** Plain typing still arrives as key presses, so the keystroke benchmark path is unchanged for ASCII.
- **Not tested (out of v1 scope):** CJK, the emoji picker, press-and-hold. winit 0.30's `insertText:` ignores the replacement range, so press-and-hold may insert "eé" no matter what Slint does.
