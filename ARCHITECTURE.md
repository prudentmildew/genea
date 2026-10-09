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
  genea-bench/    the benchmark harness and the start-floor binary (bench/README.md)
patches/          the one Slint patch (ADR 0004); see patches/README.md
scripts/          reapply-slint-patch.sh, bench.sh
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
| Waiting | `pump() -> bool`, `settle()` | `pump` applies finished work without waiting. `settle` waits until nothing is pending, including the watchers' events for changes already on disk (tests). |
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
- **Files changing on disk**: each project has one `notify` watcher
  (FSEvents) over its whole folder (`src/watcher.rs`). Changes arrive in
  batches at `Project::files_changed(FileChanges, jobs)`; react there (the
  config, the file index and open editors do; review hooks in beside them).
  `settle` waits for the watcher by writing a cookie file into
  `<support>/watch-sync`, which the same FSEvents stream watches: once its
  event is back, every earlier change has been delivered. So a test writes a
  file with `fixture.write(..)`, calls `settle()`, and asserts.
- **Open editors and external changes** (`src/disk.rs`,
  `src/project/external.rs`, ticket #32): each `Editor` keeps the text it
  believes is on disk (read at open, or set when a save *starts*). A changed
  open file is read and compared in the background: the same text is no
  change, which is how Genea's own saves are recognised. Different text
  reloads a clean buffer as one undo step (kind `Other`, a single splice of
  the changed stretch, so carets outside it keep their place) or, with
  unsaved edits, sets `EditorView::conflict` until
  `Command::ResolveConflict` (Reload or Keep my edits) or a save. A result
  whose buffer or disk text moved on meanwhile is checked again. Review
  (#53) should reuse the same "what Genea last wrote" comparison.
- **The file index** (`src/files.rs`, ticket #30): every file and folder
  outside `node_modules` and `.git`, read once in the background at open and
  then kept up to date from the watcher's batches (a changed path re-lists
  its folder; a new folder is read with its contents), one read at a time.
  The Files view's tree (`ProjectView::files`, flattened rows, shared as an
  `Arc` so an unchanged tree costs nothing to snapshot) is built from it on
  the main thread, minus the config's `exclude` (a `.gitignore` matcher
  applied when the rows are built, so `exclude` changes need no disk read).
  The finder should take its file list from here and hide the same paths;
  hidden files still open with `OpenFile`.
- **Project search** (`src/search.rs`, ticket #34): `Command::Search(SearchQuery)`
  cancels the search in flight (a flag, plus a generation that drops its
  late results) and starts a new one. It does not use the file index: it
  walks the disk with `ignore`'s parallel walker, which also applies
  `.gitignore` (the project's own, with or without a git repo; not the
  parents' or the user's global one), skips `node_modules` and `.git`, and
  applies the config's `exclude`. Each file is matched with
  `grep-searcher`/`grep-regex` (binary files are skipped). Files with
  matches stream to the main thread through a channel, one Apply in flight
  at a time, and are inserted in the tree's order (folders first,
  case-insensitive). `ProjectView::search` holds the query, the results
  (`Arc`, like the tree) and the state; a search stops at
  `MAX_SEARCH_MATCHES`. It searches what is on disk, not unsaved edits.
- **Problems** (`src/problems.rs`): every source puts its errors and
  warnings into the project's `Problems` store and owns them. A source that
  reports for the whole project calls `replace(source, problems)`; one that
  reports per file (tsgo, Oxlint) calls `replace_file(source, path,
  problems)`. Add a `ProblemSource` variant for a new source. The Problems
  view (`ProjectView::problems`), the status-bar counts and the editor's
  underlines (`EditorView::problems`) all read from the store. Positions
  are `TextPosition`s: 0-based line and char column.
- **Config** (`src/config.rs`): `genea.jsonc` parses into `Config` (in
  `ProjectView::config`), with defaults for anything missing or wrong. A
  feature reads its key from the project's config; it applies live, so read
  it when needed rather than copying it at open. A new key goes in
  `Config`, its `Default`, and the `match` in `config::parse`.
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
and the next parse starts if the text moved on. Large files get no syntax
(below).

- View state: `VisibleLine::highlights`, a list of `HighlightSpan`
  (display columns + `genea_core::Highlight`). The view colours each
  `Highlight` from the light or dark palette in `ui/theme.slint`
  (`highlight_index` in `src/surface.rs` maps them; keep the two in step).
- Languages, grammars and queries: `syntax/language.rs`. Embedded languages
  (HTML `<script>`/`<style>`, Markdown inline and fenced code) come from the
  grammars' injection queries. `.env` has no grammar: `syntax/dotenv.rs`.
- Structural editing reads `Syntax::tree()`. Semantic highlighting layers its
  tokens over `Syntax::spans()` in `Editor::grid_line`.

### Structural editing

Ticket #25. `syntax/structure.rs` answers questions about the tree with
generic rules, not per-language queries: a node is a *block* when its first
and last children are a bracket pair (`{}`, `[]`, `()`, `${}`) or an
element's tags (HTML, JSX); comments, Markdown sections and code blocks, and
YAML pairs fold too. `editor/structural.rs` turns the answers into edits,
carets and view state:

- Return (`Editor::new_line`) indents one `INDENT_UNIT` deeper inside a
  block and splits a bracket pair. The tree may be a parse behind, so an
  opening bracket at the end of the line (outside strings and comments)
  counts too. #26 replaces `INDENT_UNIT` with the resolved indentation.
- ⌘/ (`ToggleLineComment`) uses `Language::comment`; ⌥↑/⌥↓ walk nodes, with
  the shrink history in the tab's `Cursor`; `EditorView::brackets` holds the
  bracket at the caret and its match.
- Folds (`Editor::folds`) are char positions that move with edits in
  `splice`. Hidden lines take no *row*: `scroll_top`, Up/Down and
  `VisibleLine::row` count rows, while commands and `VisibleLine::index`
  stay in file lines (the view maps a clicked row to its line). A caret
  that lands in hidden lines unfolds them (`reveal_caret`).
- Edits that keep selections where they were (comments) go through
  `edit_text`; edits that put each caret somewhere in its replacement go
  through `replace_placing`.

### Large files

A file over `LARGE_FILE_BYTES` (5 MB, ticket #27) is a *large file*: it
opens with no syntax tree, no highlighting and no language intelligence, and
`StatusBar::large_file` says why. It is decided once, from the size read at
open. **Anything that starts per-file language work (language servers,
semantic tokens, …) checks `Editor::is_large` and skips large files.**

Opening reads in the background (`src/reading.rs`), and the main thread only
swaps the finished rope in. A file that may be large (over 5 MB, or of
unknown size, like a pipe) opens as soon as its first screen of lines is
read: `Editor::loading` shows those lines read-only (`EditorView::loading`),
and `Editor::finish_loading` swaps the whole file in when it is read,
keeping each tab's carets and scroll, which the first screen (a prefix of
the whole text) leaves valid. The benchmark harness's `open-1mb` and
`open-100mb` scenarios measure it.

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

Pins change only through commands (ticket #37). `OpenToolchainPicker(kind)`
lists versions in a job (`genea_toolchain::published` plus what the store
has, so it works offline) into `ProjectView::toolchain_picker`; each
`ToolchainOption` carries the `SetRuntime` / `SetPackageManager` command a
pick dispatches, which writes the exact pin (`pins::write`) and restarts that
role's download. `RemoveUnusedToolchains` is handled by the workbench (it
needs the recent projects): it keeps what each recent or open project's
`package.json` resolves to (exact pins, the newest stored match of a range,
the defaults for an unpinned role) and `Store::remove`s the rest. The
lockfile cross-check (root `pnpm-lock.yaml`, `bun.lock`, `bun.lockb` against
the package-manager role) reports as `ProblemSource::Toolchain` and is
re-run when the watcher sees a root lockfile change.

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
- `src/surface.rs`: the editor surface, a ring of line slots (the line on
  row R in slot R % slots; rows skip folded lines) with a per-slot diff,
  plus base-row rebasing for `f32` precision. A press on a gutter fold
  marker is `ToggleFold`.
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
- Themes: `ui/theme.slint`'s `Theme` global has light and dark colours,
  chosen by `follow-system` (the system's appearance, through
  `Palette.color-scheme`) or `pinned-dark`, which `WindowController::sync`
  sets from the config's `theme`. Use `Theme` colours, never literals. Each
  palette has its syntax colours (`Theme.syntax`); a `Run`'s `highlight`
  indexes them, and 0 is plain text.
- The left column (`ui/left-column.slint`): a view switcher and the active
  view, shown while the core's `left_column` is `Some`. A view's shortcut
  is a menu item that dispatches `ToggleLeftColumn` (Files is ⌘1, and a
  project opens showing it; Search is ⌘⇧F, and focuses its query field
  when it appears; Problems is ⌘6). A new
  view adds a `LeftColumnView` variant in the core, a `LeftView` value, a
  switcher tab and its component.
- Keys and text reach the surface through a hidden, focused `TextInput`
  (ADR 0004) whose `key-pressed` accepts every key; IME commits and the
  preedit go to the core as `InsertText` and `SetPreedit`. `src/blink.rs`
  stops that `TextInput`'s cursor-blink timer, which would otherwise repaint
  an idle window twice a second.
- `src/journal.rs` and `src/remote.rs`: the benchmark harness's
  instrumentation journal and control channel, off unless `GENEA_JOURNAL=1`.
  `journal.rs` is the only user of Slint's `unstable-winit-030` and
  `unstable-wgpu-30`.

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
scripts/bench.sh             # the benchmark harness (bench/README.md)
```

The toolchain is pinned in `rust-toolchain.toml`.

Releases (a signed, notarized DMG for Apple Silicon, macOS 14+) and the
third-party licence list shown in About are in `docs/releasing.md`:
`scripts/release.sh [--local]`, `scripts/third-party-licences.sh`,
`packaging/`.
