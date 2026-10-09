//! Idle: memory, CPU and wake-ups with one file open and nothing happening.
//!
//! Genea opens [`super::FILE`] in its 1200×800 pt window and is left alone.
//! After a settle well past 2 s since its last frame, the harness samples
//! the kernel's accounting from outside: `phys_footprint` once a second,
//! and CPU time and wake-ups over the whole window. Reading another
//! process's accounting doesn't wake it.
//!
//! The budgets are checked on runs with the journal on, because only the
//! journal can tell an idle Genea from one disturbed from outside: the
//! pointer crossing its window, focus or occlusion changes. A disturbed run
//! is retried (up to [`ATTEMPTS`] times). The journal also confirms the
//! window size and display scale the memory budget assumes, and counts
//! main-thread wake-ups and frames Genea drew on its own.
//!
//! Each journal run is paired with one without the journal, Genea as users
//! run it, to show that the journal costs nothing measurable while idle.
//! Those are reported, not checked: nothing can tell whether they were
//! disturbed.

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
/// From content visible (the last frame) to the first sample.
const SETTLE: Duration = Duration::from_secs(4);
/// Tries per journal run before a disturbed run is used anyway.
const ATTEMPTS: usize = 3;

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

impl Scenario for Idle {
    fn name(&self) -> &'static str {
        "idle"
    }

    fn run(&self, cx: &mut Context) -> Result<Vec<Check>, String> {
        let (mut plain, mut journaled) = (Vec::new(), Vec::new());
        let mut disturbed_runs = 0;
        for run in 0..cx.options.idle_runs {
            let sample = idle(cx, false)?;
            record(cx, run, &sample);
            plain.push(sample);
            for attempt in 1..=ATTEMPTS {
                let sample = idle(cx, true)?;
                record(cx, run, &sample);
                let disturbed = sample.journal.as_ref().is_some_and(|s| s.events > 0);
                if !disturbed || attempt == ATTEMPTS {
                    disturbed_runs += usize::from(disturbed);
                    journaled.push(sample);
                    break;
                }
                eprint!("(disturbed, again)");
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
        let (journal_mb, plain_mb) = (footprints(&journaled), footprints(&plain));
        let cpu = |s: &Sample| s.cpu_pct;
        let wakeups = |s: &Sample| s.wakeups_per_s;
        let footprint = |s: &Sample| Summary::of(&s.footprints_mb).map_or(0.0, |s| s.p50);
        cx.record(
            "result",
            json!({
                "footprint_mb": summary_json(&journal_mb),
                "cpu_pct_worst": round_small(worst(&journaled, &cpu)),
                "wakeups_per_s_worst": round(worst(&journaled, &wakeups)),
                "main_thread_wakeups_per_s_worst": round(worst(&journaled, &seen)),
                "frames_while_idle_worst": worst(&journaled, &frames),
                "disturbed_runs": disturbed_runs,
                "without_journal": {
                    "footprint_mb": summary_json(&plain_mb),
                    "cpu_pct_worst": round_small(worst(&plain, &cpu)),
                    "wakeups_per_s_worst": round(worst(&plain, &wakeups)),
                },
                "journal_cost_medians": {
                    "footprint_mb": round(median(&journaled, &footprint) - median(&plain, &footprint)),
                    "cpu_pct": round_small(median(&journaled, &cpu) - median(&plain, &cpu)),
                    "wakeups_per_s": round(median(&journaled, &wakeups) - median(&plain, &wakeups)),
                },
            }),
        );

        // The memory budget assumes a 1200×800 pt window at 2×.
        let info = journaled.iter().find_map(|s| s.journal.as_ref().map(|j| j.info.clone()));
        let as_assumed =
            info.as_ref().is_some_and(|i| i.window_width == 1200.0 && i.window_height == 800.0 && i.window_scale == 2.0);
        let disturbed_note =
            (disturbed_runs > 0).then(|| format!("{disturbed_runs} run(s) disturbed from outside after {ATTEMPTS} tries"));
        let mut memory = match (&info, Summary::of(&journal_mb)) {
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
        let cpu_check = budgets::IDLE_CPU.check(Some(worst(&journaled, &cpu))).with_note(format!(
            "{} wake-ups/s, {} on the main thread, {} frames{}",
            round(worst(&journaled, &wakeups)),
            round(worst(&journaled, &seen)),
            worst(&journaled, &frames),
            disturbed_note.map(|n| format!("; {n}")).unwrap_or_default()
        ));
        Ok(vec![memory, cpu_check])
    }
}

fn record(cx: &mut Context, run: usize, sample: &Sample) {
    cx.record(
        "run",
        json!({
            "run": run,
            "journal": sample.journal.is_some(),
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

fn idle(cx: &mut Context, journal: bool) -> Result<Sample, String> {
    let mut genea = cx.launch(journal)?;
    let info = if journal {
        genea.wait_content()?;
        Some(require_visible(&mut genea)?)
    } else {
        sleep(START_ALLOWANCE);
        None
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
