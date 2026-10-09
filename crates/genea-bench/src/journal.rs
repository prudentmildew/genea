//! Genea's instrumentation journal, as the harness reads it.
//!
//! Genea keeps the journal only when `GENEA_JOURNAL=1` is set
//! (`crates/genea-view/src/journal.rs`) and dumps it on request. It holds raw
//! timestamps, all in nanoseconds since boot (`mach_absolute_time`), which
//! the harness shares with Genea, so the harness can bracket a scenario with
//! its own clock readings ([`crate::sys::now_ns`]). Everything derived from
//! them (frames, stalls, keystroke latencies, cadence) is computed here:
//!
//! - **Run-loop activities**: two CFRunLoop observers on the main thread, one
//!   ordered first (woke up, before timers, before sources) and one last
//!   (about to wait). A *busy span* runs from waking to waiting; the longest
//!   is the longest main-thread stall.
//! - **Frames**: Slint's BeforeRendering / AfterRendering. A frame runs from
//!   the start of the run-loop segment its BeforeRendering falls in (the
//!   display-link tick) to the end of the segment its AfterRendering falls
//!   in (when the present has returned).
//! - **Keys**: key presses and IME commits as winit delivers them.
//! - **Synced**: when Genea finished pushing view state into Slint.
//! - **Events**: every winit window event but redraws (pointer, focus,
//!   occlusion, …), to tell an idle window disturbed from outside.
//! - **Milestones**: the first sync with file content in it, and AppKit
//!   first reporting the window visible.

use serde_json::Value;

/// CFRunLoopActivity values, as the journal records them.
pub const AFTER_WAITING: u32 = 1 << 6;
pub const BEFORE_TIMERS: u32 = 1 << 1;
pub const BEFORE_SOURCES: u32 = 1 << 2;
pub const BEFORE_WAITING: u32 = 1 << 5;

/// A main-thread stall longer than this misses a Genea budget.
pub const STALL_LIMIT_MS: f64 = 16.0;

/// A span of time, in ns since boot, both ends included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub from: u64,
    pub to: u64,
}

impl Window {
    fn contains(&self, t: u64) -> bool {
        self.from <= t && t <= self.to
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Journal {
    /// When the kernel started Genea's process.
    pub process_start: u64,
    /// When the journal was switched on (early in `main`).
    pub started: u64,
    /// (when, CFRunLoopActivity), in order.
    pub activities: Vec<(u64, u32)>,
    pub keys: Vec<u64>,
    pub before: Vec<u64>,
    pub after: Vec<u64>,
    pub synced: Vec<u64>,
    /// Every winit window event but redraws: input, focus, occlusion,
    /// resizes. While idle, one means something outside disturbed Genea.
    pub events: Vec<u64>,
    /// The first sync that put file content on the surface.
    pub content: Option<u64>,
    /// AppKit first reported a window visible.
    pub visible: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub start: u64,
    pub before: u64,
    pub after: u64,
    pub end: u64,
}

impl Frame {
    /// From the display-link tick to the present returning.
    pub fn work_ms(&self) -> f64 {
        ms(self.end - self.start)
    }
}

/// The main thread's longest busy spans in a window.
#[derive(Clone, Debug, PartialEq)]
pub struct Stalls {
    pub max_ms: f64,
    pub over_16ms: usize,
    /// The five longest, longest first.
    pub top_ms: Vec<f64>,
    /// Where the longest was (start, end), to find what caused it.
    pub longest: Option<(u64, u64)>,
}

impl Stalls {
    /// For the results; the longest stall's start in ms after `origin`.
    pub fn to_json(&self, origin: u64) -> Value {
        serde_json::json!({
            "max_ms": crate::stats::round(self.max_ms),
            "over_16ms": self.over_16ms,
            "top_ms": self.top_ms.iter().map(|&d| crate::stats::round(d)).collect::<Vec<_>>(),
            "longest_at_ms": self.longest.map(|(s, _)| crate::stats::round(ms(s.saturating_sub(origin)))),
        })
    }
}

/// One keystroke and the frame that showed it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Keystroke {
    pub key: u64,
    pub synced: u64,
    pub frame: Frame,
}

impl Keystroke {
    /// Genea's own work: from the key to the view state in Slint, plus the
    /// frame that shows it. The wait for the next display-link tick in
    /// between is left out: it depends on the display, not on Genea.
    pub fn genea_work_ms(&self) -> f64 {
        let frame_start = self.frame.start.max(self.synced);
        ms(self.synced - self.key) + ms(self.frame.end - frame_start)
    }

    /// From the key to the present of the frame showing it.
    pub fn input_to_present_ms(&self) -> f64 {
        ms(self.frame.end - self.key)
    }
}

/// How regularly frames were presented.
#[derive(Clone, Debug, PartialEq)]
pub struct Cadence {
    pub presented: usize,
    pub span_ms: f64,
    pub display_hz: f64,
    /// Between consecutive presents.
    pub intervals_ms: Vec<f64>,
    /// Intervals longer than 1.5 display periods: at least one frame dropped.
    pub late: usize,
    pub work_ms: Vec<f64>,
}

impl Cadence {
    pub fn dropped_pct(&self) -> f64 {
        100.0 * self.late as f64 / self.intervals_ms.len().max(1) as f64
    }

    pub fn fps(&self) -> f64 {
        if self.span_ms <= 0.0 { 0.0 } else { self.presented as f64 * 1000.0 / self.span_ms }
    }
}

pub fn ms(ns: u64) -> f64 {
    ns as f64 / 1_000_000.0
}

impl Journal {
    pub fn from_json(dump: &Value) -> Result<Journal, String> {
        let times = |key: &str| -> Result<Vec<u64>, String> {
            match dump.get(key) {
                None | Some(Value::Null) => Ok(Vec::new()),
                Some(Value::Array(items)) => {
                    items.iter().map(|v| v.as_u64().ok_or_else(|| format!("journal: bad time in {key}"))).collect()
                }
                Some(_) => Err(format!("journal: {key} is not a list")),
            }
        };
        let time = |key: &str| dump.get(key).and_then(Value::as_u64);
        let activities = match dump.get("activities") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .map(|v| match v.as_array().map(Vec::as_slice) {
                    Some([t, a]) => Some((t.as_u64()?, u32::try_from(a.as_u64()?).ok()?)),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
                .ok_or("journal: bad activity")?,
            Some(_) => return Err("journal: activities is not a list".into()),
        };
        Ok(Journal {
            process_start: time("process_start").unwrap_or(0),
            started: time("started").unwrap_or(0),
            activities,
            keys: times("keys")?,
            before: times("before")?,
            after: times("after")?,
            synced: times("synced")?,
            events: times("events")?,
            content: time("content"),
            visible: time("visible"),
        })
    }

    /// Every completed frame, in order.
    pub fn frames(&self) -> Vec<Frame> {
        let times: Vec<u64> = self.activities.iter().map(|a| a.0).collect();
        self.after
            .iter()
            .filter_map(|&after| {
                let i = self.before.partition_point(|&b| b <= after);
                let before = *self.before.get(i.checked_sub(1)?)?;
                let i = times.partition_point(|&t| t <= before);
                let start = if i == 0 { self.started } else { times[i - 1] };
                let end = *times.get(times.partition_point(|&t| t < after))?;
                Some(Frame { start, before, after, end })
            })
            .collect()
    }

    /// Main-thread busy spans, from waking to waiting. The first one starts
    /// when the journal started.
    pub fn busy_spans(&self) -> Vec<(u64, u64)> {
        let mut spans = Vec::new();
        let mut woke = Some(self.started);
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

    /// The longest busy spans that overlap `window`. Stricter than timing
    /// single events: one span can hold several.
    pub fn stalls(&self, window: Window) -> Stalls {
        let mut spans: Vec<(u64, u64)> =
            self.busy_spans().into_iter().filter(|&(s, e)| e >= window.from && s <= window.to).collect();
        spans.sort_by_key(|&(s, e)| std::cmp::Reverse(e - s));
        let durations: Vec<f64> = spans.iter().map(|&(s, e)| ms(e - s)).collect();
        Stalls {
            max_ms: durations.first().copied().unwrap_or(0.0),
            over_16ms: durations.iter().filter(|&&d| d > STALL_LIMIT_MS).count(),
            top_ms: durations.into_iter().take(5).collect(),
            longest: spans.first().copied(),
        }
    }

    /// Each keystroke in `window` with the first sync after it and the first
    /// frame drawn after that sync. Keystrokes shown by the same frame count
    /// once, from the earliest (a dead-key commit marks the key press and
    /// the IME commit). Keystrokes not shown yet are left out.
    pub fn keystrokes(&self, window: Window) -> Vec<Keystroke> {
        let frames = self.frames();
        let mut shown: Vec<Keystroke> = Vec::new();
        for &key in self.keys.iter().filter(|&&k| window.contains(k)) {
            let Some(&synced) = self.synced.iter().find(|&&s| s >= key) else { continue };
            let Some(&frame) = frames.iter().find(|f| f.before >= synced) else { continue };
            if shown.last().is_some_and(|last| last.frame == frame) {
                continue;
            }
            shown.push(Keystroke { key, synced, frame });
        }
        shown
    }

    /// Frames presented in `window` on a display refreshing at `display_hz`.
    pub fn cadence(&self, window: Window, display_hz: f64) -> Cadence {
        let frames: Vec<Frame> = self.frames().into_iter().filter(|f| window.contains(f.end)).collect();
        let intervals_ms: Vec<f64> = frames.windows(2).map(|w| ms(w[1].end - w[0].end)).collect();
        let late_ms = 1.5 * 1000.0 / display_hz;
        let span_ms = match (frames.first(), frames.last()) {
            (Some(first), Some(last)) if window.to != u64::MAX => {
                ms(window.to.saturating_sub(window.from)).max(ms(last.end - first.end))
            }
            (Some(first), Some(last)) => ms(last.end - first.end),
            _ => 0.0,
        };
        Cadence {
            presented: frames.len(),
            span_ms,
            display_hz,
            late: intervals_ms.iter().filter(|&&d| d > late_ms).count(),
            intervals_ms,
            work_ms: frames.iter().map(Frame::work_ms).collect(),
        }
    }

    /// The later of the window becoming visible and the end of the first
    /// frame drawn after the content was synced.
    pub fn content_visible(&self) -> Option<u64> {
        let content = self.content?;
        let frame = self.frames().into_iter().find(|f| f.before >= content)?;
        Some(frame.end.max(self.visible?))
    }

    /// How many window events arrived in `window`.
    pub fn events_in(&self, window: Window) -> usize {
        self.events.iter().filter(|&&t| window.contains(t)).count()
    }

    /// How often the main thread woke up in `window`.
    pub fn wakeups(&self, window: Window) -> usize {
        self.activities.iter().filter(|&&(t, a)| a == AFTER_WAITING && window.contains(t)).count()
    }
}
