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
//! | `caret-line` | the caret's line as the editor shows it, and the caret |
//! | `quit` | exits |
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

use genea_core::Command;
use objc2::{ClassType, MainThreadMarker, msg_send, rc::Retained};
use objc2_app_kit::{NSApplication, NSApplicationOcclusionState, NSEvent};
use slint::ComponentHandle;

use crate::{app, journal};

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
        ["quit"] => {
            reply(r#"{"ok":true}"#);
            std::process::exit(0);
        }
        _ => error(&format!("unknown command: {line}")),
    }
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
    let step = {
        let (direction, steps) = (direction.clone(), steps.clone());
        move || {
            let Some(window) = window.upgrade() else { return };
            let before = scroll_top();
            window.invoke_scrolled(0, -px * direction.get());
            if scroll_top() == before {
                direction.set(-direction.get());
                window.invoke_scrolled(0, -px * direction.get());
            }
            steps.set(steps.get() + 1);
        }
    };
    let step = Rc::new(step);
    (*step)();
    journal::after_every_frame(move || {
        if Instant::now() >= until {
            let steps = steps.get();
            // Answer from the event loop, not from inside the renderer.
            slint::Timer::single_shot(Duration::ZERO, move || reply(&format!(r#"{{"steps":{steps}}}"#)));
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
            let event: Option<Retained<NSEvent>> = msg_send![NSEvent::class(), eventWithCGEvent: cg_event];
            CFRelease(cg_event);
            CFRelease(source);
            if let Some(event) = event {
                ns_app.postEvent_atStart(&event, false);
            }
        }
    }
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
