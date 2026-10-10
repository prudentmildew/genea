//! Reading the journal for navigation (ticket #63): from a keystroke in the
//! finder, a search request, a click on a folder or a file written outside
//! Genea, to the first frame showing what it did.
//!
//! Journals here are written by hand in milliseconds; the journal itself
//! counts nanoseconds since boot.

use genea_bench::journal::{AFTER_WAITING, BEFORE_SOURCES, BEFORE_TIMERS, BEFORE_WAITING, Journal};

fn ns(ms: f64) -> u64 {
    (ms * 1_000_000.0).round() as u64
}

/// A frame drawn in one wake-up.
fn frame_wakeup(at: f64) -> Vec<(f64, u32)> {
    vec![(at, AFTER_WAITING), (at + 0.5, BEFORE_TIMERS), (at + 1.0, BEFORE_SOURCES), (at + 4.0, BEFORE_WAITING)]
}

/// Frames drawn at each of `starts` (ms).
fn frames_at(starts: &[f64]) -> Journal {
    let mut j = Journal::default();
    for &start in starts {
        j.activities.extend(frame_wakeup(start).into_iter().map(|(t, a)| (ns(t), a)));
        j.before.push(ns(start + 1.5));
        j.after.push(ns(start + 3.0));
    }
    j
}

#[test]
fn a_reaction_lasts_until_the_first_frame_drawn_after_its_effect_reached_the_view() {
    // Requested at 200 ms; a frame at 210 ms still shows the old results;
    // the results reach Slint at 230 ms and the frame at 235 ms shows them.
    let j = frames_at(&[210.0, 235.0]);

    let reaction = j.reaction(ns(200.0), ns(230.0)).expect("a frame showed it");
    assert_eq!(reaction.frame.end, ns(239.0));
    // Genea's work leaves out the wait for the display-link tick:
    // (230 − 200) + (239 − 236).
    assert_eq!(reaction.genea_work_ms(), 33.0);
    assert_eq!(reaction.input_to_present_ms(), 39.0);
}

#[test]
fn a_reaction_not_shown_yet_has_no_frame() {
    let j = frames_at(&[210.0]);
    assert_eq!(j.reaction(ns(200.0), ns(230.0)), None);
}

#[test]
fn the_key_a_reaction_starts_from_is_the_first_one_genea_received_after_the_harness_sent_it() {
    let j = Journal { keys: vec![ns(100.0), ns(305.0), ns(330.0)], ..Journal::default() };
    assert_eq!(j.key_after(ns(300.0)), Some(ns(305.0)));
    assert_eq!(j.key_after(ns(400.0)), None);
}
