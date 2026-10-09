//! Start times are judged as a margin over the start floor measured in the
//! same session (GLOSSARY.md, "Start floor").

use genea_bench::stats::margin_over_floor;

#[test]
fn the_margin_is_genea_s_p95_minus_the_floor_s_p95() {
    let genea = [150.0, 160.0, 170.0, 155.0, 210.0];
    let floor = [120.0, 125.0, 118.0, 127.0, 160.0];
    let margin = margin_over_floor(&genea, &floor).unwrap();
    assert_eq!(margin.genea.p95, 210.0);
    assert_eq!(margin.floor.p95, 160.0);
    assert_eq!(margin.p95_ms, 50.0);
    assert_eq!(margin.p50_ms, 35.0);
}

#[test]
fn without_floor_runs_there_is_no_margin() {
    assert!(margin_over_floor(&[150.0], &[]).is_none());
    assert!(margin_over_floor(&[], &[120.0]).is_none());
}
