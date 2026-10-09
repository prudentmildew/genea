//! PROTOTYPE. Slint has no equivalent of GPUI's foreground journal, so the
//! spike keeps its own on the main thread:
//!
//! - a CFRunLoop observer records every run-loop activity, which splits the
//!   main thread into busy spans (woke up -> about to sleep) and segments
//!   (between any two activities);
//! - the winit event filter records when each key press arrives;
//! - Slint's rendering notifier records BeforeRendering / AfterRendering.
//!
//! A frame runs from the start of the run-loop segment its BeforeRendering
//! falls in (the display-link tick, before Slint waits for a drawable) to the
//! end of the segment its AfterRendering falls in (after Skia's flush and
//! wgpu's present have returned).

use std::{
    cell::RefCell,
    ffi::c_void,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

const BEFORE_TIMERS: usize = 1 << 1;
const BEFORE_SOURCES: usize = 1 << 2;
const BEFORE_WAITING: usize = 1 << 5;
const AFTER_WAITING: usize = 1 << 6;

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
    fn CFRunLoopAddObserver(rl: *mut c_void, observer: *mut c_void, mode: *const c_void);
    static kCFRunLoopCommonModes: *const c_void;
}

#[derive(Default)]
struct Log {
    /// (when, CFRunLoopActivity)
    activities: Vec<(Instant, usize)>,
    keys: Vec<Instant>,
    before: Vec<Instant>,
    after: Vec<Instant>,
}

thread_local! {
    static LOG: RefCell<Option<Log>> = const { RefCell::new(None) };
    static BASE: RefCell<Option<Instant>> = const { RefCell::new(None) };
}

extern "C" fn observe(_: *mut c_void, activity: usize, _: *mut c_void) {
    let now = Instant::now();
    LOG.with_borrow_mut(|log| {
        if let Some(log) = log {
            log.activities.push((now, activity));
        }
    });
}

/// Starts recording. Not for the idle bench or interactive use.
pub fn start() {
    BASE.set(Some(Instant::now()));
    LOG.set(Some(Log::default()));
    unsafe {
        // One observer runs first in each activity, one last, so the busy
        // spans include every other observer's work (winit's included).
        for (activities, order) in [
            (AFTER_WAITING | BEFORE_TIMERS | BEFORE_SOURCES, isize::MIN),
            (BEFORE_WAITING, isize::MAX),
        ] {
            let observer =
                CFRunLoopObserverCreate(std::ptr::null(), activities, 1, order, observe, std::ptr::null_mut());
            CFRunLoopAddObserver(CFRunLoopGetMain(), observer, kCFRunLoopCommonModes);
        }
    }
}

pub fn recording() -> bool {
    LOG.with_borrow(|log| log.is_some())
}

/// When recording began (just after `main`).
pub fn base() -> Instant {
    BASE.with_borrow(|b| b.unwrap())
}

fn mark(f: impl FnOnce(&mut Log)) {
    LOG.with_borrow_mut(|log| {
        if let Some(log) = log {
            f(log)
        }
    });
}

pub fn mark_key() {
    let now = Instant::now();
    mark(|l| l.keys.push(now));
}

pub fn mark_before_rendering() {
    let now = Instant::now();
    mark(|l| l.before.push(now));
}

pub fn mark_after_rendering() {
    let now = Instant::now();
    mark(|l| l.after.push(now));
}

#[derive(Clone, Copy, Debug)]
pub struct Frame {
    /// Start of the run-loop segment BeforeRendering fell in.
    pub start: Instant,
    pub before: Instant,
    pub after: Instant,
    /// End of the run-loop segment AfterRendering fell in (present returned).
    pub end: Instant,
}

impl Frame {
    pub fn draw(&self) -> Duration {
        self.after - self.before
    }
}

pub struct Journal {
    activities: Vec<(Instant, usize)>,
    pub keys: Vec<Instant>,
    pub frames: Vec<Frame>,
}

pub fn journal() -> Journal {
    LOG.with_borrow(|log| {
        let log = log.as_ref().unwrap();
        let times: Vec<Instant> = log.activities.iter().map(|a| a.0).collect();
        let frames = log
            .after
            .iter()
            .filter_map(|&after| {
                let i = log.before.partition_point(|b| *b <= after);
                let before = *log.before.get(i.checked_sub(1)?)?;
                let i = times.partition_point(|t| *t <= before);
                let start = if i == 0 { base() } else { times[i - 1] };
                // A frame still in flight has no end yet.
                let end = *times.get(times.partition_point(|t| *t < after))?;
                Some(Frame { start, before, after, end })
            })
            .collect();
        Journal {
            activities: log.activities.clone(),
            keys: log.keys.clone(),
            frames,
        }
    })
}

impl Journal {
    pub fn first_frame(&self) -> Option<Frame> {
        self.frames.first().copied()
    }

    /// Main-thread busy spans (woke up -> about to sleep). The first span
    /// starts when recording began.
    fn busy(&self) -> Vec<(Instant, Instant)> {
        let mut spans = Vec::new();
        let mut woke = Some(base());
        for &(t, activity) in &self.activities {
            if activity == AFTER_WAITING {
                woke = Some(t);
            } else if activity == BEFORE_WAITING
                && let Some(w) = woke.take()
            {
                spans.push((w, t));
            }
        }
        spans
    }

    /// Longest time the main thread went without returning to the run loop
    /// to wait, in `window`. Stricter than GPUI's journal (which timed single
    /// events): one busy span can hold several events.
    pub fn longest_stall(&self, window: (Instant, Instant)) -> Value {
        let mut spans: Vec<Duration> = self
            .busy()
            .into_iter()
            .filter(|(s, e)| *e >= window.0 && *s <= window.1)
            .map(|(s, e)| e - s)
            .collect();
        spans.sort_by_key(|d| std::cmp::Reverse(*d));
        json!({
            "max_ms": spans.first().map(|d| ms(*d)).unwrap_or(0.0),
            "over_16ms": spans.iter().filter(|d| **d > Duration::from_millis(16)).count(),
            "top_ms": spans.iter().take(5).map(|d| ms(*d)).collect::<Vec<_>>(),
        })
    }

    /// Cadence and cost of frames in `window`.
    pub fn frame_stats(&self, window: (Instant, Instant)) -> Value {
        let frames: Vec<_> = self
            .frames
            .iter()
            .filter(|f| f.end >= window.0 && f.end <= window.1)
            .copied()
            .collect();
        let mut intervals: Vec<Duration> = frames.windows(2).map(|w| w[1].end - w[0].end).collect();
        let mut work: Vec<Duration> = frames.iter().map(|f| f.end - f.start).collect();
        let mut draw: Vec<Duration> = frames.iter().map(Frame::draw).collect();
        let mut present: Vec<Duration> = frames.iter().map(|f| f.end - f.after).collect();
        let mut acquire: Vec<Duration> = frames.iter().map(|f| f.before - f.start).collect();
        let span = window.1 - window.0;
        let display_fps = crate::synth::display_fps();
        let period = Duration::from_secs_f64(1.0 / display_fps);
        let budget = Duration::from_secs_f64(1.0 / 120.0);
        let late = intervals.iter().filter(|d| **d > period.mul_f64(1.5)).count();
        let over_budget = work.iter().filter(|d| **d > budget).count();
        json!({
            "display_fps": display_fps,
            "presented": frames.len(),
            "fps": frames.len() as f64 / span.as_secs_f64(),
            "late_intervals": late,
            "late_pct": 100.0 * late as f64 / intervals.len().max(1) as f64,
            "frames_over_8_3ms_work": over_budget,
            "interval_ms": stats(&mut intervals),
            "frame_work_ms": stats(&mut work),
            "tick_to_before_rendering_ms": stats(&mut acquire),
            "draw_ms": stats(&mut draw),
            "flush_present_ms": stats(&mut present),
        })
    }
}

pub fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 1e5).round() / 100.0
}

pub fn stats(samples: &mut [Duration]) -> Value {
    if samples.is_empty() {
        return Value::Null;
    }
    samples.sort();
    let pick = |q: f64| ms(samples[((samples.len() - 1) as f64 * q).round() as usize]);
    json!({
        "n": samples.len(),
        "p50": pick(0.5),
        "p95": pick(0.95),
        "p99": pick(0.99),
        "max": pick(1.0),
    })
}
