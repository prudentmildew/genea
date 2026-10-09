//! PROTOTYPE. Reads GPUI's foreground journal (the `profiler` feature) on a
//! side thread and turns it into Genea budget numbers.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use gpui::{
    App,
    profiler::journal::{ForegroundEvent, ForegroundJournalEntry, IntervalBoundary, PresentedFrame},
};
use serde_json::{Value, json};

pub struct Recorder {
    entries: Arc<Mutex<Vec<ForegroundJournalEntry>>>,
    lost: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// When recording began (just after `main` reached GPUI).
    pub base: Instant,
}

impl Recorder {
    pub fn start(cx: &App) -> Self {
        let base = Instant::now();
        let mut collector = cx.foreground_journal().collector();
        let entries = Arc::new(Mutex::new(Vec::new()));
        let lost = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::spawn({
            let (entries, lost, stop) = (entries.clone(), lost.clone(), stop.clone());
            move || loop {
                let done = stop.load(Ordering::Acquire);
                let drained = collector.collect_unseen();
                lost.fetch_add(drained.lost, Ordering::Relaxed);
                entries.lock().unwrap().extend(drained.entries);
                if done {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        Self {
            entries,
            lost,
            stop,
            thread: Some(thread),
            base,
        }
    }

    /// A snapshot of everything recorded so far.
    pub fn journal(&self) -> Journal {
        Journal {
            entries: self.entries.lock().unwrap().clone(),
            lost: self.lost.load(Ordering::Relaxed),
        }
    }

    pub fn finish(mut self) -> Journal {
        self.stop.store(true, Ordering::Release);
        self.thread.take().unwrap().join().unwrap();
        self.journal()
    }
}

pub struct Journal {
    pub entries: Vec<ForegroundJournalEntry>,
    pub lost: u64,
}

impl Journal {
    pub fn events(&self, window: (Instant, Instant)) -> Vec<ForegroundEvent> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                ForegroundJournalEntry::Event(ev) => Some(*ev),
                _ => None,
            })
            .filter(|ev| ev.end_time() >= window.0 && ev.start_time() <= window.1)
            .collect()
    }

    pub fn presents(&self) -> Vec<PresentedFrame> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                ForegroundJournalEntry::Boundary(IntervalBoundary::Presented(p)) => Some(*p),
                _ => None,
            })
            .collect()
    }

    pub fn first_present(&self) -> Option<PresentedFrame> {
        self.presents().into_iter().next()
    }

    /// Longest single piece of foreground work in `window`, with what it was.
    pub fn longest_stall(&self, window: (Instant, Instant)) -> Value {
        let mut events = self.events(window);
        events.retain(|e| !matches!(e, ForegroundEvent::SmallPolls(_)));
        events.sort_by_key(|e| std::cmp::Reverse(e.duration()));
        json!({
            "max_ms": events.first().map(|e| ms(e.duration())).unwrap_or(0.0),
            "over_16ms": events.iter().filter(|e| e.duration() > Duration::from_millis(16)).count(),
            "top": events.iter().take(5).map(describe).collect::<Vec<_>>(),
        })
    }

    /// Cadence and cost of presented frames in `window`.
    pub fn frames(&self, window: (Instant, Instant)) -> Value {
        let presents: Vec<_> = self
            .presents()
            .into_iter()
            .filter(|p| p.presentation.present_end >= window.0 && p.presentation.present_end <= window.1)
            .collect();
        let mut intervals: Vec<Duration> = presents
            .windows(2)
            .map(|w| w[1].presentation.present_end.duration_since(w[0].presentation.present_end))
            .collect();
        let mut work: Vec<Duration> = presents
            .iter()
            .map(|p| {
                p.frame.draw_duration()
                    + p.presentation.present_end.duration_since(p.presentation.present_start)
            })
            .collect();
        let span = window.1.duration_since(window.0);
        let mut draw: Vec<Duration> = presents.iter().map(|p| p.frame.draw_duration()).collect();
        let display_fps = crate::synth::display_fps();
        let period = Duration::from_secs_f64(1.0 / display_fps);
        let budget = Duration::from_secs_f64(1.0 / 120.0);
        let late = intervals.iter().filter(|d| **d > period.mul_f64(1.5)).count();
        let over_budget = work.iter().filter(|d| **d > budget).count();
        json!({
            "display_fps": display_fps,
            "presented": presents.len(),
            "fps": presents.len() as f64 / span.as_secs_f64(),
            "late_intervals": late,
            "late_pct": 100.0 * late as f64 / intervals.len().max(1) as f64,
            "frames_over_8_3ms_work": over_budget,
            "interval_ms": stats(&mut intervals),
            "draw_plus_present_ms": stats(&mut work),
            "draw_ms": stats(&mut draw),
        })
    }
}

fn describe(e: &ForegroundEvent) -> Value {
    let what = match e {
        ForegroundEvent::TaskPoll(t) => format!("task {}:{}", t.location.file(), t.location.line()),
        ForegroundEvent::Action(a) => format!("action {}", a.name),
        ForegroundEvent::Input(i) => format!("input {}", i.kind),
        ForegroundEvent::Draw(_) => "draw".into(),
        ForegroundEvent::Present(_) => "present".into(),
        ForegroundEvent::SmallPolls(_) => "small polls".into(),
    };
    json!({ "ms": ms(e.duration()), "what": what })
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
