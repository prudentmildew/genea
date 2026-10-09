//! Opening files (ticket #27): a 1 MB file, and a 100 MB one, which is a
//! large file (over 5 MB: no highlighting, read first screen first).
//!
//! Each run launches Genea on a folder of generated TypeScript
//! (`bench/workspaces/out/open-files`, made on first use) with a small file
//! open, waits for it to show, then asks Genea to open the file under test
//! over the control channel. The open lasts from that request to the end of
//! the first frame showing the file's lines. The harness then waits until
//! the whole file is in (`editor` says it is no longer loading) and 300 ms
//! more, and takes the longest main-thread stall from the request to then.
//! A fresh Genea per run, so every open is a first open; the file itself is
//! in the file cache after the first run.

use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde_json::json;

use super::{Context, Scenario, require_visible, sleep};
use crate::{
    budgets::{self, Check},
    genea::Genea,
    journal::Window,
    stats::{Summary, round, summary_json},
    sys::now_ns,
};

const MB: u64 = 1024 * 1024;

/// The file Genea starts with: small, so the start doesn't get in the way.
const START_FILE: &str = "start.ts";

/// Opening a 1 MB file.
pub struct OpenOneMb;

/// Opening a 100 MB file.
pub struct OpenHundredMb;

impl Scenario for OpenOneMb {
    fn name(&self) -> &'static str {
        "open-1mb"
    }

    fn run(&self, cx: &mut Context) -> Result<Vec<Check>, String> {
        let runs = measure(cx, "onemb.ts", MB, cx.options.open_runs)?;
        if let Some(run) = runs.iter().find(|r| r.large) {
            return Err(format!("the 1 MB file opened as a large file ({} lines)", run.line_count));
        }
        let open = Summary::of(&runs.iter().map(|r| r.open_ms).collect::<Vec<_>>()).map(|s| s.p95);
        Ok(vec![budgets::OPEN_1MB.check(open).with_note(format!("{} runs", runs.len()))])
    }
}

impl Scenario for OpenHundredMb {
    fn name(&self) -> &'static str {
        "open-100mb"
    }

    fn run(&self, cx: &mut Context) -> Result<Vec<Check>, String> {
        let runs = measure(cx, "huge.ts", 100 * MB, cx.options.open_large_runs)?;
        if let Some(run) = runs.iter().find(|r| !r.large) {
            return Err(format!("the 100 MB file didn't open as a large file ({} lines)", run.line_count));
        }
        let open = Summary::of(&runs.iter().map(|r| r.open_ms).collect::<Vec<_>>()).map(|s| s.p95);
        let stall = runs.iter().map(|r| r.stall_ms).fold(0.0, f64::max);
        let loaded = Summary::of(&runs.iter().map(|r| r.loaded_ms).collect::<Vec<_>>()).map_or(0.0, |s| s.p95);
        Ok(vec![
            budgets::OPEN_100MB.check(open).with_note(format!(
                "{} runs; whole file in after {} ms (p95)",
                runs.len(),
                round(loaded)
            )),
            budgets::OPEN_100MB_STALL.check(Some(stall)).with_note("from the request until the whole file is in"),
        ])
    }
}

/// One open, as measured.
struct Run {
    /// Request to the first frame showing the file.
    open_ms: f64,
    /// Request to the harness seeing the whole file in (polled every 20 ms).
    loaded_ms: f64,
    /// The longest main-thread stall from the request until 300 ms after
    /// the whole file was in.
    stall_ms: f64,
    line_count: u64,
    large: bool,
}

/// `runs` opens of `file`, generated at `size` bytes, each in a fresh Genea.
fn measure(cx: &mut Context, file: &str, size: u64, runs: usize) -> Result<Vec<Run>, String> {
    let folder = fixtures(cx, file, size)?;
    let mut measured = Vec::new();
    for run in 0..runs {
        let mut genea = Genea::launch(&cx.genea, &folder, START_FILE, true)?;
        genea.wait_content()?;
        require_visible(&mut genea)?;
        sleep(Duration::from_millis(500));

        let from = now_ns();
        genea.open(file)?;
        let editor = wait_until_loaded(&mut genea)?;
        let loaded = now_ns();
        sleep(Duration::from_millis(300));
        let window = Window { from, to: now_ns() };
        let journal = genea.journal()?;
        genea.quit();

        let open = journal.open(0).ok_or("the journal has no frame showing the opened file")?;
        let stalls = journal.stalls(window);
        let line_count = editor["line_count"].as_u64().unwrap_or(0);
        let large = editor["large_file"].as_bool().unwrap_or(false);
        let result = Run {
            open_ms: open.ms(),
            loaded_ms: crate::journal::ms(loaded.saturating_sub(open.requested)),
            stall_ms: stalls.max_ms,
            line_count,
            large,
        };
        cx.record(
            "run",
            json!({
                "run": run,
                "file": file,
                "bytes": size,
                "open_ms": round(result.open_ms),
                "loaded_ms": round(result.loaded_ms),
                "line_count": line_count,
                "large_file": large,
                "stalls": stalls.to_json(window.from),
            }),
        );
        eprint!(".");
        measured.push(result);
    }
    eprintln!();
    let column = |f: fn(&Run) -> f64| measured.iter().map(f).collect::<Vec<_>>();
    cx.record(
        "result",
        json!({
            "file": file,
            "bytes": size,
            "runs": runs,
            "open_ms": summary_json(&column(|r| r.open_ms)),
            "loaded_ms": summary_json(&column(|r| r.loaded_ms)),
            "stall_ms": summary_json(&column(|r| r.stall_ms)),
        }),
    );
    Ok(measured)
}

/// Polls `editor` until the opened file is all in; returns its answer.
fn wait_until_loaded(genea: &mut Genea) -> Result<serde_json::Value, String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let editor = genea.editor()?;
        if editor["loading"] == false {
            return Ok(editor);
        }
        if Instant::now() > deadline {
            return Err("the opened file was still loading after 30 s".into());
        }
        sleep(Duration::from_millis(20));
    }
}

/// The folder the scenarios open files from, next to the Typical
/// workspace, with [`START_FILE`] and `file` at exactly `size` bytes.
fn fixtures(cx: &Context, file: &str, size: u64) -> Result<PathBuf, String> {
    let out = cx.workspace().parent().ok_or("the workspace has no parent folder")?;
    let folder = out.join("open-files");
    fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let start = folder.join(START_FILE);
    if !start.exists() {
        fs::write(&start, typescript_block(0)).map_err(|e| format!("{}: {e}", start.display()))?;
    }
    let path = folder.join(file);
    if fs::metadata(&path).map(|m| m.len()).ok() != Some(size) {
        generate(&path, size).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(folder)
}

/// Writes TypeScript of exactly `size` bytes: numbered blocks of
/// declarations, padded with a comment.
fn generate(path: &Path, size: u64) -> std::io::Result<()> {
    let mut out = BufWriter::new(File::create(path)?);
    let mut written = 0u64;
    for n in 0.. {
        let block = typescript_block(n);
        if written + block.len() as u64 + 3 > size {
            break;
        }
        out.write_all(block.as_bytes())?;
        written += block.len() as u64;
    }
    let pad = (size - written) as usize;
    out.write_all(format!("//{}\n", "-".repeat(pad - 3)).as_bytes())?;
    out.flush()
}

fn typescript_block(n: u64) -> String {
    format!(
        "export interface Record{n} {{\n  id: number;\n  name: string;\n  tags: readonly string[];\n}}\n\n\
         /** Describes record {n} for the activity log. */\n\
         export function describe{n}(record: Record{n}): string {{\n\
         \x20 const label = `${{record.name}} (#${{record.id}})`;\n\
         \x20 return record.tags.length > 0 ? `${{label}}: ${{record.tags.join(\", \")}}` : label;\n}}\n\n"
    )
}
