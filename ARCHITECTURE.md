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
| Terminal | `ProjectView::terminal` (`TerminalView`, with `tabs` and `active_tab`, then the showing tab's grid; `TerminalLine::links`); `Command::ToggleTerminal`, `FocusTerminal`, `SelectTerminalTab`, `NewTerminalTab`, `CloseTerminalTab`, `OpenTerminalLink`, `SetTerminalSize`, `TerminalText`, `TerminalPreedit`, `TerminalKey`, `TerminalPaste`, `TerminalMouse`, `ScrollTerminal` | Shell tabs, plus a tab per package-manager command (`src/terminal/`, below). |
| Install | `Command::InstallDependencies` | The pinned package manager's `install` in a terminal tab (below). The new-project flow dispatches it right after opening; otherwise only a click does. |
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
  The finder takes its file list from here and hides the same paths;
  hidden files still open with `OpenFile`. `FileIndex::file_list` is that
  list (relative paths, `exclude` applied, cached until
  `FileIndex::version` changes).
- **Git** (`src/git.rs`, ticket #56): read-only, through `gix` on
  background jobs (each read opens the repository with `gix::discover`
  from the root, so a project inside a bigger repository works too). HEAD
  is read at open and again when the watcher sees `.git/HEAD`, `refs/` or
  `packed-refs` change (or `.git` appear), with every open file's text at
  HEAD (its *base*); a newly opened file reads its own. Gutter markers
  (`EditorView::gutter`) come from a line diff (`imara-diff`) of the base
  and a rope snapshot, one job per file at a time, restarted when it lands
  if the buffer's version moved on, like the syntax parse.
  `Command::ShowHunk`/`RollbackHunk` act on the hunks only while they are
  up to date with the buffer; Rollback is an ordinary undoable edit
  (`Editor::replace_lines`). A repository whose git folder is outside the
  project isn't watched. Tests make repositories with the `git` binary
  (`tests/repository.rs`).
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
- **The finder** (`src/finder.rs` for matching, `src/project/finder.rs` for
  the project's side, ticket #33): `Command::OpenFinder(FinderMode)` opens
  the overlay (`ProjectView::finder`); each `SetFinderQuery` starts a
  background job that matches with `nucleo-matcher` (generation counter for
  stale results). After every Apply, `Workbench::run` calls
  `Project::refresh_finder`, which matches again if the file index's version
  moved, so results follow files on disk and `exclude`. Recent files
  (`Project::recent_files`, in memory) are recorded when a file opens or
  its tab is selected. Actions (`src/action.rs`) are the commands a user
  can run by name: a new menu item or shortcut that is a core command gets
  an `Action` variant with its name and shortcut label (keep the labels in
  step with `genea-view`'s menus and `src/keys.rs`). A file chosen takes
  the focus from the terminal, as `OpenFile` does; editing actions
  (`Action::edits`) do what the Edit menu does while the terminal has it.
  Symbols (#47) add a `FinderItemKind` and a mode.
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

- Return (`Editor::new_line`) indents one level of the file's
  indentation (below) deeper inside a block and splits a bracket pair. The
  tree may be a parse behind, so an opening bracket at the end of the line
  (outside strings and comments) counts too.
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

### Indentation

Ticket #26 (`src/indentation.rs`, `editor/indent.rs`). A file's `useTabs`
and `tabWidth` are resolved the way Oxfmt resolves them, with the crates
Oxfmt uses (`editorconfig-parser`, `fast-glob`): `.oxfmtrc.json` (or
`.oxfmtrc.jsonc`) overrides that match the file, in order, over its root
options; then the nearest `.editorconfig`'s matching sections fill in what
is unset (`indent_size` only when indenting with spaces, else `tab_width`);
then 2 spaces. Both files are found from the project root upwards, like
Oxfmt from its working directory, and one Oxfmt would reject counts as
none. Genea never reads Prettier or Biome config and never guesses from a
file's contents.

`IndentationConfig::read` runs in a job when the project opens and again
when the watcher sees a root `.oxfmtrc.json(c)` or `.editorconfig` change
(folders above the root aren't watched). Resolving a file is cheap and
needs no disk, so `Project::indentation_of(path)` is called per keystroke
and per view: open files follow a config edit without a reopen. It drives
`Command::Indent` (Tab), `Command::Outdent` (⇧Tab), Return's auto-indent
and `StatusBar::indentation`. Tabs are still drawn 4 columns wide.

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

`genea_host::Host` provides `clock()`, `processes()`, `ptys()` (programs on
pseudo-terminals: the terminal's shells), `downloads()`,
`clipboard()` (below), `launch_environment()` (the variables Genea was
started with; `SHELL` names the login shell), and `support_dir()`, Genea's application-support
folder, where the core keeps its own files (recent projects, the toolchain
store; session state, review baselines, …). The folder is used through the
real filesystem; the host only says where it is, so tests never touch the
user's. `Downloads::fetch_with_length` also reports the response's
`Content-Length`, for progress.

- `RealHost`: the monotonic clock with one lazily started timer thread (so no
  idle wake-ups), `std::process`, `openpty` sessions (the program leads its
  own session with the pty as its controlling terminal; `hang_up` sends
  SIGHUP to its process group; `wait` reaps it with `waitpid`), and HTTP through `ureq` on the system TLS
  stack.
- The clipboard is the one effect the core uses on the main thread (Cut,
  Copy and Paste are synchronous edits). The pasteboard lives in AppKit,
  which no core crate may link, so `genea-view` supplies it through
  `RealHost::with_clipboard` (`src/pasteboard.rs`).
- `genea_testkit::TestHost`: `ManualClock::advance` fires due timers;
  `ScriptedProcesses::script("tsc", |spec, io| …)` plays a program on a
  thread with real pipes (unscripted programs fail with `NotFound`) and
  records every spawn; `ScriptedPtys` (the **fake PTY**,
  `TestHost::ptys()`) records every terminal started, each a `FakePty` the
  test plays by hand: `output(bytes)` returns once Genea has read them (so
  `settle()` after it shows them on the grid), `wait_for_input(text)` sees
  what was typed, `exit(code)`, `wait_for_hang_up()`, `fail(kind)` for a
  shell that can't start; `ScriptedProcesses::script_shell("zsh", vars)` plays
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

A new kind of effect (as `Ptys` was) gets a trait in `genea-host`, an accessor on
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

A process that must see the final environment waits for it:
`Environment::is_ready` (no capture running) and `Toolchain::is_settled`
(nothing reading, resolving or downloading), together
`Project::environment_ready`. The terminal's shell starts through
`Project::start_terminal_when_ready`, which the workbench calls after
opening, after every command and after every background result.

## The terminal

`genea-core/src/terminal/` (tickets #38, #39, #41): a pane of tabs. The
first is a shell, the user's `$SHELL -l` in the project root;
`NewTerminalTab` opens more shells, and others run a command
(`Launch::PackageManager`, ticket #41). Every tab's program gets
the project environment, `TERM=xterm-256color` and `COLORTERM=truecolor`,
on a PTY from `host.ptys()`, emulated by `alacritty_terminal` (only its
`Term` and the `vte` parser; Genea runs the PTY itself). Scrollback is
10,000 lines. Commands act on the showing tab (`TerminalView::active_tab`);
the size and focus are the pane's. A tab's programs start when
`Project::start_terminal_when_ready` sees the environment ready, and each
start takes a pane-wide generation, by which its session's Applies find
their tab. Return restarts an ended shell; a command's tab stays as it
ended until it is run again. `CloseTerminalTab` drops the tab's session,
which hangs up its program; closing the last tab collapses the pane, and
showing it again opens a new shell (so the pane may have no tabs: the
showing tab is `Terminal::tab() -> Option`).

- **Install dependencies** (ticket #41, ADR 0005): `src/dependencies.rs`
  stats the root `node_modules` at open and whenever the watcher reports a
  path at or under it. While it is missing (and the package manager is one
  Genea runs, and no install is under way), a notice offers
  `Command::InstallDependencies`, also in the File menu. It runs the
  pinned package manager's executable from the store
  (`Toolchain::package_manager_program`) with `install` in its own tab
  (`Terminal::run_package_manager`): a running install is just shown, an
  ended one runs again in its tab. Dispatched before `package.json` is
  read (the new-project flow), it waits for it. #40's scripts can run the
  same way, with their own arguments and folder.

- Links (`links.rs`): `path:line[:col]` references (the last path part
  has an extension; `file://` paths too; not after another `:`, so URLs
  and addresses aren't) are found when a grid is copied, across rows that
  were wrapped, into `TerminalLine::links` (grid columns, the path as
  printed, a 0-based `TextPosition`). `OpenTerminalLink { line, column }`
  resolves the one under the cell against the tab's `directory` (lexically,
  no disk access) and opens it like `OpenFileAt`, relative to the root if
  it is inside it.
- Threads (per tab): a reader thread reads the PTY and parses into the
  `Term` under a mutex, at most 16 KB per lock hold; a writer thread
  writes input and resizes. Answers the emulator writes back (`Event::PtyWrite`) go to the
  writer; requests (title, OSC 52 copy) wait for an Apply.
- The grid is copied into view state lazily (`Tab::screen`, a
  `RefCell`): only when view state is read after the emulator changed. The
  reader wakes the main thread only once the last copy has been taken, so a
  flood is copied at most once per read. The app reads at most once a frame
  for background work (`App::pump` in genea-view).
- `grid.rs` turns cells into `TerminalLine`s (text in grid columns, runs of
  one `TerminalStyle`, inverse and hidden applied; ANSI 0–15 and the default
  colours stay symbolic for the theme, 256-colour and 24-bit are RGB).
  `input.rs` encodes keys, mouse reports (SGR, xterm, UTF-8), the wheel and
  pastes for the modes the program set.
- Synchronized updates (mode 2026) are applied as they arrive.
- The view: `ui/terminal-pane.slint` and `src/terminal.rs` (a tab strip
  with close buttons and +, pushed when the tabs change; a slot per visible
  row, pushed only when it changed, with references underlined; the palette
  is `Theme.terminal-palette`). The pane sits right of the editor area, or
  below it (and the left column) with `terminalPosition: "bottom"`: in
  `project-window.slint` it is placed by hand, and a placeholder of its
  size in the editor area's layout keeps that clear. Its width and height
  are view-only state, dragged at the splitter (clamped while dragging:
  bound to the area's size, the placeholder would be a binding loop). ⌘T is View › New Terminal Tab; with the
  terminal focused, ⌘W and ⌘⇧[ / ⌘⇧] act on its tabs. ⌘-click opens an
  OSC 8 hyperlink in the browser, else dispatches `OpenTerminalLink`.

## Language servers

`genea-core/src/lsp/` (ticket #42, ADR 0002) is the LSP client:
hand-rolled JSON-RPC over stdio with `gen-lsp-types` (LSP 3.18) for the
message types. `project/language.rs` is a project's side of it.

- **tsgo**: the platform package's `lib/tsc`
  (`@typescript/typescript-darwin-arm64`, beside the `typescript` package,
  which may be in pnpm's store or under an alias like vscode's
  `@typescript/native`), found in the background when the project opens
  and again when `package.json` or the top of `node_modules` changes. It
  runs as `tsc --lsp --stdio` in the project environment, one per project,
  with the root as its only workspace folder, and stops when the project
  closes (`shutdown`, `exit`, then a kill after `STOP_TIMEOUT`). Without
  TypeScript 7 a notice offers `Command::AddTypeScript`.
- **Threads** (`lsp/connection.rs`): a starter that spawns the process and
  then reads its output, a writer, and a stderr drain. Nothing on the main
  thread waits on a server: a document's text is queued as a rope clone and
  serialised by the writer, and a queued `didChange` is replaced by a newer
  one. Server requests (`client/registerCapability`,
  `workspace/configuration`, refreshes) are answered on the reader thread;
  log and progress chatter is dropped there. Every request holds a busy
  token until its answer is applied, so `settle` waits for answers.
- **Documents** (`LanguageServer::sync`, after every command and every
  Apply): open first-class-language files that are fully read and not large
  (`Editor::is_large`) are opened on the server, sent whole on every change
  (with their own version counter: undo moves `Editor::version` back) and
  closed with their tab. Positions are in the encoding the server picked
  (Genea offers UTF-8 first; `lsp/text.rs` converts).
- **Diagnostics** are pulled (`textDocument/diagnostic`, one request in
  flight per file; push is turned off with `disablePushDiagnostics`, though
  pushed ones are taken too). A change re-pulls every open file (TypeScript
  diagnostics depend on other files), and so does
  `workspace/diagnostic/refresh`. They land in Problems as
  `ProblemSource::TypeScript`, per file; information and hints are left out.
  A file's diagnostics go when it closes, every file's when the server dies.
- **Watched files**: Genea advertises dynamic registration for
  `didChangeWatchedFiles`, so tsgo runs no watcher; the globs it registers
  (`lsp/watch.rs`, case-insensitive: tsgo lowercases paths) are matched in
  the background against the project watcher's batches, each path sent as
  created, changed or deleted (`FileChanges::created`).
- **Lifecycle** (`LanguageServer`): starting, ready, not responding (no
  answer to `initialize` within `START_TIMEOUT` of the open or a start; it
  keeps running), restarting (`RESTART_DELAY` after a crash), failed (more
  than `MAX_RESTARTS` within `RESTART_WINDOW`), off. All on the host clock.
  `StatusBar::language_servers` shows it; `Command::RestartLanguageServer`
  starts afresh from any state. A generation counter drops events and
  timers of an earlier process.
- **Adding a request** (completion, hover, …): a `Pending` variant, sent
  with `LanguageServer::request` while ready, answered in
  `LanguageServer::event`. Oxlint and Oxfmt are more `LanguageServer`s with
  their own `ServerSpec`.
- **Oxlint** (ticket #49, `lsp/oxc.rs`, `project/oxlint.rs`): runs when the
  root `package.json` lists both `oxlint` and `oxfmt` and
  `node_modules/oxlint/bin/oxlint` exists, as
  `<pinned node or bun> node_modules/oxlint/bin/oxlint --lsp` in the project
  environment, once that environment is ready (the launcher is a Node
  script). Type-aware linting is passed explicitly in
  `initializationOptions` (`[{ workspaceUri, options: { typeAware } }]`),
  true only when the root `.oxlintrc.json(c)` says
  `"options": { "typeAware": true }`. A change of runtime, Oxlint install or
  that flag starts it afresh. Its diagnostics are `ProblemSource::Oxlint`.
  Without Oxlint and Oxfmt a notice offers `Command::AddOxlintAndOxfmt`.
  Server generations are unique across servers, so `language_event` and
  `language_timer` route Oxlint's by generation; "Restart language server"
  restarts both.

Tests use the **fake LSP server** (`genea_testkit::FakeLsp`), installed on
the test host as `tsc` (and as `node` or `bun` for Oxlint in
`tests/oxlint.rs`): scripted per test to report markers, crash, stay
silent, delay or flood, and asked afterwards what reached it. As a binary
(`genea-fake-lsp`, script in `<binary>.json` beside it) it stands in for
tsgo in the harness's `typing-silent-lsp`. The slow lane
`tests/language_server_slow.rs` (`-- --ignored`) installs TypeScript 7 with
pnpm and checks real diagnostics.

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
  plus base-row rebasing for `f32` precision. The gutter is line numbers,
  then git markers, then fold markers at its right edge. A press on a fold
  marker is `ToggleFold`; elsewhere in those two columns, on a line with a
  git marker, it is `ShowHunk`. Git markers and the change's popover are
  placed by row, so lines hidden in a fold have none.
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
