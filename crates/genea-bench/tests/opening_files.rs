//! Reading the journal for file opens (ticket #27): from the harness's
//! request to the first frame showing the file.
//!
//! Journals here are written by hand in milliseconds; the journal itself
//! counts nanoseconds since boot.

use genea_bench::journal::{AFTER_WAITING, BEFORE_SOURCES, BEFORE_TIMERS, BEFORE_WAITING, Journal, Open};

fn ns(ms: f64) -> u64 {
    (ms * 1_000_000.0).round() as u64
}

/// A frame drawn in one wake-up.
fn frame_wakeup(at: f64) -> Vec<(f64, u32)> {
    vec![(at, AFTER_WAITING), (at + 0.5, BEFORE_TIMERS), (at + 1.0, BEFORE_SOURCES), (at + 4.0, BEFORE_WAITING)]
}

#[test]
fn an_open_lasts_from_the_request_to_the_first_frame_presented_after_the_file_showed() {
    let mut activities = frame_wakeup(100.0); // a frame before the file showed
    activities.extend(frame_wakeup(140.0));
    let j = Journal {
        activities: activities.iter().map(|&(t, a)| (ns(t), a)).collect(),
        before: vec![ns(101.5), ns(141.5)],
        after: vec![ns(103.0), ns(143.0)],
        opens: vec![(ns(90.0), Some(ns(120.0))), (ns(200.0), None)],
        ..Journal::default()
    };

    assert_eq!(j.open(0), Some(Open { requested: ns(90.0), presented: ns(144.0) }));
    assert_eq!(j.open(0).unwrap().ms(), 54.0);
    assert_eq!(j.open(1), None, "never shown");
    assert_eq!(j.open(2), None, "never asked for");
}

#[test]
fn opens_read_from_genea_s_dump() {
    let dump = serde_json::json!({ "opens": [[7, 9], [10, null]] });
    let j = Journal::from_json(&dump).unwrap();
    assert_eq!(j.opens, vec![(7, Some(9)), (10, None)]);
    assert!(Journal::from_json(&serde_json::json!({ "opens": [[7]] })).is_err());
}
