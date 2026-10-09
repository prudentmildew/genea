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
| Projects | `open_project(root) -> ProjectId`, `close_project(id)`, `projects()` | Opening an already-open folder returns its id. |
| Commands in | `dispatch(id, Command)` | `Command` (`src/command.rs`) is plain data, one variant per user action. Commands apply synchronously on the main thread. |
| View state out | `project(id) -> Option<ProjectView>` | Plain snapshots (`src/view.rs`) of what the window shows: user-visible text, 1-based labels, display columns. |
| Change notification | `set_notifier(Fn() + Send + Sync)` | Called from any thread when background work has finished. The app then calls `pump()` on the main thread and re-reads view state. |
| Waiting | `pump() -> bool`, `settle()` | `pump` applies finished work without waiting. `settle` waits until nothing is pending (tests). |

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

## The host boundary

`genea_host::Host` provides `clock()`, `processes()`, `downloads()` and
`support_dir()` (Genea's application-support folder: real
`~/Library/Application Support/Genea`, a temp dir in tests; used directly
through the filesystem). `Downloads::fetch_with_length` also reports the
response's `Content-Length`, for progress.

- `RealHost`: the monotonic clock with one lazily started timer thread (so no
  idle wake-ups), `std::process`, and HTTP through `ureq` on the system TLS
  stack.
- `genea_testkit::TestHost`: `ManualClock::advance` fires due timers;
  `ScriptedProcesses::script("tsc", |spec, io| …)` plays a program on a
  thread with real pipes (unscripted programs fail with `NotFound`) and
  records every spawn; `ScriptedDownloads::serve/fail` answers URLs from a
  table (unknown URLs answer 404) and records every request.
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
  which coalesces wake-ups into one `pump` per event-loop turn.
- `src/window.rs`: `WindowController::sync`, the only place view state flows
  into Slint.
- `src/surface.rs`: the editor surface, a ring of line slots (line L in slot
  L % slots) with a per-slot diff, plus base-line rebasing for `f32`
  precision.
- `src/keys.rs`: the keymap (WebStorm macOS). `src/dialogs.rs`: native
  NSOpenPanels.

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
