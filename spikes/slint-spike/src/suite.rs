//! PROTOTYPE. Runs every benchmark in fresh child processes and writes one
//! results file. Wrap the whole suite in `taskpolicy -c background` to pin it
//! (and its children) to the efficiency cores.

use std::{
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::{metrics, sys};

const START_RUNS: usize = 20;
const COLD_RUNS: usize = 10;

fn exe() -> PathBuf {
    std::env::current_exe().unwrap()
}

fn results_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("results");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn bench(name: &str) -> Value {
    let out = results_dir().join(format!("{name}.tmp.json"));
    let status = Command::new(exe())
        .args(["bench", name, "--out"])
        .arg(&out)
        .status()
        .unwrap();
    let Ok(text) = std::fs::read_to_string(&out) else {
        eprintln!("bench {name} failed: {status}");
        return json!({ "failed": status.to_string() });
    };
    std::fs::remove_file(out).ok();
    let value: Value = serde_json::from_str(&text).unwrap();
    if value["window_visible_at_end"] == false {
        eprintln!("bench {name}: window was occluded, frame numbers are throttled");
    }
    value
}

fn starts(runs: usize, purge: bool) -> Value {
    let mut to_frame = Vec::new();
    for _ in 0..runs {
        if purge {
            // Drops the file cache so the binary and its libraries come from disk.
            Command::new("sudo").args(["-n", "purge"]).status().unwrap();
            std::thread::sleep(Duration::from_secs(2));
        }
        if let Some(ms) = bench("start")["start_to_first_frame_ms"].as_f64() {
            to_frame.push(Duration::from_secs_f64(ms / 1000.0));
        }
    }
    json!({ "start_to_first_frame_ms": metrics::stats(&mut to_frame) })
}

/// Idle: the editor sits with one small file open and the caret blinking.
fn idle() -> Value {
    let settle = Duration::from_secs(10);
    let window = Duration::from_secs(30);
    let mut child = Command::new(exe())
        .args(["bench", "idle"])
        .env("SPIKE_IDLE_SECONDS", (settle + window).as_secs().to_string())
        .spawn()
        .unwrap();
    let pid = child.id() as i32;
    std::thread::sleep(settle);
    let a = sys::usage(pid).unwrap();
    let t = Instant::now();
    std::thread::sleep(window);
    let b = sys::usage(pid).unwrap();
    let elapsed = t.elapsed().as_secs_f64();
    child.wait().unwrap();
    json!({
        "cpu_pct": 100.0 * (b.cpu - a.cpu).as_secs_f64() / elapsed,
        "wakeups_per_s": (b.wakeups - a.wakeups) as f64 / elapsed,
        "footprint_mb": b.footprint_mb,
    })
}

pub fn run(label: &str) {
    let can_purge = Command::new("sudo")
        .args(["-n", "true"])
        .status()
        .is_ok_and(|s| s.success());
    // One throwaway launch so "warm" really is warm.
    bench("start");
    let mut results = json!({
        "label": label,
        "warm_start": starts(START_RUNS, false),
        "cold_start": if can_purge { starts(COLD_RUNS, true) } else {
            json!("skipped: needs passwordless sudo for `purge` (run `sudo -v` first)")
        },
    });
    for name in ["typing", "scroll", "open", "huge"] {
        eprintln!("[{label}] {name}");
        results[name] = bench(name);
    }
    eprintln!("[{label}] idle");
    results["idle"] = idle();
    let path = results_dir().join(format!("{label}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&results).unwrap()).unwrap();
    println!("{}", path.display());
}
