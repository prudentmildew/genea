//! Summary statistics over samples: what every scenario reports.

use genea_bench::stats::Summary;

#[test]
fn p95_of_twenty_samples_is_the_nineteenth_smallest() {
    // Shuffled 1..=20: the nearest-rank p95 of 20 samples is index 18.
    let samples = [7.0, 20.0, 1.0, 13.0, 2.0, 19.0, 3.0, 18.0, 4.0, 17.0, 5.0, 16.0, 6.0, 15.0, 8.0, 14.0, 9.0, 12.0, 10.0, 11.0];
    let summary = Summary::of(&samples).unwrap();
    assert_eq!(summary.n, 20);
    assert_eq!(summary.p50, 11.0);
    assert_eq!(summary.p95, 19.0);
    assert_eq!(summary.max, 20.0);
}

#[test]
fn one_sample_is_every_percentile() {
    let summary = Summary::of(&[4.2]).unwrap();
    assert_eq!((summary.p50, summary.p95, summary.p99, summary.max), (4.2, 4.2, 4.2, 4.2));
}

#[test]
fn no_samples_have_no_summary() {
    assert!(Summary::of(&[]).is_none());
}
