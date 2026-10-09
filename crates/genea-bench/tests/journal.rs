//! Reading Genea's instrumentation journal: frames, stalls, keystrokes,
//! frame cadence, milestones and wake-ups.
//!
//! Journals here are written by hand in milliseconds; the journal itself
//! counts nanoseconds since boot.

use genea_bench::journal::{AFTER_WAITING, BEFORE_SOURCES, BEFORE_TIMERS, BEFORE_WAITING, Journal, Window};

fn ns(ms: f64) -> u64 {
    (ms * 1_000_000.0).round() as u64
}

fn all() -> Window {
    Window { from: 0, to: u64::MAX }
}

/// A journal from run-loop activities and rendering marks, in ms.
fn journal(activities: &[(f64, u32)], before: &[f64], after: &[f64]) -> Journal {
    Journal {
        activities: activities.iter().map(|&(t, a)| (ns(t), a)).collect(),
        before: before.iter().map(|&t| ns(t)).collect(),
        after: after.iter().map(|&t| ns(t)).collect(),
        ..Journal::default()
    }
}

/// A frame drawn in one wake-up: woke, timers, sources (the display-link
/// tick: BeforeRendering, AfterRendering), then waiting again.
fn frame_wakeup(at: f64) -> Vec<(f64, u32)> {
    vec![(at, AFTER_WAITING), (at + 0.5, BEFORE_TIMERS), (at + 1.0, BEFORE_SOURCES), (at + 4.0, BEFORE_WAITING)]
}

#[test]
fn a_frame_spans_the_run_loop_segments_its_rendering_marks_fall_in() {
    let j = journal(&frame_wakeup(100.0), &[101.5], &[103.0]);
    let frames = j.frames();
    assert_eq!(frames.len(), 1);
    let f = frames[0];
    // From the segment start before BeforeRendering to the first activity
    // after AfterRendering (present returned).
    assert_eq!((f.start, f.before, f.after, f.end), (ns(101.0), ns(101.5), ns(103.0), ns(104.0)));
    assert_eq!(f.work_ms(), 3.0);
}

#[test]
fn a_frame_still_in_flight_is_not_a_frame() {
    let j = journal(&[(100.0, AFTER_WAITING), (101.0, BEFORE_SOURCES)], &[101.5], &[103.0]);
    assert!(j.frames().is_empty());
}

#[test]
fn a_stall_is_a_main_thread_busy_span_from_waking_to_waiting() {
    let mut activities = frame_wakeup(0.0); // 4 ms busy
    activities.extend([(50.0, AFTER_WAITING), (52.0, BEFORE_SOURCES), (70.5, BEFORE_WAITING)]); // 20.5 ms
    activities.extend([(100.0, AFTER_WAITING), (117.0, BEFORE_WAITING)]); // 17 ms
    let j = journal(&activities, &[], &[]);

    let stalls = j.stalls(all());
    assert_eq!(stalls.max_ms, 20.5);
    assert_eq!(stalls.over_16ms, 2);
    assert_eq!(stalls.top_ms, vec![20.5, 17.0, 4.0]);

    // Only spans that overlap the window count.
    let stalls = j.stalls(Window { from: ns(90.0), to: ns(200.0) });
    assert_eq!(stalls.max_ms, 17.0);
    assert_eq!(stalls.over_16ms, 1);
}

#[test]
fn the_first_busy_span_starts_when_the_journal_started() {
    // Startup: the main thread is busy from launch until it first waits.
    let mut j = journal(&[(30.0, BEFORE_SOURCES), (90.0, BEFORE_WAITING)], &[], &[]);
    j.started = ns(10.0);
    assert_eq!(j.stalls(all()).max_ms, 80.0);
}

#[test]
fn keystroke_work_is_input_to_sync_plus_the_frame_that_shows_it() {
    // Key at 200 ms, applied and synced to Slint at 201 ms, then the next
    // display-link wake-up at 205 ms draws it.
    let mut activities = vec![(199.5, AFTER_WAITING), (201.5, BEFORE_WAITING)];
    activities.extend(frame_wakeup(205.0));
    let mut j = journal(&activities, &[206.5], &[208.0]);
    j.keys = vec![ns(200.0)];
    j.synced = vec![ns(201.0)];

    let keys = j.keystrokes(all());
    assert_eq!(keys.len(), 1);
    // Genea's work leaves out the wait for the display-link tick:
    // (201 − 200) + (209 − 206).
    assert_eq!(keys[0].genea_work_ms(), 4.0);
    assert_eq!(keys[0].input_to_present_ms(), 9.0);
}

#[test]
fn keystrokes_shown_by_the_same_frame_count_once_from_the_earliest() {
    // A dead-key commit marks both the key press and the IME commit.
    let mut activities = vec![(199.5, AFTER_WAITING), (201.5, BEFORE_WAITING)];
    activities.extend(frame_wakeup(205.0));
    let mut j = journal(&activities, &[206.5], &[208.0]);
    j.keys = vec![ns(200.0), ns(200.1)];
    j.synced = vec![ns(201.0)];

    let keys = j.keystrokes(all());
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].input_to_present_ms(), 9.0);
}

#[test]
fn a_keystroke_without_a_frame_yet_is_left_out() {
    let mut j = journal(&[(199.5, AFTER_WAITING), (201.5, BEFORE_WAITING)], &[], &[]);
    j.keys = vec![ns(200.0)];
    j.synced = vec![ns(201.0)];
    assert!(j.keystrokes(all()).is_empty());
}

#[test]
fn a_frame_interval_over_one_and_a_half_display_periods_is_a_dropped_frame() {
    // 120 Hz: ends 8.33 ms apart, then one 25 ms gap (two frames missed).
    let mut activities = Vec::new();
    let (mut before, mut after) = (Vec::new(), Vec::new());
    for start in [0.0, 8.333, 16.667, 41.667, 50.0] {
        activities.extend(frame_wakeup(start));
        before.push(start + 1.5);
        after.push(start + 3.0);
    }
    let j = journal(&activities, &before, &after);

    let cadence = j.cadence(all(), 120.0);
    assert_eq!(cadence.presented, 5);
    assert_eq!(cadence.intervals_ms.len(), 4);
    assert_eq!(cadence.late, 1);
    assert_eq!(cadence.dropped_pct(), 25.0);
    assert_eq!(cadence.work_ms, vec![3.0; 5]);
}

#[test]
fn content_is_visible_once_the_window_is_and_a_frame_showing_it_has_presented() {
    let mut activities = frame_wakeup(100.0); // a frame before the content
    activities.extend(frame_wakeup(140.0));
    let mut j = journal(&activities, &[101.5, 141.5], &[103.0, 143.0]);
    j.content = Some(ns(120.0));

    j.visible = Some(ns(130.0));
    assert_eq!(j.content_visible(), Some(ns(144.0)), "the frame showing the content came later");
    j.visible = Some(ns(150.0));
    assert_eq!(j.content_visible(), Some(ns(150.0)), "the window became visible later");
    j.visible = None;
    assert_eq!(j.content_visible(), None);
}

#[test]
fn wakeups_count_the_times_the_main_thread_woke() {
    let mut activities = frame_wakeup(0.0);
    activities.extend(frame_wakeup(1000.0));
    activities.extend(frame_wakeup(2000.0));
    let j = journal(&activities, &[], &[]);
    assert_eq!(j.wakeups(Window { from: ns(500.0), to: ns(3000.0) }), 2);
}

#[test]
fn a_journal_reads_from_genea_s_dump() {
    let dump = serde_json::json!({
        "process_start": 5_000_000,
        "started": 6_000_000,
        "activities": [[10_000_000, AFTER_WAITING], [11_000_000, BEFORE_WAITING]],
        "keys": [7], "before": [8], "after": [9], "synced": [10],
        "content": 12, "visible": null,
    });
    let j = Journal::from_json(&dump).unwrap();
    assert_eq!(j.process_start, 5_000_000);
    assert_eq!(j.started, 6_000_000);
    assert_eq!(j.activities, vec![(10_000_000, AFTER_WAITING), (11_000_000, BEFORE_WAITING)]);
    assert_eq!((j.keys[0], j.before[0], j.after[0], j.synced[0]), (7, 8, 9, 10));
    assert_eq!((j.content, j.visible), (Some(12), None));
    assert!(Journal::from_json(&serde_json::json!({ "keys": "nope" })).is_err());
}
