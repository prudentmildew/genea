# Slint as the GUI framework

**Supersedes [ADR 0003](0003-gpui-gui-framework.md).**

Genea's UI is built on Slint, a crates.io dependency pinned to an exact release (1.18.x at the time of writing). It uses the winit backend and the Skia renderer, which draws to Metal through wgpu. Genea's chrome (file tree, tabs, panels, dialogs) is `.slint` markup bound to Rust models. The editor surface is Genea's own, a Rust-driven grid of `Text` items, not `TextEdit`.

The choice comes from two spikes measured on the same machine, with the same harness and fixtures ([Does GPUI meet Genea's budgets?](https://github.com/prudentmildew/genea/issues/14) and [Does Slint meet Genea's budgets?](https://github.com/prudentmildew/genea/issues/15)). Only two budgets separated the frameworks, and Slint won both:

- **Idle wake-ups.** Slint renders only when something is dirty. GPUI's macOS display link wakes the main thread on every vsync while a window is visible. Upstream hasn't fixed this. The closest change, demand-driven frames on Windows, was declined in favour of "internal plans". Meeting the budget on GPUI would mean carrying a fork from the first commit.
- **Keystroke to frame.** With the tree-sitter reparse off the frame path, Slint takes 4.2 ms. GPUI takes 10.3 ms, and about 5 % of its frames wait ~8 ms between draw and present for an unknown reason.

Both frameworks missed idle memory (~111 MB, mostly Metal and Skia graphics) and start time (warm ~175–200 ms, cold 0.75–1 s) in the same way. Those misses didn't separate the two frameworks and are handled outside this decision. Both passed on feel, scrolling, stalls, file opening and licences.

We accept that:

- the UI is written in a second language;
- the binary is larger (22 MB with prebuilt Skia) and cold start a little slower;
- the royalty-free licence needs attribution: an About dialog shows `AboutSlint`. It also forbids exposing Slint's APIs, which is fine while plugins are out of scope;
- 1.18 has no partial rendering on the GPU path, so every caret blink repaints the whole window.

## Considered Options

- **GPUI** (ADR 0003): the most macOS-native stack. It has its own `NSTextInputClient` and Zed proves it in production. Rejected because it needs a fork from the start to meet the idle budget, it misses the keystroke budget for an unknown reason, and it is a git-only, pre-1.0 dependency with thin docs.
- **Slint 1.17**, pinned for its native Skia Metal surface, which supported opt-in partial rendering. Rejected: it pins an old version and loses fixes, and installing a rendering notifier (which the benchmark harness needs) turns partial rendering off anyway.
- **iced, egui, Xilem/Masonry, Floem, Makepad**: rejected in ADR 0003, for reasons unchanged by the spikes.

## Consequences

- **The core stays framework-free.** Buffer and undo, syntax, the LSP client, git, the file watcher and review baseline, the toolchain runner and config live in crates that don't depend on Slint. Only the view layer touches Slint. There is no GUI abstraction layer: switching frameworks means rewriting the views, not the core.
- **Pin bumps are deliberate.** Slint is pinned to an exact version. The pin moves only when a fix or feature is needed, or after a minor release, and every bump must pass the benchmark harness first. Features marked `unstable-*` are avoided where possible, because they fall outside Slint's 1.x stability promise.
- **Text input goes through a hidden `TextInput`.** Slint exposes IME composition (marked text, the IME cursor area) only to its built-in `TextInput`; `FocusScope` drops composition events, so the IME is never switched on and dead keys break: on a Norwegian layout ⇧´ space gives a space instead of `` ` ``. Genea keeps a transparent, focused `TextInput` at the composition start. Genea reads its `preedit-text`, draws the preedit inline itself, takes each commit into its own buffer and empties the `TextInput`. Its `key-pressed` accepts every key, so arrows, shortcuts and undo stay Genea's. The `TextInput` must be re-focused once the native window exists, because Slint drops the IME request made before then. No Slint patch is carried. [Can Genea's Slint editor surface take marked text?](https://github.com/prudentmildew/genea/issues/17) proved this route.
- **IME scope is dead keys.** Genea is for writing code, so v1 needs dead keys (`` ` ``, `^`, `~` and accents on layouts like Norwegian), and nothing more. CJK input, the emoji picker and press-and-hold accents are out of v1 scope and untested. Press-and-hold is known to be at risk below Slint: winit 0.30 ignores the replacement range of `insertText:`.
- **Reopen condition.** The decision reopens if Genea's editor surface can't take dead-key composition.
- **The caret blink costs a full repaint.** The Editing core's blink policy has to account for it.
- **Native menus via muda.** The application menu bar and context menus are native `NSMenu`s.
- **Accessibility is not a v1 requirement.** Slint's AccessKit support keeps it possible later. A custom editor surface would need its own `accessible-*` properties.
- **No LCD subpixel antialiasing.** No candidate offers it, and macOS has had it off by default since Mojave.
