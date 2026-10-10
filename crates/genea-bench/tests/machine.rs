//! Results record the machine they came from (ticket #63), so dev-machine
//! runs aren't mistaken for reference-machine runs.

use genea_bench::report::{displays, machine_line};
use serde_json::json;

#[test]
fn displays_are_read_from_system_profiler_main_display_first() {
    let profile = json!({ "SPDisplaysDataType": [{
        "_name": "Apple M5",
        "spdisplays_ndrvs": [
            { "_name": "LG ULTRAFINE", "_spdisplays_resolution": "1920 x 1080 @ 60.00Hz" },
            { "_name": "Built-in Liquid Retina XDR Display", "_spdisplays_resolution": "1512 x 982 @ 120.00Hz",
              "spdisplays_main": "spdisplays_yes" },
        ],
    }]});
    assert_eq!(
        displays(&profile),
        ["Built-in Liquid Retina XDR Display 1512 x 982 @ 120.00Hz", "LG ULTRAFINE 1920 x 1080 @ 60.00Hz"]
    );
    assert!(displays(&json!({})).is_empty());
}

#[test]
fn the_summary_says_whether_the_results_are_from_the_reference_machine() {
    let machine = json!({
        "model": "Mac17,3", "chip": "Apple M5", "macos": "27.2", "cores": 10, "memory_gb": 32.0,
        "displays": ["LG ULTRAFINE 1920 x 1080 @ 60.00Hz"], "reference": false,
    });
    assert_eq!(
        machine_line(&machine),
        "Mac17,3 (Apple M5, 10 cores, 32 GB), macOS 27.2, LG ULTRAFINE 1920 x 1080 @ 60.00Hz; \
         a dev machine, NOT the reference machine"
    );

    let mut reference = machine.clone();
    reference["reference"] = json!(true);
    assert!(machine_line(&reference).ends_with("; the reference machine"));
}
