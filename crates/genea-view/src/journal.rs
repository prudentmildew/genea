//! The instrumentation journal the benchmark harness reads (spec #19,
//! ticket #23; `crates/genea-bench`).
//!
//! It is off unless `GENEA_JOURNAL=1` is set when Genea starts. Off, it
//! installs nothing: no run-loop observer, no rendering notifier, no winit
//! event filter, no control channel. The only trace left in a normal run is
//! one relaxed atomic load per sync ([`mark_synced`]). On, it records, on
//! the main thread:
//!
//! - every run-loop activity, from two CFRunLoop observers (one ordered
//!   first, one last), so the harness can find main-thread stalls and idle
//!   wake-ups;
//! - Slint's BeforeRendering and AfterRendering, for frames;
//! - key presses and IME commits as winit delivers them (keystroke receipt);
//! - each sync of view state into Slint, and the first one with file content;
//! - when AppKit first reports a window visible.
//!
//! Timestamps are `mach_absolute_time` in nanoseconds since boot, a clock the
//! harness shares, and the harness does all the analysis. The journal is
//! never written on a timer (that would be an idle wake-up of its own): the
//! harness asks for it over the control channel (`src/remote.rs`), which
//! `main` opens while the journal is on.
//!
//! This is the one module that uses Slint's `unstable-*` APIs
//! (`unstable-winit-030` for the event filter, `unstable-wgpu-30` for the
//! rendering notifier on Metal), so a Slint pin bump touches only this file.

use std::{
    cell::RefCell,
    ffi::c_void,
    fmt::Write as _,
    sync::atomic::{AtomicBool, Ordering},
};

use slint::{
    RenderingState,
    winit_030::{
        EventResult, WinitWindowAccessor,
        winit::event::{Ime, WindowEvent},
    },
};

const BEFORE_TIMERS: usize = 1 << 1;
const BEFORE_SOURCES: usize = 1 << 2;
const BEFORE_WAITING: usize = 1 << 5;
const AFTER_WAITING: usize = 1 << 6;

static ON: AtomicBool = AtomicBool::new(false);

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRunLoopGetMain() -> *mut c_void;
    fn CFRunLoopObserverCreate(
        allocator: *const c_void,
        activities: usize,
        repeats: u8,
        order: isize,
        callout: extern "C" fn(*mut c_void, usize, *mut c_void),
        context: *mut c_void,
    ) -> *mut c_void;
    fn CFRunLoopAddObserver(run_loop: *mut c_void, observer: *mut c_void, mode: *const c_void);
    static kCFRunLoopCommonModes: *const c_void;
}

/// Raw records, in `mach_absolute_time` ticks.
#[derive(Default)]
struct Log {
    started: u64,
    activities: Vec<(u64, usize)>,
    keys: Vec<u64>,
    before: Vec<u64>,
    after: Vec<u64>,
    synced: Vec<u64>,
    content: Option<u64>,
    visible: Option<u64>,
    /// Called once the first frame with content is presented and a window
    /// is visible (the harness's `wait-content`).
    on_content_visible: Option<Box<dyn FnOnce()>>,
    /// Called after every frame until it returns false (the harness's
    /// scrolling).
    after_frame: Option<Box<dyn FnMut() -> bool>>,
}

thread_local! {
    static LOG: RefCell<Log> = RefCell::default();
}

/// Whether the journal is recording.
pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Switches the journal on if `GENEA_JOURNAL=1`. Call first thing in `main`,
/// on the main thread, before the event loop runs.
pub fn start() {
    if std::env::var_os("GENEA_JOURNAL").is_none_or(|v| v != "1") {
        return;
    }
    ON.store(true, Ordering::Relaxed);
    LOG.with_borrow_mut(|log| log.started = now());
    // One observer runs first in each activity and one last, so each busy
    // span includes every other observer's work, winit's included.
    for (activities, order) in
        [(AFTER_WAITING | BEFORE_TIMERS | BEFORE_SOURCES, isize::MIN), (BEFORE_WAITING, isize::MAX)]
    {
        // SAFETY: plain CoreFoundation calls on the main thread; the
        // observer is never released, as it lives as long as the process.
        unsafe {
            let observer = CFRunLoopObserverCreate(std::ptr::null(), activities, 1, order, observe, std::ptr::null_mut());
            CFRunLoopAddObserver(CFRunLoopGetMain(), observer, kCFRunLoopCommonModes);
        }
    }
}

/// Hooks a window up to the journal: frames, keys and visibility. Does
/// nothing while the journal is off.
pub fn attach(window: &slint::Window) {
    if !on() {
        return;
    }
    let notifier = window.set_rendering_notifier(|state, _| match state {
        RenderingState::BeforeRendering => record(|log, t| log.before.push(t)),
        RenderingState::AfterRendering => {
            record(|log, t| log.after.push(t));
            if let Some(mut after_frame) = LOG.with_borrow_mut(|log| log.after_frame.take()) {
                let keep = after_frame();
                LOG.with_borrow_mut(|log| {
                    if keep && log.after_frame.is_none() {
                        log.after_frame = Some(after_frame);
                    }
                });
            }
        }
        _ => {}
    });
    if let Err(error) = notifier {
        eprintln!("genea: journal: no rendering notifier: {error:?}");
    }
    // A debugging aid: `GENEA_JOURNAL_TRACE=1` prints winit's window events.
    let trace = std::env::var_os("GENEA_JOURNAL_TRACE").is_some_and(|v| v == "1");
    window.on_winit_window_event(move |_, event| {
        if trace && !matches!(event, WindowEvent::RedrawRequested | WindowEvent::CursorMoved { .. }) {
            eprintln!("genea: winit: {event:?}");
        }
        match event {
            WindowEvent::KeyboardInput { event, .. } if event.state.is_pressed() => record(|log, t| log.keys.push(t)),
            WindowEvent::Ime(Ime::Commit(_)) => record(|log, t| log.keys.push(t)),
            WindowEvent::Occluded(false) => record(|log, t| {
                log.visible.get_or_insert(t);
            }),
            _ => {}
        }
        EventResult::Propagate
    });
}

/// View state was just pushed into Slint; `content` says whether the
/// editor showed file content.
pub fn mark_synced(content: bool) {
    if !on() {
        return;
    }
    record(|log, t| {
        log.synced.push(t);
        if content {
            log.content.get_or_insert(t);
        }
    });
}

/// Calls `f` once the first frame with file content has been presented and
/// a window is visible (right away if that has happened).
pub fn when_content_visible(f: impl FnOnce() + 'static) {
    LOG.with_borrow_mut(|log| log.on_content_visible = Some(Box::new(f)));
    check_content_visible();
}

/// Calls `f` after every presented frame until it returns false.
pub fn after_every_frame(f: impl FnMut() -> bool + 'static) {
    LOG.with_borrow_mut(|log| log.after_frame = Some(Box::new(f)));
}

/// The whole journal as one JSON object, in nanoseconds since boot. The
/// harness parses it (`genea_bench::journal::Journal::from_json`).
pub fn dump() -> String {
    LOG.with_borrow(|log| {
        let list = |items: &[u64]| {
            let mut s = String::from("[");
            for (i, t) in items.iter().enumerate() {
                let _ = write!(s, "{}{}", if i > 0 { "," } else { "" }, ns(*t));
            }
            s.push(']');
            s
        };
        let option = |t: Option<u64>| t.map_or("null".to_string(), |t| ns(t).to_string());
        let mut activities = String::from("[");
        for (i, (t, a)) in log.activities.iter().enumerate() {
            let _ = write!(activities, "{}[{},{}]", if i > 0 { "," } else { "" }, ns(*t), a);
        }
        activities.push(']');
        format!(
            r#"{{"process_start":{},"started":{},"activities":{},"keys":{},"before":{},"after":{},"synced":{},"content":{},"visible":{}}}"#,
            ns(process_start()),
            ns(log.started),
            activities,
            list(&log.keys),
            list(&log.before),
            list(&log.after),
            list(&log.synced),
            option(log.content),
            option(log.visible),
        )
    })
}

extern "C" fn observe(_: *mut c_void, activity: usize, _: *mut c_void) {
    let t = now();
    LOG.with_borrow_mut(|log| log.activities.push((t, activity)));
    if activity == BEFORE_WAITING {
        check_content_visible();
    }
}

fn record(f: impl FnOnce(&mut Log, u64)) {
    let t = now();
    LOG.with_borrow_mut(|log| f(log, t));
}

/// Runs the content-visible callback once a frame drawn after the content
/// sync has presented (an activity followed its AfterRendering) and a
/// window is visible.
fn check_content_visible() {
    let ready = LOG.with_borrow_mut(|log| {
        if log.on_content_visible.is_none() || log.visible.is_none() {
            return None;
        }
        let content = log.content?;
        let before = *log.before.iter().find(|&&b| b >= content)?;
        let after = *log.after.iter().find(|&&a| a >= before)?;
        let presented = log.activities.last().is_some_and(|&(t, _)| t >= after);
        if presented { log.on_content_visible.take() } else { None }
    });
    if let Some(f) = ready {
        f();
    }
}

#[repr(C)]
struct Timebase {
    numer: u32,
    denom: u32,
}

unsafe extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut Timebase) -> i32;
}

fn now() -> u64 {
    // SAFETY: no preconditions.
    unsafe { mach_absolute_time() }
}

/// Mach ticks to nanoseconds.
fn ns(ticks: u64) -> u64 {
    let mut timebase = Timebase { numer: 0, denom: 0 };
    // SAFETY: writes into the struct we pass.
    unsafe { mach_timebase_info(&mut timebase) };
    (ticks as u128 * timebase.numer as u128 / timebase.denom.max(1) as u128) as u64
}

/// When the kernel started this process, in mach ticks.
fn process_start() -> u64 {
    // SAFETY: proc_pid_rusage fills the struct we pass for our own pid.
    unsafe {
        let mut info: libc::rusage_info_v4 = std::mem::zeroed();
        let rc = libc::proc_pid_rusage(
            libc::getpid(),
            libc::RUSAGE_INFO_V4,
            &mut info as *mut libc::rusage_info_v4 as *mut libc::rusage_info_t,
        );
        if rc == 0 { info.ri_proc_start_abstime } else { 0 }
    }
}
