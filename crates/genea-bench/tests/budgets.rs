//! Budget checks: a missed Genea budget fails the run; end-to-end targets
//! are only reported.

use genea_bench::budgets::{Budget, Kind, Verdict, verdicts};

const KEYSTROKE: Budget = Budget { id: "typing.keystroke", title: "Keystroke to frame (p95)", limit: 8.0, unit: "ms", kind: Kind::Budget };
const COMPLETIONS: Budget = Budget { id: "ls.completions", title: "Completions (p95)", limit: 100.0, unit: "ms", kind: Kind::Target };

#[test]
fn a_budget_passes_at_or_under_its_limit_and_fails_over_it() {
    assert_eq!(KEYSTROKE.check(Some(8.0)).verdict(), Verdict::Pass);
    assert_eq!(KEYSTROKE.check(Some(8.01)).verdict(), Verdict::Fail);
}

#[test]
fn a_missed_budget_fails_the_run() {
    let checks = [KEYSTROKE.check(Some(4.0)), KEYSTROKE.check(Some(9.5))];
    let summary = verdicts(&checks);
    assert!(!summary.passed());
    assert_eq!(summary.exit_code(), 1);
}

#[test]
fn a_missed_end_to_end_target_is_reported_but_does_not_fail_the_run() {
    let checks = [KEYSTROKE.check(Some(4.0)), COMPLETIONS.check(Some(140.0))];
    assert_eq!(checks[1].verdict(), Verdict::Missed);
    assert_eq!(verdicts(&checks).exit_code(), 0);
}

#[test]
fn a_budget_without_a_measurement_is_skipped_with_the_reason() {
    let check = KEYSTROKE.skipped("needs a 2× display");
    assert_eq!(check.verdict(), Verdict::Skipped);
    let summary = verdicts(&[check]);
    assert_eq!(summary.exit_code(), 0);
    assert_eq!(summary.skipped, 1);
}

#[test]
fn the_summary_lists_every_check_with_its_measurement_and_limit() {
    let checks = [KEYSTROKE.check(Some(4.25)), KEYSTROKE.check(Some(9.5)), KEYSTROKE.skipped("no display"), COMPLETIONS.check(Some(140.0))];
    let text = verdicts(&checks).text();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "PASS  Keystroke to frame (p95): 4.25 ms (limit 8 ms)");
    assert_eq!(lines[1], "FAIL  Keystroke to frame (p95): 9.5 ms (limit 8 ms)");
    assert_eq!(lines[2], "SKIP  Keystroke to frame (p95): no display");
    assert_eq!(lines[3], "MISS  Completions (p95): 140 ms (target 100 ms, report only)");
    assert_eq!(lines.last().unwrap(), &"FAILED: 1 of 3 Genea budgets missed, 1 skipped");
}

#[test]
fn a_check_is_one_jsonl_record() {
    let record = KEYSTROKE.check(Some(4.25)).to_json();
    assert_eq!(record["type"], "check");
    assert_eq!(record["id"], "typing.keystroke");
    assert_eq!(record["measured"], 4.25);
    assert_eq!(record["limit"], 8.0);
    assert_eq!(record["verdict"], "pass");
}
