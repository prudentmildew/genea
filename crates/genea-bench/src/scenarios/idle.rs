//! Idle: memory, CPU and wake-ups with one file open and nothing happening.
//!
//! Genea opens [`super::FILE`] and is left alone. After a settle well past
//! 2 s since its last frame, the harness samples the kernel's accounting
//! from outside: `phys_footprint` once a second, and CPU time and wake-ups
//! over the whole window. Reading another process's accounting doesn't wake
//! it.
//!
//! The budgets are checked on runs with the journal on, because only the
//! journal can tell an idle Genea from one disturbed from outside: the
//! pointer crossing its window, focus or occlusion changes. A disturbed run
//! is retried (up to [`ATTEMPTS`] times). Those runs also resize the window to
//! the 1200×800 pt the memory budget assumes, confirm the 2× scale, and
//! count main-thread wake-ups and frames Genea drew on its own.
//!
//! Each round also runs Genea as users run it (no journal) next to a journal
//! run at the same, default window size, to show that the journal costs
//! nothing measurable while idle. Those are reported, not checked.

use std::time::{Duration, Instant};

use serde_json::json;

use super::{Context, Scenario, require_visible, sleep};
use crate::{
    budgets::{self, Check},
    genea::Info,
    journal::Window,
    stats::{Summary, round, summary_json},
    sys::{self, now_ns},
};

pub struct Idle;

/// Without the journal there is no content milestone to wait for, so wait
/// this long after launch (a warm start is well under 1 s).
const START_ALLOWANCE: Duration = Duration::from_secs(3);
/// From the last frame (content visible, or the resize) to the first sample.
const SETTLE: Duration = Duration::from_secs(4);
/// Tries per journal run before a disturbed run is used anyway.
const ATTEMPTS: usize = 3;
/// The window the memory budget is defined for, in points.
const BUDGET_WINDOW: (f64, f64) = (1200.0, 800.0);

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// No journal: Genea as users run it, at its default window size.
    Plain,
    /// The journal on, at the default window size: to compare with `Plain`.
    Journal,
    /// The journal on, the window resized to the budget's size.
    Budget,
}

struct Sample {
    footprints_mb: Vec<f64>,
    cpu_pct: f64,
    wakeups_per_s: f64,
    /// With the journal only.
    journal: Option<Seen>,
}

struct Seen {
    info: Info,
    main_thread_wakeups_per_s: f64,
    /// Frames Genea presented during the window.
    frames: usize,
    /// Window events from outside during the window.
    events: usize,
}

impl Sample {
    fn disturbed(&self) -> bool {
        self.journal.as_ref().is_some_and(|s| s.events > 0)
    }
}

impl Scenario for Idle {
    fn name(&self) -> &'static str {
        "idle"
    }

    fn run(&self, cx: &mut Context) -> Result<Vec<Check>, String> {
        let (mut plain, mut journal, mut budget) = (Vec::new(), Vec::new(), Vec::new());
        let mut disturbed_runs = 0;
        for run in 0..cx.options.idle_runs {
            for mode in [Mode::Plain, Mode::Journal, Mode::Budget] {
                for attempt in 1..=ATTEMPTS {
                    let sample = idle(cx, mode)?;
                    record(cx, run, mode, &sample);
                    let disturbed = sample.disturbed();
                    if !disturbed || attempt == ATTEMPTS {
                        if mode == Mode::Budget {
                            disturbed_runs += usize::from(disturbed);
                        }
                        match mode {
                            Mode::Plain => plain.push(sample),
                            Mode::Journal => journal.push(sample),
                            Mode::Budget => budget.push(sample),
                        }
                        break;
                    }
                    eprint!("(disturbed, again)");
                }
            }
            eprint!(".");
        }
        eprintln!();

        let footprints = |s: &[Sample]| s.iter().flat_map(|s| s.footprints_mb.iter().copied()).collect::<Vec<_>>();
        let worst = |s: &[Sample], f: &dyn Fn(&Sample) -> f64| s.iter().map(f).fold(0.0, f64::max);
        let median = |s: &[Sample], f: &dyn Fn(&Sample) -> f64| {
            Summary::of(&s.iter().map(f).collect::<Vec<_>>()).map_or(0.0, |s| s.p50)
        };
        let seen = |s: &Sample| s.journal.as_ref().map_or(0.0, |j| j.main_thread_wakeups_per_s);
        let frames = |s: &Sample| s.journal.as_ref().map_or(0.0, |j| j.frames as f64);
        let cpu = |s: &Sample| s.cpu_pct;
        let wakeups = |s: &Sample| s.wakeups_per_s;
        // Each run's settled footprint: its p95, since memory can still be
        // settling early in the window.
        let footprint = |s: &Sample| Summary::of(&s.footprints_mb).map_or(0.0, |s| s.p95);
        let budget_mb = footprints(&budget);
        let default_window = journal.iter().find_map(|s| s.journal.as_ref().map(|j| j.info.clone()));
        cx.record(
            "result",
            json!({
                "footprint_mb": summary_json(&budget_mb),
                "cpu_pct_worst": round_small(worst(&budget, &cpu)),
                "wakeups_per_s_worst": round(worst(&budget, &wakeups)),
                "main_thread_wakeups_per_s_worst": round(worst(&budget, &seen)),
                "frames_while_idle_worst": worst(&budget, &frames),
                "disturbed_runs": disturbed_runs,
                "default_window": default_window.as_ref().map(|i| json!({
                    "width": i.window_width, "height": i.window_height, "scale": i.window_scale,
                })),
                "without_journal": {
                    "footprint_mb": summary_json(&footprints(&plain)),
                    "cpu_pct_worst": round_small(worst(&plain, &cpu)),
                    "wakeups_per_s_worst": round(worst(&plain, &wakeups)),
                },
                "with_journal_same_window": {
                    "footprint_mb": summary_json(&footprints(&journal)),
                    "cpu_pct_worst": round_small(worst(&journal, &cpu)),
                    "wakeups_per_s_worst": round(worst(&journal, &wakeups)),
                },
                "journal_cost_medians_of_runs": {
                    "footprint_mb": round(median(&journal, &footprint) - median(&plain, &footprint)),
                    "cpu_pct": round_small(median(&journal, &cpu) - median(&plain, &cpu)),
                    "wakeups_per_s": round(median(&journal, &wakeups) - median(&plain, &wakeups)),
                },
            }),
        );

        // The memory budget assumes a 1200×800 pt window at 2×.
        let info = budget.iter().find_map(|s| s.journal.as_ref().map(|j| j.info.clone()));
        let as_assumed = info.as_ref().is_some_and(|i| {
            (i.window_width, i.window_height) == BUDGET_WINDOW && i.window_scale == 2.0
        });
        let disturbed_note =
            (disturbed_runs > 0).then(|| format!("{disturbed_runs} run(s) disturbed from outside after {ATTEMPTS} tries"));
        let mut memory = match (&info, Summary::of(&budget_mb)) {
            (Some(_), Some(s)) if as_assumed => budgets::IDLE_MEMORY.check(Some(s.p95)),
            (Some(i), Some(_)) => budgets::IDLE_MEMORY.skipped(&format!(
                "the window was {}×{} pt at {}×, not 1200×800 at 2×",
                i.window_width, i.window_height, i.window_scale
            )),
            _ => budgets::IDLE_MEMORY.check(None),
        };
        if let Some(note) = &disturbed_note
            && memory.measured.is_some()
        {
            memory = memory.with_note(note.clone());
        }
        let cpu_check = budgets::IDLE_CPU.check(Some(worst(&budget, &cpu))).with_note(format!(
            "{} wake-ups/s, {} on the main thread, {} frames{}",
            round(worst(&budget, &wakeups)),
            round(worst(&budget, &seen)),
            worst(&budget, &frames),
            disturbed_note.map(|n| format!("; {n}")).unwrap_or_default()
        ));
        Ok(vec![memory, cpu_check])
    }
}

fn record(cx: &mut Context, run: usize, mode: Mode, sample: &Sample) {
    cx.record(
        "run",
        json!({
            "run": run,
            "mode": match mode { Mode::Plain => "no journal", Mode::Journal => "journal", Mode::Budget => "journal, 1200×800" },
            "footprint_mb": summary_json(&sample.footprints_mb),
            "cpu_pct": round_small(sample.cpu_pct),
            "wakeups_per_s": round(sample.wakeups_per_s),
            "main_thread_wakeups_per_s": sample.journal.as_ref().map(|j| round(j.main_thread_wakeups_per_s)),
            "frames": sample.journal.as_ref().map(|j| j.frames),
            "outside_events": sample.journal.as_ref().map(|j| j.events),
            "window": sample.journal.as_ref().map(|j| json!({
                "width": j.info.window_width, "height": j.info.window_height, "scale": j.info.window_scale,
            })),
        }),
    );
}

fn idle(cx: &mut Context, mode: Mode) -> Result<Sample, String> {
    let mut genea = cx.launch(mode != Mode::Plain)?;
    let info = match mode {
        Mode::Plain => {
            sleep(START_ALLOWANCE);
            None
        }
        Mode::Journal => {
            genea.wait_content()?;
            Some(require_visible(&mut genea)?)
        }
        // Slint sizes a new window to its layout, so the size can only be
        // set once it is shown.
        Mode::Budget => {
            genea.wait_content()?;
            genea.resize(BUDGET_WINDOW.0, BUDGET_WINDOW.1)?;
            Some(require_visible(&mut genea)?)
        }
    };
    sleep(SETTLE);

    let pid = genea.pid();
    let gone = || format!("Genea (pid {pid}) exited while idle");
    let (from, a) = (now_ns(), sys::usage(pid).ok_or_else(gone)?);
    let started = Instant::now();
    let mut footprints_mb = Vec::new();
    while started.elapsed() < Duration::from_secs(cx.options.idle_seconds) {
        sleep(Duration::from_secs(1));
        footprints_mb.push(sys::usage(pid).ok_or_else(gone)?.footprint_mb());
    }
    let (to, b) = (now_ns(), sys::usage(pid).ok_or_else(gone)?);
    let seconds = (to - from) as f64 / 1e9;
    let seen = match info {
        Some(info) => {
            let journal = genea.journal()?;
            let window = Window { from, to };
            Some(Seen {
                main_thread_wakeups_per_s: journal.wakeups(window) as f64 / seconds,
                frames: journal.cadence(window, info.display_hz.max(1.0)).presented,
                events: journal.events_in(window),
                info,
            })
        }
        None => None,
    };
    genea.quit();
    Ok(Sample {
        footprints_mb,
        cpu_pct: 100.0 * (b.cpu_ns - a.cpu_ns) as f64 / 1e9 / seconds,
        wakeups_per_s: (b.wakeups - a.wakeups) as f64 / seconds,
        journal: seen,
    })
}

/// CPU percentages near zero need more than two decimals.
fn round_small(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}
