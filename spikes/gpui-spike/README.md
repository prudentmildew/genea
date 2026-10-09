# GPUI spike (PROTOTYPE, throwaway)

Answers **Does GPUI meet Genea's budgets?** (prudentmildew/genea#14). It lives only on the `prototype/gpui-spike` branch and is never merged. The verdict is recorded on the issue.

It's a minimal editor surface on GPUI pinned to Zed `v1.23.2`: a ropey buffer, tree-sitter TypeScript highlighting of the visible lines, one cursor, pixel scrolling and a blinking caret. It uses only Zed's Apache-2.0 crates.

## Run

Needs Rust (pinned by `rust-toolchain.toml`) and Xcode with the Metal Toolchain (`xcodebuild -downloadComponent MetalToolchain`).

```sh
./fetch-fixtures.sh                      # checker.ts, typical.ts, onemb*.ts, huge.ts (~100 MB)
cargo build --release

# Feel check: type and trackpad-scroll in a real TS file
./target/release/gpui-spike              # opens fixtures/checker.ts
./target/release/gpui-spike path/to/file.ts

# Benchmarks (keep the display awake and don't touch the machine while they run)
caffeinate -u -d -i ./target/release/gpui-spike suite --label raw
caffeinate -u -d -i taskpolicy -c background ./target/release/gpui-spike suite --label ecore

# Cold start needs `purge`, which needs sudo in the same terminal
sudo -v && caffeinate -u -d -i ./target/release/gpui-spike suite --label cold

# Licence gate
cargo deny check licenses
```

Results are written to `results/<label>.json`.

## How things are measured

- **Frames, input, stalls**: GPUI's own foreground journal (`profiler` feature) records every input dispatch, action, task poll, draw and present on the main thread. The spike drains it on a side thread. The idle bench doesn't do this, because the drain thread would be a wake-up source itself.
- **Keystrokes**: synthetic `NSEvent`s are posted into the app's own queue, so they take the real path: AppKit → GPUI → `NSTextInputClient` → the editor. The spike's edit log splits each keystroke into rope edit, tree-sitter reparse, waiting for the frame, GPUI draw and GPUI present.
- **Scrolling**: a fixed number of pixels per frame, driven by `request_animation_frame`, both steady (30 px/frame) and fling (200 px/frame, all-new lines every frame).
- **Start**: from the process start the kernel recorded (`proc_pid_rusage`) to the first presented frame, which already shows the restored file. This is a raw binary launch, not through LaunchServices or an `.app` bundle.
- **Idle**: CPU time, wake-ups (package idle + interrupt) and physical footprint from `proc_pid_rusage`, over 30 s after a 10 s settle.
- **Reference machine**: approximated by pinning the whole suite to the efficiency cores (`taskpolicy -c background`). This turned out to be a poor stand-in: the background clamp also throttles timers and the display link (frames ~100 ms apart while drawing took 2–3 ms), so only the CPU-side figures in `results/ecore.json` are meaningful.
- **Visibility**: macOS throttles presentation for windows that are covered or on another Space. Every bench records `window_visible_at_end`; don't use the machine while a suite runs.

## Findings

Verdict and full numbers are on the issue. `results/raw.json` (unconstrained, Apple M5, 60 Hz display) and `results/ecore.json` are the runs it quotes. In short:

- GPUI drawing is cheap: p95 1.5 ms per keystroke frame, 2.0 ms steady scroll, 6.0 ms fling.
- GPUI's display link wakes the main thread every vsync while the window is visible (`gpui_macos/src/window.rs`, `start_display_link`/`step`), so idle is ~40–100 wake-ups/s, not just the caret blink.
- Idle footprint is 111 MB, of which ~46 MB is Metal's three window-sized drawables (65 MB before the second frame).
- Warm start p95 175 ms; GPUI's `application()` and `open_window` are ~75 ms of it.
- Keystroke-to-frame is dominated by the synchronous tree-sitter reparse (p95 8 ms on checker.ts).
