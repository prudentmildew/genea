# Architecture

How Genea's code is laid out, and the rules every ticket keeps. The *why* is
in spec #19 (Implementation Decisions → Architecture, Testing Decisions) and
ADR 0004. Terms follow `GLOSSARY.md`.

## Crates

```
crates/
  genea-core/     framework-free core; the Workbench is its single entry point
  genea-host/     the host boundary: processes, downloads, the clock; RealHost
  genea-testkit/  TestHost (manual clock, scripted processes/downloads) + FixtureProject
  genea-toolchain/ toolchain pins, version resolution, the shared store (ADR 0005)
  genea-view/     the thin Slint view layer and the `genea` binary
patches/          the one Slint patch (ADR 0004); see patches/README.md
scripts/          reapply-slint-patch.sh
```

Dependencies point one way: `genea-view → genea-core → genea-host`.
`genea-testkit` depends on `genea-host` only (never on `genea-core`), and
`genea-core` uses it as a dev-dependency.

### The core is framework-free

No core crate depends on Slint, winit, muda, Skia, wgpu or AppKit, directly
or transitively. `crates/genea-core/tests/no_gui_dependency.rs` checks the
dependency tree. **When you add a core crate, add it to `CORE_CRATES` in that
test.** Only `genea-view` touches Slint. There is no GUI abstraction layer:
switching frameworks means rewriting the views, not the core.

New core areas (syntax, LSP client, git, review, toolchain, terminal, …) can
start as modules of `genea-core` and move into their own `genea-*` crates once
they grow. Either way, they are reached only through the workbench.

## The core API

`genea_core::Workbench` is the seam that both the Slint view and the tests
use, and nothing else:

| Part | API | Notes |
|---|---|---|
| Projects | `open_project(root) -> ProjectId`, `close_project(id)`, `projects()` | Opening an already-open folder returns its id (the app focuses its window). Opening puts the folder first in the recent projects. |
| Welcome | `welcome() -> Option<WelcomeView>` | `Some` exactly while no project is open; lists the recent projects, most recent first (kept in `recent-projects.txt` in the application-support folder, saved in the background 1 s after a change). |
| Commands in | `dispatch(id, Command)` | `Command` (`src/command.rs`) is plain data, one variant per user action. Commands apply synchronously on the main thread. |
| View state out | `project(id) -> Option<ProjectView>` | Plain snapshots (`src/view.rs`) of what the window shows: user-visible text, 1-based labels, display columns. |
| Change notification | `set_notifier(Fn() + Send + Sync)` | Called from any thread when background work has finished. The app then calls `pump()` on the main thread and re-reads view state. |
| Waiting | `pump() -> bool`, `settle()` | `pump` applies finished work without waiting. `settle` waits until nothing is pending (tests). |
| Templates | `create_project(NewProject)`, `project_creation() -> Option<ProjectCreation>` | Generates in the background (`src/templates/`, files in `crates/genea-core/templates/`). Not tied to an open project. The slow lane is `tests/templates_slow.rs` (`-- --ignored`). |
| Processes | `spawn(id, ProcessSpec) -> io::Result<Child>` | Starts a process in the project environment (below), in the project root unless the spec names a folder. Tests use it to see what the project's processes get. |
| Update check | `start_update_checks(version)`, `update_notice() -> Option<UpdateNotice>` | At most daily, 10 s after start, on a background job: GitHub's latest release (`RELEASES_URL`) through `downloads()`. Its last time (on the clock's `system_time()`) and result are kept in `update-check.json` in the application-support folder. The notice is app-wide; every window shows it. |

### Extending it

- **A new user action**: add a `Command` variant, handle it in
  `Project::dispatch` (`src/project.rs`), and add what the user sees to the
  view-state structs.
- **Background work**: never block the main thread. Use `Jobs::spawn` (in
  `src/jobs.rs`). The closure runs on a background thread and returns an
  `Apply`, a `FnOnce(&mut Core)` that changes state on the main thread.
  Look a project up by id in the Apply (`core.project_mut(id)`): it may have
  been closed in the meantime. Use a generation counter to drop stale results.
- **Long-lived background sources** (a watcher, an LSP reader thread) hold a
  `Jobs::busy()` token while they are processing something, and `finish` it
  with their Apply. That is how `settle` knows the watcher is drained.
- **Effects outside the process** go through the host (`core.host`), never
  `std::process`, an HTTP client or `std::time` directly. The filesystem is
  used directly.
- **Tabs and the split** (`src/project/tabs.rs`): an open file has one
  `Editor` however many tabs show it; each tab keeps its own `Cursor`
  (caret, selection, scroll). `Project::editor` is always the *focused*
  tab's editor, so commands that act on "the open file" keep using it. The
  other open files are parked; reach any open file by path with
  `Project::open_editor(_mut)`, e.g. in an Apply whose file may have lost
  the focus meanwhile. `ProjectView::editor` is the focused file;
  `ProjectView::panes` lists each side's tabs and editor.
- **Carets** (`src/editor.rs`, ticket #52): an editor has one or more
  carets, each with its own selection, in the order they were added; the
  last is the primary (the one scrolled to and shown in the status bar).
  Every edit goes through `Editor::replace`, which applies it at every caret
  as one undo edit; a new edit command should too. A tab's `Cursor` holds
  the whole caret list. `EditorView::caret` is the primary,
  `EditorView::carets` every caret on the visible lines.

### Syntax

`src/syntax/` (ticket #24) holds an open file's tree-sitter tree and its
highlight spans. Every change to a buffer's text (`Editor::replace`, undo
and redo, all through `Editor::apply`) ends in `Editor::splice`, which
applies it to the main thread's tree (`Tree::edit`, positions only) and
shifts the spans, so a keystroke never waits on a parse. Don't edit the
rope anywhere else. One background parse per file runs at a time
(`Project::reparse`, `spawn_parse`): it reparses incrementally and
recomputes the highlights, edits made meanwhile are replayed onto its result,
and the next parse starts if the text moved on. Files over 5 MB get no syntax.

- View state: `VisibleLine::highlights`, a list of `HighlightSpan`
  (display columns + `genea_core::Highlight`). The view colours each
  `Highlight` from the light or dark palette in `ui/theme.slint`
  (`highlight_index` in `src/surface.rs` maps them; keep the two in step).
- Languages, grammars and queries: `syntax/language.rs`. Embedded languages
  (HTML `<script>`/`<style>`, Markdown inline and fenced code) come from the
  grammars' injection queries. `.env` has no grammar: `syntax/dotenv.rs`.
- Structural editing reads `Syntax::tree()`. Semantic highlighting layers its
  tokens over `Syntax::spans()` in `Editor::grid_line`.

## The host boundary

`genea_host::Host` provides `clock()`, `processes()`, `downloads()`,
`clipboard()` (below), `launch_environment()` (the variables Genea was
started with; `SHELL` names the login shell), and `support_dir()`, Genea's application-support
folder, where the core keeps its own files (recent projects, the toolchain
store; session state, review baselines, …). The folder is used through the
real filesystem; the host only says where it is, so tests never touch the
user's. `Downloads::fetch_with_length` also reports the response's
`Content-Length`, for progress.

- `RealHost`: the monotonic clock with one lazily started timer thread (so no
  idle wake-ups), `std::process`, and HTTP through `ureq` on the system TLS
  stack.
- The clipboard is the one effect the core uses on the main thread (Cut,
  Copy and Paste are synchronous edits). The pasteboard lives in AppKit,
  which no core crate may link, so `genea-view` supplies it through
  `RealHost::with_clipboard` (`src/pasteboard.rs`).
- `genea_testkit::TestHost`: `ManualClock::advance` fires due timers;
  `ScriptedProcesses::script("tsc", |spec, io| …)` plays a program on a
  thread with real pipes (unscripted programs fail with `NotFound`) and
  records every spawn; `ScriptedProcesses::script_shell("zsh", vars)` plays
  a login shell with a real `/bin/sh` whose environment is exactly `vars`;
  `TestHost::set_launch_environment` sets the launch environment (by default
  only a `PATH`, with no `SHELL`, so no login shell runs);
  `ScriptedDownloads::serve/fail` answers URLs from a
  table (unknown URLs answer 404) and records every request. Each test host
  has its own temp support folder; a second workbench on a clone of the same
  host is a restart.
  Once started, `TestHost::download_server()` (the **local download fixture
  server**, a real HTTP server on 127.0.0.1) answers every URL not in the
  table, over real HTTP. Tests publish files under the real URLs
  (`server.publish(url, bytes)`), `hold`/`release` a response halfway to
  look at Genea mid-download, and `TestHost::tools()` publishes fake Node,
  Bun and pnpm releases (indexes, checksums, archives; `*_with_bad_checksum`
  for a mismatch).

A new kind of effect (a PTY, say) gets a trait in `genea-host`, an accessor on
`Host`, a real implementation in `genea-host/src/real.rs` and a scripted one
in `genea-testkit/src/host.rs`. Keep the traits small and blocking: the core
calls them from background threads.

## The toolchain

`genea-toolchain` (blocking, no threads) reads and writes the pins in
`package.json` (`pins`), resolves a `Request` (newest match in the store,
else newest published), and installs into the `Store` at
`<support_dir>/toolchains/<tool>/<version>/`; `Installed::bin_dir` is the
folder to put on PATH (`bin/` for Node and Bun, the unpacked `@pnpm/exe`
package itself for pnpm). `genea-core/src/toolchain.rs` runs it per project: it reads the pins
in a job when a project with a root `package.json` opens, starts one job per
role, and exposes `ProjectView.toolchain`, `StatusBar.toolchain` and notices
whose `NoticeAction` carries the `Command` a click dispatches.

## The project environment

Every process Genea starts for a project (terminal shells, scripts,
language servers, the project check) gets the project environment
(`genea-core/src/environment.rs`, ticket #36, ADR 0005): the variables of the
user's login shell, captured once per open by running `$SHELL -l -i -c` in
the project root, with each `Installed::bin_dir` of the toolchain first on
PATH (worked out at spawn time, so a download that finishes later counts).
If the shell fails or takes longer than `LOGIN_SHELL_TIMEOUT` (5 s, host
clock), processes get the launch environment and a notice offers
`Command::ReloadEnvironment`, which also sits in the File menu. A process
started before the capture lands gets the launch environment.

**Starting a process from the core**: never build a `ProcessSpec` for a
project process without it. Take `Project::process_env()` (a `Send`
snapshot, fine to move into a job) and pass the spec through
`ProcessEnv::apply(spec)`, then spawn it through `host.processes()` (or a
PTY). `apply` sets `clear_env`, puts the project's variables first (the
spec's own `env` entries win, e.g. `TERM`), and defaults `cwd` to the root.
The slow lane `tests/environment_slow.rs` (`-- --ignored`) runs the real
login shell.

## Tests

Behaviour is tested only through the core API (spec #19, Testing Decisions).
Integration tests live in `crates/genea-core/tests/`, **one file per area**
(`open_file.rs`, `scrolling.rs`, `caret.rs`, `projects.rs`, …), so tickets
running in parallel add files rather than edit the same one. A test:

```rust
let fixture = FixtureProject::new().file("src/main.ts", "let a = 1;\n").build();
let host = TestHost::new();                 // keep a clone to script the host
let mut workbench = Workbench::new(host.shared());
let project = workbench.open_project(fixture.root()).unwrap();
workbench.dispatch(project, Command::OpenFile("src/main.ts".into()));
workbench.settle().unwrap();                // never sleep
assert_eq!(workbench.project(project).unwrap().editor.unwrap().lines[0].text, "let a = 1;");
```

It asserts on what a user would see (view state, files on disk via
`fixture.read`), never on internals. Timers are not background work: with
the test host, call `host.clock().advance(..)` and then `settle()`. The view
layer has no automated tests. It stays thin enough that the core tests carry
the behaviour, and the benchmark harness covers the rest.

## The view layer

`crates/genea-view` (ADR 0004): Slint `=1.18.1`, winit + Skia over
wgpu/Metal (chosen in code by `BackendSelector`, not env vars), `.slint`
chrome, native menus via muda (Slint's `MenuBar`).

- `ui/*.slint`: one component per file; `app.slint` exports what Rust
  instantiates. `theme.slint` holds colours, fonts and metrics.
- `src/app.rs`: the `App` (workbench + windows) in a main-thread
  `thread_local`, reached through `with_app`; callback wiring; the notifier,
  which coalesces wake-ups into one `pump` per event-loop turn. One window
  per open project: `App::open_project(from, folder)` opens a folder in a new
  window (or focuses its existing one). The welcome window shows while no
  project is open; closing it quits. The event loop runs with
  `run_event_loop_until_quit`, so closing the last project window doesn't.
- `src/window.rs`: `WindowController::sync`, the only place view state flows
  into Slint. `WindowController::focus` brings a window to the front.
  `src/welcome.rs`: the welcome window's sync.
- `src/surface.rs`: the editor surface, a ring of line slots (line L in slot
  L % slots) with a per-slot diff, plus base-line rebasing for `f32`
  precision.
- `src/fonts.rs`: registers Apple Color Emoji and Hiragino Sans GB (CJK),
  memory-mapped, the first time visible text has an emoji or CJK character
  that Menlo and Apple Symbols lack. It is
  the only user of Slint's `unstable-fontique-011`. Wide characters are
  drawn one per run at their grid column (`genea_core::grid_pieces`), since
  fallback glyphs aren't two Menlo cells wide.
- `src/keys.rs`: the keymap (WebStorm macOS), plus `CloneCaretGesture`
  (press ⌥ twice and hold, then ↑/↓). Secondary carets ride in each line
  slot (`Line.carets`), so they share its diff. `src/dialogs.rs`: native
  NSOpenPanels.
- Keys and text reach the surface through a hidden, focused `TextInput`
  (ADR 0004) whose `key-pressed` accepts every key; IME commits and the
  preedit go to the core as `InsertText` and `SetPreedit`. `src/blink.rs`
  stops that `TextInput`'s cursor-blink timer, which would otherwise repaint
  an idle window twice a second.

Rules: push to Slint only on change. Use no repeating timers (the caret is
steady). Install no rendering notifier or run-loop observer unless the
benchmark journal is on. Idle must be 0 % CPU.

The editor font is Menlo 13 pt with a line height of 1.2 (`Theme` in
`ui/theme.slint` and `LINE_HEIGHT` in `src/surface.rs`; keep them in step).

## Build

```sh
cargo build                  # first build downloads prebuilt Skia
cargo test                   # core API tests + host adapter smoke tests
cargo run --bin genea -- [FOLDER [FILE]]
```

The toolchain is pinned in `rust-toolchain.toml`.

Releases (a signed, notarized DMG for Apple Silicon, macOS 14+) and the
third-party licence list shown in About are in `docs/releasing.md`:
`scripts/release.sh [--local]`, `scripts/third-party-licences.sh`,
`packaging/`.
