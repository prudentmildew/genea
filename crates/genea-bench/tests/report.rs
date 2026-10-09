//! Result files are named by their UTC start time.

use genea_bench::report::utc_label;

#[test]
fn a_run_is_labelled_with_its_utc_start_time() {
    assert_eq!(utc_label(0), "1970-01-01T00-00-00Z");
    // 2026-10-09 21:30:05 UTC.
    assert_eq!(utc_label(1_791_581_405), "2026-10-09T21-30-05Z");
    // A leap day.
    assert_eq!(utc_label(1_709_210_096), "2024-02-29T12-34-56Z");
}
