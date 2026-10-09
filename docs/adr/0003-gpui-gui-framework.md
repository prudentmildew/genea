# GPUI as the GUI framework

**Status: reopened.** The validation spike ([Does GPUI meet Genea's budgets?](https://github.com/prudentmildew/genea/issues/14)) missed several budgets: idle wake-ups, idle memory, start time and keystroke-to-frame. Under the reopen condition below, a Slint spike is measured with the same harness, and the decision is then taken again. GPUI stays the working choice until then.

Genea's UI is built on GPUI, the framework Zed is built on. It is consumed as a git dependency on `zed-industries/zed`, pinned to a Zed stable release tag. GPUI is the only candidate whose whole stack is shaped like Genea: a 120 fps code editor on Metal, with CoreText shaping, its own `NSTextInputClient`, a native `NSMenu`, and AccessKit. Zed 1.x proves it in production every week. We accept that GPUI is pre-1.0 with frequent breaking changes, has thin docs, isn't usefully published to crates.io, and follows Zed's roadmap rather than ours.

Genea depends only on Zed's Apache-2.0 crates (GPUI and its support crates such as `sum_tree`). It does not depend on Zed's GPL crates (`editor`, `rope`, `text`, `ui`, `theme`, `lsp`) or on gpui-component. Genea writes its own components and editor surface, building the editor on the crates from the editor-crates research. Apache-2.0 code from gpui-component may be copied in with attribution.

## Considered Options

- **Slint**: the only framework with a stable 1.x API, and the best docs. Rejected because its `TextEdit` reportedly becomes unusable past 5K lines, its default FemtoVG/OpenGL renderer has weaker text, UI goes in a second language (`.slint`), and the royalty-free licence requires attribution. It is the fallback (see Consequences).
- **iced, egui, Xilem/Masonry**: no native menu bar and no code editor shipped on them. iced also lacks accessibility and depends on one maintainer, egui is immediate mode, and Xilem is alpha.
- **Floem**: Lapce proves it can carry an editor, but it is effectively stalled: no release since 2024, little activity on `main`, its rendering rework closed unmerged, and no accessibility.
- **Makepad**: it ships an editor, but has no accessibility, almost no API docs, and its focus has moved to an AI dev environment.
- **gpui-component as the component layer**: rejected. It would tie Genea to Longbridge's `gpui-pre` snapshot and put a third-party editor widget in the ≤ 8 ms keystroke path.
- **Reusing Zed's editor crates**: rejected. They are GPL-3.0 and wired into Zed's own workspace and settings model.

## Consequences

- **The core stays framework-free.** Buffer and undo, syntax, the LSP client, git, the file watcher and review baseline, the toolchain runner and config live in crates that don't depend on GPUI. Only the view layer touches GPUI. There is no GUI abstraction layer: switching frameworks means rewriting the views, not the core.
- **Pin bumps are deliberate.** The pin moves only when a GPUI fix or feature is needed, or about once a quarter, and every bump must pass the benchmark harness first. The Rust toolchain version follows the pinned tag. Genea forks GPUI only for a patch upstream won't take, and the fork starts from a tag.
- **Reopen condition.** The decision was made on documentary evidence. It reopens if the validation spike misses a Genea budget, or if a licence check finds a GPL crate in GPUI's pinned dependency tree. Slint is then the first candidate, with Genea's own editor surface on Skia/Metal rather than `TextEdit`.
- **Accessibility is not a v1 requirement.** GPUI's AccessKit support keeps it possible later.
- **No LCD subpixel antialiasing.** No candidate offers it, and macOS has had it off by default since Mojave.
