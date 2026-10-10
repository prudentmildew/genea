//! Where results go: one JSONL file per run, plus the pass/fail summary.
//!
//! Every line is a JSON object with a `type`:
//! - `session`: the machine, Genea's commit, the workspace fingerprint;
//! - `run`: one measured run of a scenario (a start, an idle sample, …);
//! - `result`: a scenario's summary statistics;
//! - `check`: one budget with its measurement and verdict;
//! - `error`: a scenario that couldn't run;
//! - `verdict`: the run's outcome and exit code.

use std::{
    fs::File,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::{Value, json};

pub struct Output {
    path: PathBuf,
    file: BufWriter<File>,
}

impl Output {
    pub fn create(path: &Path) -> Result<Output, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Output { path: path.to_path_buf(), file: BufWriter::new(file) })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends one line, flushed so a crashed run keeps what it measured.
    pub fn write(&mut self, value: &Value) {
        let _ = writeln!(self.file, "{value}");
        let _ = self.file.flush();
    }
}

/// The machine the results are from (#63 compares them across machines):
/// model, chip, memory, macOS, displays, and whether the person running
/// the harness said it is the reference machine (`--reference-machine`).
pub fn machine(reference: bool) -> Value {
    let run = |program: &str, args: &[&str]| {
        Command::new(program)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let memory_gb = run("sysctl", &["-n", "hw.memsize"]).and_then(|s| s.parse::<f64>().ok()).map(|b| b / 1024f64.powi(3));
    json!({
        "model": run("sysctl", &["-n", "hw.model"]),
        "chip": run("sysctl", &["-n", "machdep.cpu.brand_string"]),
        "cores": run("sysctl", &["-n", "hw.ncpu"]).and_then(|s| s.parse::<u32>().ok()),
        "memory_gb": memory_gb,
        "macos": run("sw_vers", &["-productVersion"]),
        "macos_build": run("sw_vers", &["-buildVersion"]),
        "displays": run("system_profiler", &["SPDisplaysDataType", "-json"])
            .and_then(|json| serde_json::from_str::<Value>(&json).ok())
            .map(|profile| displays(&profile)),
        "reference": reference,
    })
}

/// The displays in `system_profiler SPDisplaysDataType -json` output, the
/// main one first: name, resolution and refresh rate.
pub fn displays(profile: &Value) -> Vec<String> {
    let mut found: Vec<(bool, String)> = Vec::new();
    for gpu in profile["SPDisplaysDataType"].as_array().into_iter().flatten() {
        for display in gpu["spdisplays_ndrvs"].as_array().into_iter().flatten() {
            let name = display["_name"].as_str().unwrap_or("display");
            let mode = display["_spdisplays_resolution"].as_str().or(display["spdisplays_resolution"].as_str());
            let main = display["spdisplays_main"] == "spdisplays_yes";
            found.push((main, mode.map_or(name.to_string(), |mode| format!("{name} {mode}"))));
        }
    }
    found.sort_by_key(|(main, _)| !main);
    found.into_iter().map(|(_, display)| display).collect()
}

/// The machine in one line for the summary, saying plainly whether it is
/// the reference machine (spec #19, Further Notes).
pub fn machine_line(machine: &Value) -> String {
    let s = |k: &str| machine[k].as_str().unwrap_or("?").to_string();
    let mut hardware = vec![s("chip")];
    if let Some(cores) = machine["cores"].as_u64() {
        hardware.push(format!("{cores} cores"));
    }
    if let Some(gb) = machine["memory_gb"].as_f64() {
        hardware.push(format!("{} GB", gb.round()));
    }
    let mut line = format!("{} ({}), macOS {}", s("model"), hardware.join(", "), s("macos"));
    for display in machine["displays"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        line.push_str(&format!(", {display}"));
    }
    line.push_str(if machine["reference"] == true {
        "; the reference machine"
    } else {
        "; a dev machine, NOT the reference machine"
    });
    line
}

/// The commit `dir`'s git repository is at, if it is one.
pub fn git_commit(dir: &Path) -> Option<String> {
    let output = Command::new("git").arg("-C").arg(dir).args(["rev-parse", "HEAD"]).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Whether `dir`'s git work tree has uncommitted changes.
pub fn git_dirty(dir: &Path) -> Option<bool> {
    let output = Command::new("git").arg("-C").arg(dir).args(["status", "--porcelain"]).output().ok()?;
    output.status.success().then(|| !output.stdout.is_empty())
}

/// A file-name-safe UTC timestamp for `unix_seconds`, like
/// `2026-10-09T21-30-05Z`.
pub fn utc_label(unix_seconds: u64) -> String {
    let days = (unix_seconds / 86_400) as i64;
    let secs = unix_seconds % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}-{:02}-{:02}Z", secs / 3600, secs % 3600 / 60, secs % 60)
}
