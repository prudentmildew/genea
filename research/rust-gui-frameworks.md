# Which Rust GUI frameworks could Genea build on?

Research for [#4](https://github.com/prudentmildew/genea/issues/4), part of the wayfinder map [#1](https://github.com/prudentmildew/genea/issues/1). Snapshot taken **2026-10-09**.

The question: which Rust GUI frameworks could Genea build a native macOS IDE on? This file is facts only. Choosing a framework is a later grilling ticket ([#10](https://github.com/prudentmildew/genea/issues/10)).

**Method.** Every claim is traced to a primary source: the framework's repo (README, `Cargo.toml`, LICENSE, CHANGELOG, source), its official docs, GitHub releases, the crates.io API, or blog posts by the maintainers. Repo statistics come from the GitHub API on the snapshot date. Claims that could not be verified are marked **Unverified**.

**Terms.** These follow [GLOSSARY.md](../GLOSSARY.md). Genea's editor surface must give *first-class languages* (TS/TSX/JS/JSX) syntax highlighting plus *language intelligence*, so the framework's text and input stack decides much of the outcome.

---

## At a glance

| | GPUI | iced | Slint | Makepad | Floem | egui | Xilem / Masonry |
|---|---|---|---|---|---|---|---|
| Latest release | crates.io 0.2.2 (2025-10-22); `main` is used via git | 0.14.0 (2025-12-07) | 1.18.1 (2026-09-21) | crates.io 1.0.0 (2025-05-13); `dev` is at 2.0 | 0.2.0 (2024-11-14) | 0.36.2 (2026-09-08) | 0.4.0 (2025-10-29) |
| Activity (2026) | very high, inside Zed | high, almost all one maintainer | very high | high | low on `main` | high | moderate, then falling |
| Backer | Zed Industries (VC-funded) | hecrj (sponsors) | SixtyFPS GmbH (licence sales) | Makepad B.V. | Lapce contributors | Rerun | Linebender |
| Stated stability | pre-1.0, "often breaking changes" | "experimental" | 1.x, stable API | no policy | "still maturing" | "interfaces still in flux" | "experimental", "alpha-quality" |
| macOS GPU | Metal (own renderer) | wgpu (Metal), tiny-skia fallback | FemtoVG/OpenGL by default; Skia (Metal), FemtoVG-wgpu | Metal (own platform layer) | wgpu + vger (vello and Skia optional) | wgpu (Metal) or glow | Vello on wgpu |
| Text shaping | CoreText | cosmic-text (fork) | Parley (HarfRust) | rustybuzz (vendored), SDF atlases | cosmic-text in 0.2; Parley on `main` | harfrust + skrifa (since 0.35) | Parley (HarfRust) |
| Ligatures | yes | not documented | yes | yes (GSUB) | yes (fixes landed in 2025) | yes (0.35) | Unverified |
| LCD subpixel AA | no (grayscale) | no | no | none found | no | no | no |
| macOS IME | full `NSTextInputClient` | since 0.14; open macOS press-and-hold bug | through winit; nothing macOS-specific found | own handler, with tests | through winit fork; open bugs | through winit; minor open macOS bugs | through winit; fixes in 2026 |
| Accessibility | AccessKit (merged 2026-05) | none upstream | AccessKit, on by default | none (no-op) | none (draft PR stale) | AccessKit, always on | AccessKit integrated |
| Native macOS menus | yes (NSMenu) | no (custom-drawn planned) | yes (muda) | yes (NSMenu) | yes (muda) | no | no (planned in-app) |
| Shipped code editor | **Zed 1.x** (plus Longbridge Pro, not an editor) | none on upstream (cosmic-edit uses the pop-os fork) | none found | Makepad Studio / Director | **Lapce** 0.4.6, pre-1.0 | none (Ferrite is a Markdown/config editor) | none |
| Docs | thin: 2 guides, examples, stale docs.rs | strong API docs, incomplete book | extensive: guides, reference, tutorials | weak (5.9% of items on docs.rs) | lags `main` | good API docs and demo, no book | thin (admitted in #392) |
| Licence | Apache-2.0 (Zed's editor/ui crates are GPL-3.0-or-later) | MIT | GPL-3.0 / royalty-free / commercial | MIT (crates say MIT OR Apache-2.0) | MIT | MIT OR Apache-2.0 | Apache-2.0 |

Some background that the rows above take for granted:

- **LCD subpixel antialiasing.** None of the frameworks has it. Modern macOS also disables LCD font smoothing by default (since 10.14 Mojave), so on macOS this gap mostly shows up on non-Retina displays. This is general platform knowledge and is not cited to a primary source here.
- **What "text shaping" means.** The column only covers how text is laid out and shaped. A large-document editing surface (rope, virtualised lines, syntax highlighting) is a separate layer. Of these frameworks, only GPUI/Zed, Floem/Lapce and Makepad have shipped one, and gpui-component offers a third-party one (see the GPUI section).

---

## GPUI

**Maturity and maintenance**

- **Releases.**
  - On crates.io, `gpui` is at 0.2.2, published 2025-10-22 ([crates.io](https://crates.io/api/v1/crates/gpui)).
  - On `main`, the crate still says 0.2.2 ([Cargo.toml](https://github.com/zed-industries/zed/blob/main/crates/gpui/Cargo.toml)).
  - On `main`, the platform code has moved into separate crates: `gpui_platform`, `gpui_macos`, `gpui_apple`, `gpui_wgpu`. All of them inherit `publish = false` ([root Cargo.toml](https://github.com/zed-industries/zed/blob/main/Cargo.toml)).
  - The README tells users to depend on `gpui_platform`, yet that crate is not on crates.io ([README](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md)). Inference: current GPUI realistically has to be used as a git dependency.
  - An unofficial weekly snapshot, `gpui-pre` 0.3.8 (2026-10-05), is published by Longbridge, not by Zed ([crates.io](https://crates.io/api/v1/crates/gpui-pre)).
- **Stability.** The README says GPUI is "pre-1.0" and that there will "often be breaking changes between versions" ([README](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md)). The website says "For the near future, GPUI is tied to Zed" ([gpui.rs](https://www.gpui.rs/)).
- **Activity.** The zed repo has about 91.5k stars. About 170 commits touched `crates/gpui` between 2026-07-09 and 2026-10-09. Zed itself ships weekly stable releases, for example v1.23.2 on 2026-10-07 ([releases](https://github.com/zed-industries/zed/releases)).
- **Maintainer.** Zed Industries maintains GPUI. Its $32M Series B was led by Sequoia ([blog](https://zed.dev/blog/sequoia-backs-zed)).

**GPU rendering**

- On macOS, GPUI uses its own Metal renderer (`metal_renderer.rs` and `shaders.metal` in [gpui_apple](https://github.com/zed-industries/zed/tree/main/crates/gpui_apple/src)).
- On Linux it uses wgpu 29 through `gpui_wgpu` ([gpui_linux/Cargo.toml](https://github.com/zed-industries/zed/blob/main/crates/gpui_linux/Cargo.toml)).
- Each primitive type has its own instanced shader, using SDF rounded rects and analytic shadows ([zed.dev/blog/videogame](https://zed.dev/blog/videogame)).
- The website describes the model as "hybrid immediate and retained mode" ([gpui.rs](https://www.gpui.rs/)).

**Text**

- **Shaping and rasterisation.** On macOS, shaping uses CoreText (`CTLine`) and rasterisation uses CoreGraphics ([text_system.rs](https://github.com/zed-industries/zed/blob/main/crates/gpui_apple/src/text_system.rs)).
- **Antialiasing and positioning.** Antialiasing is grayscale only; there is no LCD path. Glyph positioning is subpixel, with up to 16 variants per glyph (same file; [blog](https://zed.dev/blog/videogame)).
- **Ligatures.** Ligatures and OpenType features are supported, and ligatures can be turned off with `FontFeatures::disable_ligatures()` (same file).
- **Large documents.** GPUI provides virtualised `uniform_list` and `list` elements. Zed's editor and rope are separate crates under GPL (see Licence below) ([rope post](https://zed.dev/blog/zed-decoded-rope-sumtree)).

**IME and input**

- **Text input.** `GPUIView` implements `NSTextInputClient`: marked text, `insertText:replacementRange:`, `firstRectForCharacterRange` and `doCommandBySelector:` ([window.rs](https://github.com/zed-industries/zed/blob/main/crates/gpui_macos/src/window.rs)).
- **Dead keys and keyboard layouts.** These are handled with `UCKeyTranslate`, with a fallback for non-ASCII layouts ([events.rs](https://github.com/zed-industries/zed/blob/main/crates/gpui_macos/src/events.rs)).
- **Keybindings.** The system is actions plus key contexts plus keymaps ([key_dispatch.md](https://github.com/zed-industries/zed/blob/main/crates/gpui/docs/key_dispatch.md)).

**Accessibility**

- AccessKit support merged on 2026-05-27 ([#56065](https://github.com/zed-industries/zed/pull/56065)). On macOS it uses `accesskit_macos` 0.26 ([gpui_macos/Cargo.toml](https://github.com/zed-industries/zed/blob/main/crates/gpui_macos/Cargo.toml)).
- Follow-up PRs add landmarks and menus ([#60397](https://github.com/zed-industries/zed/pull/60397)) and identifiers ([#61926](https://github.com/zed-industries/zed/pull/61926)).
- The umbrella issue [#41138](https://github.com/zed-industries/zed/issues/41138) is still open.
- **Unverified:** how well VoiceOver works in practice.

**macOS feel**

- **Native pieces.** GPUI uses a native `NSMenu` menu bar, a dock menu, the Services menu, and `NSOpenPanel`/`NSSavePanel` ([platform.rs](https://github.com/zed-industries/zed/blob/main/crates/gpui_macos/src/platform.rs)).
- **Windows.** Windows are `NSWindow`s, with `NSVisualEffectView` blur and custom positioning for the traffic-light buttons ([window.rs](https://github.com/zed-industries/zed/blob/main/crates/gpui_macos/src/window.rs)).
- **Controls.** All controls are custom-drawn.
- **Trackpad.** GPUI tells precise scroll deltas apart from line deltas, and maps pinch to `PinchEvent` ([events.rs](https://github.com/zed-industries/zed/blob/main/crates/gpui_macos/src/events.rs)).
- **Unverified:** whether momentum scrolling is handled. `momentumPhase` is not read.

**Shipped editor**

- **Zed** reached 1.0 on 2026-04-29 and runs on macOS, Windows and Linux ([Zed 1.0](https://zed.dev/blog/zed-1-0)).
- **Longbridge Pro**, a commercial app that is not an editor, is built on GPUI through gpui-component ([gpui-kit](https://github.com/longbridge/gpui-kit)).

**Docs**

- docs.rs covers only 0.2.2 and was built for Linux ([docs.rs](https://docs.rs/crate/gpui/latest)).
- There are two in-repo guides ([docs](https://github.com/zed-industries/zed/tree/main/crates/gpui/docs)) and about 24 examples. The website points readers to Zed's own crates for more ([gpui.rs](https://www.gpui.rs/)).

**Licence**

- **The GPUI crates are Apache-2.0.** That covers `gpui`, `gpui_platform`, `gpui_macos`, `gpui_wgpu` and `sum_tree` ([gpui Cargo.toml](https://github.com/zed-industries/zed/blob/main/crates/gpui/Cargo.toml)).
- **Zed's own crates are GPL-3.0-or-later.** That covers `editor`, `rope`, `text`, `ui`, `theme` and `lsp` ([editor Cargo.toml](https://github.com/zed-industries/zed/blob/main/crates/editor/Cargo.toml)). Zed's editor widget and component library therefore cannot be reused under a permissive licence.

**Ecosystem: gpui-component**

- `longbridge/gpui-component`, which redirects to `longbridge/gpui-kit`, has about 16.7k stars ([repo](https://github.com/longbridge/gpui-kit)).
- It is Apache-2.0, at 0.7.1 (2026-10-05), released about weekly, and depends on `gpui-pre =0.3.8` ([crates.io](https://crates.io/api/v1/crates/gpui-component)).
- It claims more than 75 components, including:
  - a code editor built on tree-sitter, LSP and ropey, said to be "stable at 200K lines"
  - virtual lists and tables
  - a dock layout
  - AccessKit support
- **Unverified:** these claims come from the README only.

---

## iced

**Maturity and maintenance**

- **Releases.** The latest is 0.14.0 (2025-12-07). Before that came 0.13.0 (2024-09) and 0.12.0 (2024-02), so roughly one release a year ([releases](https://github.com/iced-rs/iced/releases)). `master` is at 0.15.0-dev.
- **Usage.** About 31.7k stars and about 2.9M downloads ([crates.io](https://crates.io/api/v1/crates/iced)).
- **Maintainer.** One person carries the project: hecrj wrote 97 of the last 100 commits, which date from 2026-09-21 to 2026-10-07 (GitHub API).
- **Funding.** The README names Kraken/Cryptowatch as sponsor ([README](https://github.com/iced-rs/iced)). **Unverified:** whether that sponsorship is still current.
- **Stability.** The README says "Iced is currently experimental software". It makes no commitment to 1.0 or to a stable API.

**GPU rendering**

- **Renderers.** iced ships `iced_wgpu` (Metal on macOS) and `iced_tiny_skia`, a software fallback ([Cargo.toml](https://github.com/iced-rs/iced/blob/master/Cargo.toml)).
- **Architecture.** It follows the Elm architecture. Since 0.14 it renders reactively ([0.14.0](https://github.com/iced-rs/iced/releases/tag/0.14.0)).
- **Windowing.** It uses a fork of winit pinned to a fixed revision.

**Text**

- **Shaping.** Shaping uses a fork of cosmic-text, and glyphs are drawn by `cryoglyph` ([Cargo.toml](https://github.com/iced-rs/iced/blob/master/Cargo.toml)). 0.14 added an `Auto` shaping strategy ([CHANGELOG](https://github.com/iced-rs/iced/blob/master/CHANGELOG.md)).
- **Ligatures.** No changelog entry mentions ligatures.
- **LCD antialiasing.** There is none. A readability issue on non-HiDPI displays is still open ([#2254](https://github.com/iced-rs/iced/issues/2254)).
- **The `text_editor` widget.**
  - It is built on cosmic-text's editor, and `iced_highlighter` (syntect) provides highlighting ([source](https://github.com/iced-rs/iced/tree/master/graphics/src/text)).
  - A high-CPU issue in `text_editor` was closed on 2026-09-13 ([#2477](https://github.com/iced-rs/iced/issues/2477)).
  - An issue about regular key presses not being captured is still open ([#3467](https://github.com/iced-rs/iced/issues/3467)).
  - No large-file benchmark has been published.

**IME and input**

- IME support arrived in 0.14 (#2777, with follow-up fixes) ([CHANGELOG](https://github.com/iced-rs/iced/blob/master/CHANGELOG.md)).
- An issue about macOS press-and-hold accents not working is still open ([#3266](https://github.com/iced-rs/iced/issues/3266)).

**Accessibility**

- **Upstream has no AccessKit.** The tracking issue has been open since 2020 ([#552](https://github.com/iced-rs/iced/issues/552)).
- Draft PRs [#1849](https://github.com/iced-rs/iced/pull/1849) and [#3111](https://github.com/iced-rs/iced/pull/3111) are still open.
- A complete external PR was closed unmerged on 2026-03-14 with the reply "I'll work on this myself" ([#3281](https://github.com/iced-rs/iced/pull/3281)).
- System76's `pop-os/iced` fork has an `a11y` feature ([fork Cargo.toml](https://github.com/pop-os/iced/blob/master/Cargo.toml)).

**macOS feel**

- **Menus.** iced has no native menus. A muda PR was closed with "I'd like to try drawing the menus ourselves first" ([#2931](https://github.com/iced-rs/iced/pull/2931)).
- **Widgets.** All widgets are custom-drawn.
- **Scrolling.** Smooth wheel scrolling merged on 2026-09-21 ([#3479](https://github.com/iced-rs/iced/pull/3479)). A kinetic touchpad-scrolling PR is still open ([#3426](https://github.com/iced-rs/iced/pull/3426)).

**Shipped editor**

- **Nothing on upstream iced.**
- **cosmic-edit** (v1.10.0, GPL-3.0-only, a self-described "incomplete pre-alpha") is built on libcosmic, which wraps the **pop-os iced fork** ([cosmic-edit](https://github.com/pop-os/cosmic-edit)). **Unverified:** whether it runs on macOS.
- Otherwise there are only small prototypes and widget crates.

**Docs**

- **API docs.** The workspace sets `missing_docs = "deny"` ([Cargo.toml](https://github.com/iced-rs/iced/blob/master/Cargo.toml)), and there are 56 examples, including an `editor` example ([examples](https://github.com/iced-rs/iced/tree/master/examples)).
- **The book.** It is largely unwritten ([SUMMARY.md](https://github.com/iced-rs/book/blob/master/src/SUMMARY.md)).

**Licence:** MIT ([LICENSE](https://github.com/iced-rs/iced/blob/master/LICENSE)).

---

## Slint

**Maturity and maintenance**

- **Releases.** The latest is v1.18.1 (2026-09-21). Minor releases come every 2–3 months: 1.15 in Feb, 1.16 in Apr, 1.17 in Jun and 1.18 in Sep 2026 ([releases](https://github.com/slint-ui/slint/releases)).
- **Activity.** About 24.1k stars, with 76–173 commits per week over the last 12 weeks.
- **Maintainer.** SixtyFPS GmbH maintains Slint and earns its revenue from commercial licences ([LICENSE.md](https://github.com/slint-ui/slint/blob/master/LICENSE.md)).
- **Stability.** 1.0 shipped on 2023-04-03 and a compatibility-feature mechanism protects the API. `unstable-*` features such as wgpu, winit and fontique are excluded from that guarantee ([CHANGELOG](https://github.com/slint-ui/slint/blob/master/CHANGELOG.md), [Cargo.toml](https://github.com/slint-ui/slint/blob/master/api/rs/slint/Cargo.toml)).

**GPU rendering and UI language**

- **Default renderer.** The default is FemtoVG on OpenGL, plus a software renderer.
- **Other renderers:**
  - Skia (Metal, OpenGL or Vulkan)
  - FemtoVG-wgpu
  - Vello, experimental since 1.18
  - Qt
- **Renderer caveats.** The docs call FemtoVG's text quality "sometimes sub-optimal" and say Skia has a "heavy disk-footprint" ([backends doc](https://github.com/slint-ui/slint/blob/master/docs/astro/src/content/docs/guide/backends-and-renderers/backends_and_renderers.mdx)).
- **UI language.** UIs are written in the declarative `.slint` language, which is either compiled or interpreted, and comes with live preview and an LSP.

**Text**

- **Shaping.** Slint uses Parley 0.11.1, fontique and skrifa ([internal/core Cargo.toml](https://github.com/slint-ui/slint/blob/master/internal/core/Cargo.toml)). Parley shapes with HarfRust.
- **Rasterisation and positioning.** Slint switched to Parley in 1.14. Since 1.16, glyphs are rasterised with swash, and Skia has subpixel glyph positioning ([CHANGELOG](https://github.com/slint-ui/slint/blob/master/CHANGELOG.md)).
- **Ligatures.** Ligatures render, and 1.18 fixed selection colours when a selection boundary falls inside one.
- **LCD antialiasing.** There is none. The issue has been open since 2024-08 ([#5748](https://github.com/slint-ui/slint/issues/5748)).
- **Large documents.**
  - An open issue reports that `TextEdit` "becomes unusable at more than 5K lines"; the reporter wrote their own editor on ropey ([#10087](https://github.com/slint-ui/slint/issues/10087)).
  - 1.18 claims better performance for long texts.

**IME and input**

- Slint takes IME through winit 0.30. 1.18 fixed the IME position not updating when a focused input moves.
- **Still open:**
  - A composing character is dropped on focus loss (reported on Windows) ([#10861](https://github.com/slint-ui/slint/issues/10861)).
  - IME is not part of the public Platform API ([#3811](https://github.com/slint-ui/slint/issues/3811)).
- **Unverified:** I found no macOS-specific IME evidence either way.

**Accessibility**

- AccessKit 0.24 is on by default. 1.18 exposes text-input content and selection to assistive technology.
- **Open macOS VoiceOver issues:**
  - List-item rows are silent ([#12644](https://github.com/slint-ui/slint/issues/12644)).
  - Accessibility focus is lost beyond the visible part of a virtualised `ListView` ([#12645](https://github.com/slint-ui/slint/issues/12645)).

**macOS feel**

- **Menus.** The `MenuBar` element maps to the native macOS menu bar through muda 0.21, and context menus are native on macOS since 1.16 ([Window docs](https://docs.slint.dev/latest/docs/slint/reference/window/window/), [CHANGELOG](https://github.com/slint-ui/slint/blob/master/CHANGELOG.md)).
- **Gestures.** A pinch gesture handler arrived in 1.16. **Unverified:** whether the macOS trackpad feeds it.
- **Style.** The `cupertino` style emulates macOS ([style docs](https://github.com/slint-ui/slint/blob/master/docs/astro/src/content/docs/reference/std-widgets/style.mdx)). The docs and the changelog disagree about the default style on macOS.

**Shipped editor:** none found.

**Docs:** extensive ([docs.slint.dev](https://docs.slint.dev/)): guides, a language reference, a tutorial, and API docs for multiple languages. docs.rs reports 100% coverage.

**Licence**

- The SPDX expression is `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0` ([crates.io](https://crates.io/api/v1/crates/slint)).
- **Royalty-free 2.0** ([text](https://github.com/slint-ui/slint/blob/master/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md)):
  - It allows proprietary desktop, mobile and web apps.
  - Attribution is required, in one of two ways: show the `AboutSlint` widget in an About screen or splash screen, or show a "Made with Slint" badge on a public web page.
  - It excludes embedded systems.
  - It forbids distributing an application that exposes Slint's APIs.
- **Commercial tiers.** Commercial licences are sold in tiers ([pricing](https://slint.dev/pricing)).

---

## Makepad

**Maturity and maintenance**

- **Releases.** The latest crates.io release of `makepad-widgets` is 1.0.0 (2025-05-13) ([crates.io](https://crates.io/api/v1/crates/makepad-widgets)). The default branch `dev` is at 2.0.0 and unpublished.
- **Activity.** About 7.2k stars, with 5–162 commits per week. Recent commits come from "Admin" and Kevin Boos ([commits](https://github.com/makepad/makepad/commits/dev)).
- **Maintainer.** Copyright is held by Makepad B.V. **Unverified:** how the project is funded.
- **Scope.** The README now pitches Makepad as an "AI-accelerated application and game development environment". The workspace also holds many unrelated apps ([README](https://github.com/makepad/makepad/blob/dev/README.md)).
- **Stability.** No API-stability policy was found.

**GPU rendering and UI language**

- **Platform layer.** Makepad has its own platform layer, with no winit. On macOS it uses its own Metal backend ([apple](https://github.com/makepad/makepad/tree/dev/platform/src/os/apple)).
- **UI language.** UIs are written in a live-editable DSL / script engine ("Splash").

**Text**

- **Shaping.** Shaping uses a vendored rustybuzz, which covers GSUB ligatures, BiDi and font fallback ([shaper.rs](https://github.com/makepad/makepad/blob/dev/draw/src/text/shaper.rs)).
- **Glyph rendering.** Glyphs go into SDF and MSDF atlases, and very large text uses a "Slug" GPU curve path ([rasterizer.rs](https://github.com/makepad/makepad/blob/dev/draw/src/text/rasterizer.rs)).
- **CoreText.** CoreText is only an outline fallback ([coretext.rs](https://github.com/makepad/makepad/blob/dev/draw/src/text/coretext.rs)).
- **LCD antialiasing.** None was found.
- **Large documents.** Director keeps up to 32 files of at most 2 MiB each open ([director README](https://github.com/makepad/makepad/blob/dev/apps/director/README.md)).

**IME and input**

- There is a dedicated macOS composition handler, with tests for Korean commit, Escape cancel and preedit backspace ([macos_ime.rs](https://github.com/makepad/makepad/blob/dev/platform/src/os/apple/macos/macos_ime.rs)).
- IME fixes landed in 2026-06 and 2026-09.

**Accessibility:** effectively absent.

- `AccessibilityUpdate` is a no-op in every OS backend ([macos.rs](https://github.com/makepad/makepad/blob/dev/platform/src/os/apple/macos/macos.rs)).
- There is no AccessKit dependency.
- The issue "On Accessibility" has been open since 2023 ([#196](https://github.com/makepad/makepad/issues/196)).

**macOS feel**

- **Menus.** Makepad uses a native NSMenu main menu ([macos_app.rs](https://github.com/makepad/makepad/blob/dev/platform/src/os/apple/macos/macos_app.rs)).
- **Trackpad.** Pinch works through `magnifyWithEvent:`.
- **Widgets.** All widgets are custom-drawn, and there is no native style.

**Shipped editor:** yes.

- The `makepad-code-editor` crate powers Makepad Studio, now called "Director". Director has code editors, terminals and docking ([code_editor](https://github.com/makepad/makepad/tree/dev/code_editor)).
- Robrix, a Matrix client, is a third-party app ([robrix](https://github.com/project-robius/robrix)).

**Docs:** weak.

- docs.rs documents 5.92% of items ([docs.rs](https://docs.rs/crate/makepad-widgets/latest)).
- The repo's `docs/` folder holds only agent notes.

**Licence:** the root LICENSE is MIT ([LICENSE](https://github.com/makepad/makepad/blob/dev/LICENSE)). Crate metadata says `MIT OR Apache-2.0`, but the repo root has no Apache licence file.

---

## Floem

**Maturity and maintenance**

- **Releases.** The latest crates.io release is 0.2.0 (2024-11-14), about 23 months ago ([crates.io](https://crates.io/api/v1/crates/floem)).
- **`main`.** It depends on git crates, including a winit fork, so it cannot be published as-is. The maintainer recommends `main` over 0.2.0 and calls 1.0 "unlikely… probably not in the next 2-4 years" ([#1050](https://github.com/lapce/floem/issues/1050)).
- **Activity.** Only 5 commits have landed on `main` since 2026-04-09.
- **A rework that did not land.** The maintainer's large rendering rework ([#1075](https://github.com/lapce/floem/pull/1075)) was closed unmerged on 2026-10-08.
- **Usage.** About 4.3k stars.
- **Funding.** No funding statement was found.
- **Stability.** The README says Floem is "still maturing" and that occasional breaking changes will come ([README](https://github.com/lapce/floem)).

**GPU rendering**

- **Renderers.** The default is wgpu with vger. vello and Skia are optional, and tiny-skia is the CPU fallback ([Cargo.toml](https://github.com/lapce/floem/blob/main/Cargo.toml)).
- **Architecture.** Floem uses fine-grained signals, and Taffy does the layout.

**Text**

- **Shaping.** 0.2.0 uses cosmic-text 0.12. On `main` it was replaced by Parley 0.7 (HarfRust), merged on 2026-03-05 ([#1034](https://github.com/lapce/floem/pull/1034)).
- **Ligatures.** Ligature fixes landed for the editor and for the caret ([#939](https://github.com/lapce/floem/pull/939), [#922](https://github.com/lapce/floem/pull/922)).
- **LCD antialiasing.** There is none. The request has been open since 2024 ([#381](https://github.com/lapce/floem/issues/381)).
- **Large documents.** There is an editor view (`floem-editor-core` with `lapce-xi-rope`), which Lapce uses.

**IME and input**

- IME goes through the winit fork. An IME batch landed in Oct 2025 ([#933](https://github.com/lapce/floem/pull/933), [#935](https://github.com/lapce/floem/pull/935)).
- **Still open:**
  - The editor cannot delete text through the IME ([#1024](https://github.com/lapce/floem/issues/1024)).
  - A panic when no IME is available ([#1088](https://github.com/lapce/floem/pull/1088)).
- **Unverified:** how well macOS composition works.

**Accessibility:** none on `main`. The issue has been open since 2023 ([#8](https://github.com/lapce/floem/issues/8)), and the draft PR has been stale since 2025-11 ([#973](https://github.com/lapce/floem/pull/973)).

**macOS feel**

- **Menus.** Native menus and context menus come through muda 0.17 ([menu.rs](https://github.com/lapce/floem/blob/main/src/platform/menu.rs)).
- **Trackpad.** Pinch events exist.
- **Widgets.** All widgets are custom-drawn.
- **Forks.** One third party building a macOS app keeps a fork with crash and event fixes ([#1090](https://github.com/lapce/floem/issues/1090)).

**Shipped editor: Lapce.**

- Lapce has about 38.9k stars and is Apache-2.0.
- Its latest stable release is v0.4.6 (2026-01-21), still pre-1.0, and a nightly was published on 2026-10-09 ([releases](https://github.com/lapce/lapce/releases)).
- It had no commits from April to August 2026.
- It pins a Floem git revision from 2026-03 ([Cargo.toml](https://github.com/lapce/lapce/blob/master/Cargo.toml)).

**Docs:** docs.rs covers only 0.2.0. [docs.floem.dev](https://docs.floem.dev/) uses older API names than `main`.

**Licence:** MIT ([Cargo.toml](https://github.com/lapce/floem/blob/main/Cargo.toml)).

---

## egui

**Maturity and maintenance**

- **Releases.** The latest is 0.36.2 (2026-09-08). A minor release comes every 2–3 months: 0.34 in Mar, 0.35 in Jun and 0.36 in Aug 2026 ([releases](https://github.com/emilk/egui/releases)).
- **Usage.** About 31.1k stars and about 25.7M downloads ([crates.io](https://crates.io/api/v1/crates/egui)).
- **Maintainers.** emilk maintains egui and several other contributors are active. Rerun sponsors it ([README](https://github.com/emilk/egui)).
- **Stability.** The README says the "interfaces are still in flux" and that new releases will break things.

**GPU rendering**

- **Mode.** egui is immediate mode. Its own README calls layout "a fundamental shortcoming" of that model and says large scroll areas can be slow ([README](https://github.com/emilk/egui)).
- **Renderers and windowing.** eframe renders with wgpu (Metal) by default, or with glow. Windowing uses upstream winit 0.30 ([eframe Cargo.toml](https://github.com/emilk/egui/blob/main/crates/eframe/Cargo.toml)).

**Text**

- **The text stack in 2026:**
  - 0.34 moved rasterisation to skrifa and vello_cpu, with hinting.
  - 0.35 switched shaping to harfrust "for better kerning and ligatures" and added subpixel binning ([CHANGELOG](https://github.com/emilk/egui/blob/main/CHANGELOG.md)).
- **Fonts and emoji.** System-font fallback and colour emoji are merged but not yet released ([#8492](https://github.com/emilk/egui/pull/8492)).
- **LCD antialiasing.** There is none. The issue has been open since 2022 ([#2354](https://github.com/emilk/egui/issues/2354)).
- **Large documents.**
  - Text selection got faster for large documents in 0.34.2.
  - `CompletionPopup` is merged but not yet released ([#8529](https://github.com/emilk/egui/pull/8529)).
  - No large-file benchmark has been published.

**IME and input**

- 0.35 reworked how composition looks, and 0.34 fixed a macOS backspace bug in the IME ([CHANGELOG](https://github.com/emilk/egui/blob/main/CHANGELOG.md)).
- **Still open:**
  - The Korean hanja candidate window does not open with Option+Return ([#7974](https://github.com/emilk/egui/issues/7974)).
  - The emoji/character viewer is not supported ([#2359](https://github.com/emilk/egui/issues/2359)).

**Accessibility:** AccessKit has been always on since 0.34, and eframe enables native macOS accessibility by default ([eframe Cargo.toml](https://github.com/emilk/egui/blob/main/crates/eframe/Cargo.toml)).

**macOS feel**

- **Menus.** egui has no native menu bar; the issue has been open since 2023 ([#3411](https://github.com/emilk/egui/issues/3411)). Menus are custom-drawn.
- **Window chrome.** eframe can position the traffic-light buttons ([macos.rs](https://github.com/emilk/egui/blob/main/crates/eframe/src/native/macos.rs)).
- **Scrolling.** Kinetic scrolling was reworked on 2026-10-04 ([#8662](https://github.com/emilk/egui/pull/8662)).

**Shipped editor:** no IDE.

- Rerun Viewer is a data viewer.
- Ferrite is a Markdown/config text editor ([repo](https://github.com/OlaProeis/Ferrite)).
- `egui_code_editor` is a small widget.

**Docs:** docs.rs API docs, a live web demo ([egui.rs](https://www.egui.rs/#demo)), 26 examples and a template. There is no book.

**Licence:** MIT OR Apache-2.0 ([Cargo.toml](https://github.com/emilk/egui/blob/main/Cargo.toml)).

---

## Xilem / Masonry

**Maturity and maintenance**

- **Releases.** The latest is 0.4.0 (2025-10-29). Before that came 0.3 (2025-05) and 0.1 (2024-05), and nothing has been released in 11+ months ([releases](https://github.com/linebender/xilem/releases)).
- **Stability.** The README calls Xilem "An experimental Rust architecture", and the 0.4.0 notes call it "alpha-quality software" ([v0.4.0](https://github.com/linebender/xilem/releases/tag/v0.4.0)).
- **Activity.** About 86 commits since 2026-04-09, falling to 1–10 a month after May. The last commit was on 2026-09-14.
- **Usage.** About 5.6k stars.
- **Funding.** The Linebender blog notes that two core members took jobs at Canva ([tmil-25](https://linebender.org/blog/tmil-25/)). **Unverified:** no explicit funding statement was found.

**GPU rendering**

- **Renderer.** The default is Vello 0.8 on wgpu 28. Vello Hybrid ("roughly beta quality") and Skia are optional ([#1847](https://github.com/linebender/xilem/pull/1847)).
- **Architecture.** A Xilem view tree is reconciled into Masonry's retained widget tree ([README](https://github.com/linebender/xilem)).

**Text**

- **Shaping.** Xilem uses Parley 0.8 (HarfRust) with fontique, which lags Parley's own 0.11.1 ([Cargo.toml](https://github.com/linebender/xilem/blob/main/Cargo.toml)).
- **Hinting and LCD antialiasing.** Vello renders glyphs as vectors. The issue tracking text rendering says it will initially support neither hinting nor RGB subpixel rendering, and that issue is still open ([vello#204](https://github.com/linebender/vello/issues/204)).
- **Text area.** `TextArea` wraps Parley's `PlainEditor` ([text_area.rs](https://github.com/linebender/xilem/blob/main/masonry/src/widgets/text_area.rs)). The goal of a "first-in-class text editing widget" is still open ([#388](https://github.com/linebender/xilem/issues/388)).
- **Unverified:** ligatures and large-document performance in Masonry.

**IME and input**

- Xilem uses upstream winit 0.30 plus ui-events. IME fixes landed in 2024–2026, including composition jitter in Apr 2026 ([#1716](https://github.com/linebender/xilem/pull/1716)).
- **Unverified:** how well macOS IME works.

**Accessibility:** AccessKit is integrated, and Parley's `accesskit` feature is enabled ([Cargo.toml](https://github.com/linebender/xilem/blob/main/Cargo.toml)). **Unverified:** how well VoiceOver works.

**macOS feel**

- **Menus.** Xilem has no native menus; menus are planned as in-app layers ([#1343](https://github.com/linebender/xilem/issues/1343)).
- **Trackpad.** Pinch is an open request ([#1375](https://github.com/linebender/xilem/issues/1375)).
- **Widgets.** All widgets are custom-drawn.

**Shipped editor:** none found. The apps highlighted are a port of the Runebender font editor and the early Mastodon client Placehero ([tmil-25](https://linebender.org/blog/tmil-25/)).

**Docs:** thin. The project itself says "Lack of documentation is one of the problems new users complain about the most" ([#392](https://github.com/linebender/xilem/issues/392)), and the book PR was closed unmerged ([#1522](https://github.com/linebender/xilem/pull/1522)).

**Licence:** Apache-2.0 ([Cargo.toml](https://github.com/linebender/xilem/blob/main/Cargo.toml)).

---

## Cross-cutting observations (facts, not a recommendation)

- **Proven code editors.** Three frameworks have one:
  - GPUI: Zed, 1.x, weekly releases.
  - Floem: Lapce, pre-1.0, stable releases rare.
  - Makepad: Studio/Director.

  gpui-component also offers a third-party editor widget on GPUI.
- **macOS IME, at the `NSTextInputClient` level.** GPUI and Makepad implement it directly, without winit. The others depend on winit's IME, or iced's and Floem's forks of winit.
- **AccessKit.** GPUI (since 2026-05), Slint, egui and Xilem have it. iced (upstream), Floem and Makepad do not.
- **Native NSMenu menu bar.** GPUI, Slint, Makepad and Floem have one. iced, egui and Xilem draw their own menus or have none.
- **LCD antialiasing.** No framework does it. Grayscale antialiasing is the norm.
- **Stable APIs.** Only Slint promises one (1.x). Every other framework says it is pre-1.0 or experimental.
- **Publishing.** GPUI's and Floem's current code is effectively consumed from git rather than from crates.io.
- **Permissive licences.** These are permissively licensed: GPUI, iced, egui, Floem, Xilem/Masonry and Makepad.
  - Slint's royalty-free licence attaches attribution conditions to proprietary use.
  - Zed's editor, rope and `ui` crates are GPL. Only the GPUI framework crates themselves are Apache-2.0.

## Not verified

- How well VoiceOver works in practice, for every framework.
- Momentum scrolling in GPUI.
- macOS IME quality for Slint, Floem and Xilem.
- Large-document performance claims (gpui-component's "200K lines", Slint 1.18's "long texts").
- Whether cosmic-edit runs on macOS.
- Whether Kraken still sponsors iced.
- Who funds Makepad, Floem and Linebender.
