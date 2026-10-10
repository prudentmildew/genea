//! The benchmark harness's control channel (ticket #23; `crates/genea-bench`).
//!
//! Open only while the journal is on (`GENEA_JOURNAL=1`, `src/journal.rs`).
//! The harness writes one command per line to Genea's stdin, and Genea
//! answers each with one JSON line on stdout, in order. A command that waits
//! (`wait-content`, `scroll`) answers when it is done. A reader thread
//! blocks on stdin, so the channel wakes the main thread only when a command
//! arrives; end of input quits Genea, so a harness that dies takes Genea
//! with it.
//!
//! The commands are primitives; the scenarios live in the harness:
//!
//! | Command | Answer |
//! |---|---|
//! | `info` | pid, display refresh rate and scale, window size, visibility |
//! | `wait-content` | once the first frame with file content is presented and a window is visible |
//! | `journal` | the whole journal (`journal::dump`) |
//! | `key CODE FLAGS` | posts a key down and up into Genea's own event queue |
//! | `place-caret LINE COLUMN` | after the caret moved (0-based, like a click) |
//! | `scroll PX MS` | scrolls PX points per presented frame for MS ms, bouncing at the ends |
//! | `resize WIDTH HEIGHT` | after asking for a window size in points |
//! | `caret-line` | the caret's line as the editor shows it, and the caret |
//! | `open PATH` | once a frame showing the file (relative to the project root) is presented |
//! | `editor` | the focused file: path, line count, whether it is still loading or large |
//! | `finder MODE` | after opening the finder (`files`, `recent`, `actions`, `everywhere`) or closing it (`close`) |
//! | `search TEXT` | after showing the Search view and searching for TEXT (literal, any case): `at`, when |
//! | `toggle-folder PATH` | after expanding or collapsing a folder in the Files view: `at`, when |
//! | `expect NAME CONDITION…` | at once; from now on, notes when a synced view first meets the condition |
//! | `check CONDITION…` | whether the first window's view meets the condition now: `holds` |
//! | `await NAME MS` | once a frame drawn after the view met NAME's condition is presented: `met`, when it was met; after MS ms, `met` and `"presented":false` if it was met but no frame came (nothing on screen changed), else an error |
//! | `processes` | the pids of Genea's child processes and theirs (language servers, shells) |
//! | `quit` | exits |
//!
//! Times are `mach_absolute_time` in ns since boot, the journal's clock. The
//! conditions `expect` takes (ticket #63), on the first window's view state:
//!
//! | Condition | Met when |
//! |---|---|
//! | `finder QUERY` | the finder shows the results for QUERY (the rest of the line, maybe empty) |
//! | `search-results QUERY` | the Search view shows a match for QUERY, or has finished without one |
//! | `search-done QUERY` | the search for QUERY has finished |
//! | `expanded PATH`, `collapsed PATH` | the Files view shows the folder PATH expanded, collapsed |
//! | `tree-has PATH`, `tree-lacks PATH` | the Files view shows PATH, doesn't |
//! | `editor-has PATH TEXT`, `editor-lacks PATH TEXT` | an editor showing PATH has (hasn't) TEXT in its visible lines |
//! | `problems PATH N` | Problems lists exactly N TypeScript problems in PATH |
//! | `server STATE` | a language server is `ready`, `off`, `failed`, `starting`, … |
//!
//! Paths are relative to the project root, without spaces.
//!
//! `key` takes a virtual key code and `CGEventFlags` (⇧ `1<<17`, ⌥ `1<<19`).
//! It builds a keyboard `CGEvent`, wraps it in an `NSEvent` and posts that to
//! the app's own queue (`postEvent:atStart:`). That needs no Accessibility
//! permission and takes the real path: NSApplication → NSWindow → winit's
//! view → NSTextInputClient → Slint's `TextInput` → the keymap. The
//! characters come from the current keyboard layout, so dead keys compose.
//! (An `NSEvent` made with `keyEventWithType:…` does not: AppKit starts the
//! composition, then drops it on the next key.)

use std::{
    cell::Cell,
    ffi::c_void,
    io::{BufRead, Write},
    rc::Rc,
    time::{Duration, Instant},
};

use genea_core::{
    Command, FileRowKind, FinderMode, LanguageServerState, LeftColumnView, ProblemSource, ProjectView, SearchQuery,
};
use objc2::{
    ClassType, MainThreadMarker,
    encode::{Encoding, RefEncode},
    msg_send,
    rc::Retained,
};
use objc2_app_kit::{NSApplication, NSApplicationOcclusionState, NSEvent};
use slint::ComponentHandle;

use crate::{app, journal, surface::LINE_HEIGHT};

/// Starts the reader thread. `main` calls it once the backend is up, while
/// the journal is on.
pub fn listen() {
    let spawned = std::thread::Builder::new().name("genea-remote".into()).spawn(|| {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if slint::invoke_from_event_loop(move || handle(line.trim())).is_err() {
                return;
            }
        }
        let _ = slint::invoke_from_event_loop(|| std::process::exit(0));
    });
    if let Err(error) = spawned {
        eprintln!("genea: journal: no control channel: {error}");
    }
}

fn reply(json: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{json}");
    let _ = out.flush();
}

fn error(message: &str) {
    reply(&format!(r#"{{"error":{}}}"#, quote(message)));
}

fn handle(line: &str) {
    let words: Vec<&str> = line.split_whitespace().collect();
    match words.as_slice() {
        ["info"] => info(),
        ["wait-content"] => journal::when_content_visible(|| reply(r#"{"content_visible":true}"#)),
        ["journal"] => reply(&journal::dump()),
        ["key", code, flags] => match (code.parse(), flags.parse()) {
            (Ok(code), Ok(flags)) => {
                post_key(code, flags);
                reply(r#"{"ok":true}"#);
            }
            _ => error("key: bad code or flags"),
        },
        ["place-caret", line, column] => match (line.parse(), column.parse()) {
            (Ok(line), Ok(column)) => app::with_app(move |app| {
                if let Some((controller, workbench)) = app.first_window() {
                    controller.dispatch(workbench, Command::PlaceCaret { line, column });
                }
                reply(r#"{"ok":true}"#);
            }),
            _ => error("place-caret: bad line or column"),
        },
        ["scroll", px, ms] => match (px.parse(), ms.parse()) {
            (Ok(px), Ok(ms)) => scroll(px, Duration::from_millis(ms)),
            _ => error("scroll: bad step or duration"),
        },
        ["resize", width, height] => match (width.parse(), height.parse()) {
            (Ok(width), Ok(height)) => app::with_app(move |app| {
                if let Some((controller, _)) = app.first_window() {
                    controller.window.window().set_size(slint::LogicalSize::new(width, height));
                }
                reply(r#"{"ok":true}"#);
            }),
            _ => error("resize: bad width or height"),
        },
        ["caret-line"] => app::with_app(|app| {
            let Some((controller, workbench)) = app.first_window() else { return error("no project window") };
            let Some(editor) = workbench.project(controller.project).and_then(|p| p.editor) else {
                return error("no editor");
            };
            let text = editor.lines.iter().find(|l| l.index == editor.caret.line).map_or("", |l| l.text.as_str());
            reply(&format!(
                r#"{{"line":{},"column":{},"text":{},"composing":{}}}"#,
                editor.caret.line,
                editor.caret.column,
                quote(text),
                editor.preedit.is_some()
            ));
        }),
        ["open", ..] => {
            let path = std::path::PathBuf::from(line["open".len()..].trim());
            journal::open_requested(path.clone(), || reply(r#"{"shown":true}"#));
            app::with_app(move |app| match app.first_window() {
                Some((controller, workbench)) => controller.dispatch(workbench, Command::OpenFile(path)),
                None => error("no project window"),
            });
        }
        ["editor"] => app::with_app(|app| {
            let Some((controller, workbench)) = app.first_window() else { return error("no project window") };
            let Some(view) = workbench.project(controller.project) else { return error("no project") };
            let Some(editor) = view.editor else { return error("no editor") };
            reply(&format!(
                r#"{{"path":{},"line_count":{},"loading":{},"large_file":{}}}"#,
                quote(&editor.path.to_string_lossy()),
                editor.line_count,
                editor.loading,
                view.status.large_file.is_some()
            ));
        }),
        ["finder", mode] => {
            let command = match *mode {
                "files" => Command::OpenFinder(FinderMode::Files),
                "recent" => Command::OpenFinder(FinderMode::RecentFiles),
                "actions" => Command::OpenFinder(FinderMode::Actions),
                "everywhere" => Command::OpenFinder(FinderMode::Everywhere),
                "close" => Command::CloseFinder,
                _ => return error("finder: unknown mode"),
            };
            app::with_app(move |app| match app.first_window() {
                Some((controller, workbench)) => {
                    controller.dispatch(workbench, command);
                    reply(r#"{"ok":true}"#);
                }
                None => error("no project window"),
            });
        }
        ["search", ..] => {
            let text = line["search".len()..].trim().to_string();
            app::with_app(move |app| match app.first_window() {
                Some((controller, workbench)) => {
                    controller.show_view(workbench, LeftColumnView::Search);
                    let at = journal::now_ns();
                    controller.dispatch(workbench, Command::Search(SearchQuery { text, ..SearchQuery::default() }));
                    reply(&format!(r#"{{"at":{at}}}"#));
                }
                None => error("no project window"),
            });
        }
        ["toggle-folder", path] => {
            let path = std::path::PathBuf::from(path);
            app::with_app(move |app| match app.first_window() {
                Some((controller, workbench)) => {
                    let at = journal::now_ns();
                    controller.dispatch(workbench, Command::ToggleFolder(path));
                    reply(&format!(r#"{{"at":{at}}}"#));
                }
                None => error("no project window"),
            });
        }
        ["expect", name, kind, ..] => {
            let rest = line.splitn(4, char::is_whitespace).nth(3).unwrap_or("").trim();
            match condition(kind, rest) {
                Ok(condition) => {
                    journal::expect(name.to_string(), condition);
                    reply(r#"{"ok":true}"#);
                }
                Err(message) => error(&format!("expect: {message}")),
            }
        }
        ["check", kind, ..] => {
            let rest = line.splitn(3, char::is_whitespace).nth(2).unwrap_or("").trim();
            match condition(kind, rest) {
                Ok(condition) => app::with_app(move |app| {
                    let Some((controller, workbench)) = app.first_window() else { return error("no project window") };
                    let holds = workbench.project(controller.project).is_some_and(|view| condition(&view));
                    reply(&format!(r#"{{"holds":{holds}}}"#));
                }),
                Err(message) => error(&format!("check: {message}")),
            }
        }
        ["await", name, ms] => match ms.parse() {
            Ok(ms) => wait_for(name.to_string(), Duration::from_millis(ms)),
            Err(_) => error("await: bad timeout"),
        },
        ["processes"] => {
            let pids: Vec<String> = child_pids(std::process::id()).iter().map(u32::to_string).collect();
            reply(&format!(r#"{{"pids":[{}]}}"#, pids.join(",")));
        }
        ["quit"] => {
            reply(r#"{"ok":true}"#);
            std::process::exit(0);
        }
        _ => error(&format!("unknown command: {line}")),
    }
}

/// A condition `expect` watches for (the table at the top): its kind and
/// the rest of the command line.
fn condition(kind: &str, rest: &str) -> Result<Box<dyn Fn(&ProjectView) -> bool>, String> {
    let rest = rest.to_string();
    let (first, tail) = match rest.split_once(char::is_whitespace) {
        Some((first, tail)) => (first.to_string(), tail.trim().to_string()),
        None => (rest.clone(), String::new()),
    };
    let path = std::path::PathBuf::from(&first);
    Ok(match kind {
        "finder" => Box::new(move |v| v.finder.as_ref().is_some_and(|f| f.query == rest && !f.matching)),
        "search-results" => {
            Box::new(move |v| v.search.query.text == rest && (v.search.match_count > 0 || !v.search.searching))
        }
        "search-done" => Box::new(move |v| v.search.query.text == rest && !v.search.searching),
        "expanded" | "collapsed" => {
            let expanded = kind == "expanded";
            Box::new(move |v| v.files.iter().any(|row| row.path == path && row.kind == FileRowKind::Folder { expanded }))
        }
        "tree-has" => Box::new(move |v| v.files.iter().any(|row| row.path == path)),
        "tree-lacks" => Box::new(move |v| !v.files.iter().any(|row| row.path == path)),
        "editor-has" | "editor-lacks" => {
            let has = kind == "editor-has";
            Box::new(move |v| {
                let mut editors = v.panes.iter().filter_map(|p| p.editor.as_ref()).filter(|e| e.path == path);
                editors.any(|e| e.lines.iter().any(|l| l.text.contains(tail.as_str())) == has)
            })
        }
        "problems" => {
            let count: usize = tail.parse().map_err(|_| "problems: bad count".to_string())?;
            Box::new(move |v| {
                v.problems.iter().filter(|p| p.source == ProblemSource::TypeScript && p.path == path).count() == count
            })
        }
        "server" => {
            let state = match first.as_str() {
                "starting" => LanguageServerState::Starting,
                "ready" => LanguageServerState::Ready,
                "not-responding" => LanguageServerState::NotResponding,
                "restarting" => LanguageServerState::Restarting,
                "failed" => LanguageServerState::Failed,
                "off" => LanguageServerState::Off,
                _ => return Err(format!("server: unknown state {first}")),
            };
            Box::new(move |v| v.status.language_servers.iter().any(|s| s.state == state))
        }
        _ => return Err(format!("unknown condition {kind}")),
    })
}

/// Answers `await`: when the condition watched as `name` was met, once a
/// frame drawn after that is presented; an error after `timeout`.
fn wait_for(name: String, timeout: Duration) {
    let answered = Rc::new(Cell::new(false));
    let done = answered.clone();
    let unknown = name.clone();
    journal::when_presented(&name, move |met| {
        if done.replace(true) {
            return;
        }
        match met {
            Some(met) => reply(&format!(r#"{{"met":{met}}}"#)),
            None => error(&format!("await: nothing expected as {unknown}")),
        }
    });
    slint::Timer::single_shot(timeout, move || {
        if !answered.replace(true) {
            journal::forget_presented(&name);
            // Met, but nothing on screen changed, so no frame came.
            match journal::met(&name) {
                Some(met) => reply(&format!(r#"{{"met":{met},"presented":false}}"#)),
                None => error(&format!("await: {name} not met within {} ms", timeout.as_millis())),
            }
        }
    });
}

/// The pids of `pid`'s children, and of theirs.
fn child_pids(pid: u32) -> Vec<u32> {
    let mut buffer = vec![0i32; 256];
    // SAFETY: proc_listchildpids writes at most the buffer's size in bytes.
    let n = unsafe {
        libc::proc_listchildpids(pid as i32, buffer.as_mut_ptr().cast(), (buffer.len() * size_of::<i32>()) as i32)
    };
    let mut found = Vec::new();
    for &child in buffer.iter().take(n.max(0) as usize).filter(|&&c| c > 0) {
        found.push(child as u32);
        found.extend(child_pids(child as u32));
    }
    found
}

fn info() {
    let Some(mtm) = MainThreadMarker::new() else { return error("not on the main thread") };
    let ns_app = NSApplication::sharedApplication(mtm);
    let visible = ns_app.occlusionState().contains(NSApplicationOcclusionState::Visible);
    let screen = ns_app.keyWindow().and_then(|w| w.screen());
    let display_hz = screen.as_ref().map_or(0, |s| s.maximumFramesPerSecond());
    let display_scale = screen.as_ref().map_or(0.0, |s| s.backingScaleFactor());
    app::with_app(move |app| {
        let (width, height, scale) = match app.first_window() {
            Some((controller, _)) => {
                let window = controller.window.window();
                let scale = window.scale_factor();
                let size = window.size().to_logical(scale);
                (size.width, size.height, scale)
            }
            None => (0.0, 0.0, 0.0),
        };
        reply(&format!(
            r#"{{"pid":{},"display_hz":{display_hz},"display_scale":{display_scale},"window_scale":{scale},"window_width":{width},"window_height":{height},"visible":{visible}}}"#,
            std::process::id()
        ));
    });
}

/// Scrolls the first window's left pane by `px` per presented frame, through its
/// `scrolled` callback (the trackpad's path), until `duration` is up. Each
/// step runs right after the previous frame, so every display-link tick has
/// something new to draw. At either end it turns around.
fn scroll(px: f32, duration: Duration) {
    let Some(window) = first_window_handle() else { return error("no project window") };
    let until = Instant::now() + duration;
    let direction = Rc::new(Cell::new(1.0f32));
    let steps = Rc::new(Cell::new(0u32));
    let answered = Rc::new(Cell::new(false));
    let step = {
        let (direction, steps) = (direction.clone(), steps.clone());
        move || {
            let Some(window) = window.upgrade() else { return };
            let before = scroll_top().unwrap_or_default();
            window.invoke_scrolled(0, -px * direction.get());
            // At an end the core clamps, and a step that moves less than a
            // pixel draws no frame, which would end the chain: turn around.
            let moved = (scroll_top().unwrap_or_default() - before).abs() * f64::from(LINE_HEIGHT);
            if moved < 1.0 {
                direction.set(-direction.get());
                window.invoke_scrolled(0, -px * direction.get());
            }
            steps.set(steps.get() + 1);
        }
    };
    let finish = {
        let (steps, answered) = (steps.clone(), answered.clone());
        move || {
            if !answered.replace(true) {
                reply(&format!(r#"{{"steps":{}}}"#, steps.get()));
            }
        }
    };
    let step = Rc::new(step);
    (*step)();
    // If frames stop coming (nothing left to scroll), answer anyway.
    let fallback = finish.clone();
    slint::Timer::single_shot(duration + Duration::from_millis(500), fallback);
    journal::after_every_frame(move || {
        if Instant::now() >= until {
            // Answer from the event loop, not from inside the renderer.
            slint::Timer::single_shot(Duration::ZERO, finish.clone());
            return false;
        }
        let next = step.clone();
        slint::Timer::single_shot(Duration::ZERO, move || (*next)());
        true
    });
}

fn first_window_handle() -> Option<slint::Weak<crate::ProjectWindow>> {
    let handle = Rc::new(Cell::new(None));
    let out = handle.clone();
    // Not deferred: the app is never borrowed while a command is handled.
    app::with_app(move |app| out.set(app.first_window().map(|(c, _)| c.window.as_weak())));
    handle.take()
}

fn scroll_top() -> Option<f64> {
    let top = Rc::new(Cell::new(None));
    let out = top.clone();
    app::with_app(move |app| {
        out.set(
            app.first_window()
                .and_then(|(c, w)| w.project(c.project))
                .and_then(|p| p.panes.into_iter().next()?.editor)
                .map(|e| e.scroll_top),
        )
    });
    top.get()
}

fn post_key(code: u16, flags: u64) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let ns_app = NSApplication::sharedApplication(mtm);
    for down in [true, false] {
        // SAFETY: CoreGraphics calls with valid arguments; each object we
        // create is released once `eventWithCGEvent:` has copied it.
        unsafe {
            let source = CGEventSourceCreate(HID_SYSTEM_STATE);
            let cg_event = CGEventCreateKeyboardEvent(source, code, down);
            if cg_event.is_null() {
                CFRelease(source);
                continue;
            }
            CGEventSetFlags(cg_event, flags);
            let event: Option<Retained<NSEvent>> =
                msg_send![NSEvent::class(), eventWithCGEvent: cg_event.cast::<CGEvent>()];
            CFRelease(cg_event);
            CFRelease(source);
            if let Some(event) = event {
                ns_app.postEvent_atStart(&event, false);
            }
        }
    }
}

/// `CGEventRef`'s pointee, so `eventWithCGEvent:` gets the argument type
/// its method signature names (objc2 checks it in debug builds).
#[repr(C)]
struct CGEvent {
    _opaque: [u8; 0],
}

// SAFETY: a `CGEventRef` is a pointer to the opaque `struct __CGEvent`.
unsafe impl RefEncode for CGEvent {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("__CGEvent", &[]));
}

/// `kCGEventSourceStateHIDSystemState`.
const HID_SYSTEM_STATE: i32 = 1;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceCreate(state: i32) -> *mut c_void;
    fn CGEventCreateKeyboardEvent(source: *mut c_void, code: u16, down: bool) -> *mut c_void;
    fn CGEventSetFlags(event: *mut c_void, flags: u64);
    fn CFRelease(object: *const c_void);
}

/// A JSON string literal.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
