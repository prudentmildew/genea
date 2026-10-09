//! Scrolling: frame cadence, frame work and stalls.
//!
//! Genea scrolls [`super::FILE`] by a fixed step per presented frame through
//! its `scrolled` callback (the trackpad's path), turning around at either
//! end: a steady phase (30 pt a frame) and a fling (200 pt a frame, so every
//! frame shows all-new lines). A frame interval longer than 1.5 display
//! periods is a dropped frame.
//!
//! The budget is 120 fps with ≤ 1 % dropped. On a display slower than
//! 120 Hz the harness checks dropped frames at the display's rate and that
//! each frame's work fits a 120 Hz frame (8.3 ms), and says so.

use std::time::Duration;

use serde_json::json;

use super::{Context, Scenario, require_visible, sleep};
use crate::{
    budgets::{self, Check},
    journal::Window,
    stats::{Summary, round, summary_json},
    sys::now_ns,
};

pub struct Scroll;

impl Scenario for Scroll {
    fn name(&self) -> &'static str {
        "scroll"
    }

    fn run(&self, cx: &mut Context) -> Result<Vec<Check>, String> {
        let mut genea = cx.launch(true)?;
        genea.wait_content()?;
        let info = require_visible(&mut genea)?;
        sleep(Duration::from_millis(500));

        let seconds = cx.options.scroll_seconds;
        let mut phases = Vec::new();
        for (phase, px, duration) in [
            ("steady", 30.0, Duration::from_secs(seconds)),
            ("fling", 200.0, Duration::from_secs(seconds.div_ceil(2))),
        ] {
            let from = now_ns();
            genea.scroll(px, duration)?;
            phases.push((phase, px, Window { from, to: now_ns() }));
            sleep(Duration::from_millis(300));
        }
        let journal = genea.journal()?;
        genea.quit();

        let (mut dropped, mut work_p95, mut stall) = (0.0f64, 0.0f64, 0.0f64);
        for (phase, px, window) in phases {
            let cadence = journal.cadence(window, info.display_hz);
            let stalls = journal.stalls(window);
            if cadence.presented < 10 {
                return Err(format!("{phase} scrolling presented only {} frames", cadence.presented));
            }
            let work = Summary::of(&cadence.work_ms).map_or(0.0, |s| s.p95);
            dropped = dropped.max(cadence.dropped_pct());
            work_p95 = work_p95.max(work);
            stall = stall.max(stalls.max_ms);
            cx.record(
                "result",
                json!({
                    "phase": phase,
                    "px_per_frame": px,
                    "display_hz": info.display_hz,
                    "presented": cadence.presented,
                    "fps": round(cadence.fps()),
                    "late": cadence.late,
                    "dropped_pct": round(cadence.dropped_pct()),
                    "interval_ms": summary_json(&cadence.intervals_ms),
                    "frame_work_ms": summary_json(&cadence.work_ms),
                    "stalls": stalls.to_json(window.from),
                }),
            );
        }
        let hz = info.display_hz;
        let mut dropped_check = budgets::SCROLL_DROPPED.check(Some(dropped));
        if hz < 120.0 {
            dropped_check = dropped_check.with_note(format!("display is {hz} Hz; 120 Hz needs a ProMotion display"));
        }
        Ok(vec![dropped_check, budgets::SCROLL_FRAME_WORK.check(Some(work_p95)), budgets::SCROLL_STALL.check(Some(stall))])
    }
}
