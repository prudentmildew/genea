//! The language server, end to end (ticket #63), on Typical with its real
//! tsgo.
//!
//! The end-to-end targets include tsgo, which Genea doesn't control, so
//! they are reported and never fail the run. One check does fail it: a
//! language server must never block typing or rendering.
//!
//! The scenario writes a file with one type error into `packages/web/src`
//! (and deletes it afterwards), then:
//!
//! - **First diagnostics, cold**: launches Genea on that file `lsp_runs`
//!   times, each with tsgo starting cold, and times process start to the
//!   first frame showing the error. With cold runs on and passwordless
//!   `sudo`, each launch follows a `purge`, so tsgo and the workspace come
//!   from disk.
//! - **Diagnostics after an edit**: in the last of those Geneas, types a
//!   letter that makes a second error and deletes it again, `lsp_edits`
//!   times, each timed from the key to the first frame showing the problems
//!   changed.
//! - **Never blocks**: the longest main-thread stall and the longest
//!   keystroke (Genea's work) in that Genea, from just after content
//!   visible (after a warm-up key: Genea's first key stalls, LS or not)
//!   through the edits, while tsgo loads the project and checks every edit. Both
//!   within 16 ms.
//! - **Completions and go-to-definition**: Genea has neither yet (#43,
//!   #44), so they are timed against the same tsgo binary directly
//!   (`crate::lsp`): request written to response read, `lsp_requests`
//!   times each. That is tsgo's share; time them through Genea once it has
//!   them.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use serde_json::json;

use super::{
    Context, Scenario,
    memory::language_servers,
    reaction_ms, sleep,
    start::{can_purge, purge},
};
use crate::{
    budgets::{self, Check},
    genea::{Genea, Met},
    journal::{BEFORE_WAITING, Journal, Window},
    keys,
    lsp::{self, Client},
    stats::{Summary, summary_json},
    sys::{self, now_ns},
};

pub struct LanguageServer;

/// The file the scenario writes, relative to Typical's root.
const FILE: &str = "packages/web/src/genea-bench-lsp.ts";
/// One type error (TS2322, line 11). Typing `x` after `benchTotal` on the
/// last line makes a second (TS2552); deleting it takes it away again.
const TEXT: &str = r#"// Written by genea-bench for the language-server scenario (ticket #63); deleted after it.
export interface BenchOrder {
  id: number;
  total: number;
}

export function benchTotal(orders: readonly BenchOrder[]): number {
  return orders.reduce((sum, order) => sum + order.total, 0);
}

const missing: number = "not a number";
export const shown: number = benchTotal([]) + missing;
"#;
const DIAGNOSTICS_TIMEOUT: Duration = Duration::from_secs(30);
const EDIT_TIMEOUT: Duration = Duration::from_secs(10);

impl Scenario for LanguageServer {
    fn name(&self) -> &'static str {
        "language-server"
    }

    fn run(&self, cx: &mut Context) -> Result<Vec<Check>, String> {
        let path = cx.workspace().join(FILE);
        fs::write(&path, TEXT).map_err(|e| format!("{}: {e}", path.display()))?;
        let checks = measure(cx, &path);
        let _ = fs::remove_file(&path);
        checks
    }
}

/// Where `needle` starts on `line` of [`TEXT`] (0-based), plus `offset`.
fn position(line: usize, needle: &str, offset: usize) -> (usize, usize) {
    let text = TEXT.lines().nth(line).unwrap_or_default();
    (line, text.find(needle).unwrap_or(0) + offset)
}

fn measure(cx: &mut Context, path: &Path) -> Result<Vec<Check>, String> {
    let cold = cx.options.cold && can_purge();
    let runs = cx.options.lsp_runs.max(1);
    let mut first = Vec::new();
    let mut last = None;
    for run in 0..runs {
        if cold {
            purge()?;
        }
        let mut genea = Genea::launch(&cx.genea, cx.workspace(), FILE, true)?;
        genea.expect("diagnostics", &format!("problems {FILE} 1"))?;
        let warm_up = if run + 1 == runs {
            // The first key Genea gets stalls its main thread for some
            // 30 ms, language server or not (`typing` sees it too): press
            // one that edits nothing while tsgo loads, and start watching
            // for stalls after it.
            genea.wait_content()?;
            let at = now_ns();
            genea.key(keys::RIGHT)?;
            Some(at)
        } else {
            None
        };
        let met = genea.await_met("diagnostics", DIAGNOSTICS_TIMEOUT).map_err(|e| format!("no diagnostics: {e}"))?;
        eprint!(".");
        if let Some(warm_up) = warm_up {
            // The edits go on in this one. Its journal is read at the end:
            // dumping it is work on the main thread, a stall of its own.
            last = Some((genea, met, warm_up));
            break;
        }
        let journal = genea.journal()?;
        genea.quit();
        first.push(first_diagnostics(cx, run, cold, &journal, met)?);
    }
    eprintln!();
    let (mut genea, met, warm_up) = last.ok_or("no Genea ran")?;
    let tsgo = language_servers(&mut genea)?.first().and_then(|&pid| sys::executable(pid));
    let (journal, after_edit, blocking, blocking_note) = edit(cx, &mut genea, warm_up)?;
    genea.quit();
    first.push(first_diagnostics(cx, runs - 1, cold, &journal, met)?);

    let tsgo = tsgo.ok_or("Genea ran no tsgo")?;
    let (completion, definition) = requests(cx, &tsgo, path)?;

    let p95 = |samples: &[f64]| Summary::of(samples).map(|s| s.p95);
    cx.record(
        "result",
        json!({
            "first_diagnostics_ms": summary_json(&first),
            "first_diagnostics_after_purge": cold,
            "diagnostics_after_edit_ms": summary_json(&after_edit),
            "completion_ms": summary_json(&completion),
            "definition_ms": summary_json(&definition),
            "tsgo": tsgo,
        }),
    );
    let first_note = if cold { "after purge" } else { "tsgo started fresh; no purge (cold runs off or no sudo)" };
    Ok(vec![
        budgets::LSP_FIRST_DIAGNOSTICS.check(p95(&first)).with_note(format!("{} launches, {first_note}", first.len())),
        budgets::LSP_DIAGNOSTICS_AFTER_EDIT.check(p95(&after_edit)).with_note(format!("{} edits", after_edit.len())),
        budgets::LSP_COMPLETION
            .check(p95(&completion))
            .with_note(format!("{} requests to tsgo directly: Genea has no completions yet (#43)", completion.len())),
        budgets::LSP_DEFINITION
            .check(p95(&definition))
            .with_note(format!("{} requests to tsgo directly: Genea has no go-to-definition yet (#44)", definition.len())),
        budgets::LSP_BLOCKING.check(Some(blocking)).with_note(blocking_note),
    ])
}

/// Process start to the first frame showing the error, recorded as a run.
fn first_diagnostics(cx: &mut Context, run: usize, cold: bool, journal: &Journal, met: Met) -> Result<f64, String> {
    let (_, to_present) =
        reaction_ms(journal, journal.process_start, met).ok_or("the journal has no frame showing the error")?;
    cx.record("run", json!({ "run": run, "first_diagnostics_ms": crate::stats::round(to_present), "after_purge": cold }));
    Ok(to_present)
}

/// Types a second error and deletes it, `lsp_edits` times in all. Returns
/// the journal, each edit's key-to-problems-shown time, the longest stall
/// or keystroke from the warm-up key (sent at `warm_up`) on, and what that
/// was.
fn edit(cx: &mut Context, genea: &mut Genea, warm_up: u64) -> Result<(Journal, Vec<f64>, f64, String), String> {
    let (line, column) = position(11, "benchTotal", "benchTotal".len());
    genea.place_caret(line, column)?;
    sleep(Duration::from_millis(500));
    let mut edits = Vec::new();
    for i in 0..cx.options.lsp_edits {
        let (key, problems) = if i % 2 == 0 { (keys::letter('x').expect("x has a key"), 2) } else { (keys::BACKSPACE, 1) };
        genea.expect("edit", &format!("problems {FILE} {problems}"))?;
        let at = now_ns();
        genea.key(key)?;
        let met = genea.await_met("edit", EDIT_TIMEOUT).map_err(|e| format!("no diagnostics after an edit: {e}"))?;
        edits.push((at, met));
        sleep(Duration::from_millis(300));
    }
    // Leave the buffer as the file is, so quitting asks nothing.
    if cx.options.lsp_edits % 2 == 1 {
        genea.key(keys::BACKSPACE)?;
        sleep(Duration::from_millis(300));
    }
    let to = now_ns();
    let journal = genea.journal()?;
    let mut after_edit = Vec::new();
    for (at, met) in edits {
        let key = journal.key_after(at).ok_or("the journal has no key the harness pressed")?;
        let (_, to_present) = reaction_ms(&journal, key, met).ok_or("the journal has no frame showing the problems")?;
        after_edit.push(to_present);
    }
    // From the end of the busy span that took the warm-up key, a moment
    // after content visible, while tsgo loads the project.
    let key = journal.key_after(warm_up).ok_or("the journal has no warm-up key")?;
    let from = journal
        .activities
        .iter()
        .find(|&&(t, activity)| t >= key && activity == BEFORE_WAITING)
        .map_or(key, |&(t, _)| t + 1);
    let window = Window { from, to };
    let stalls = journal.stalls(window);
    let keys: Vec<f64> = journal.keystrokes(window).iter().map(|k| k.genea_work_ms()).collect();
    let longest_key = keys.iter().copied().fold(0.0, f64::max);
    cx.record(
        "result",
        json!({
            "while_tsgo_works": {
                "stalls": stalls.to_json(window.from),
                "keystroke_genea_work_ms": summary_json(&keys),
            },
        }),
    );
    let note = format!(
        "longest stall {} ms, longest of {} keystrokes {} ms, from just after content visible through the edits",
        crate::stats::round(stalls.max_ms),
        keys.len(),
        crate::stats::round(longest_key)
    );
    Ok((journal, after_edit, stalls.max_ms.max(longest_key), note))
}

/// Times completions after `order.` and go-to-definition on `benchTotal`
/// against `tsgo` directly.
fn requests(cx: &mut Context, tsgo: &PathBuf, path: &Path) -> Result<(Vec<f64>, Vec<f64>), String> {
    let mut client = Client::start(tsgo, cx.workspace())?;
    client.open(path, TEXT)?;
    let document = json!({ "uri": lsp::file_uri(path) });
    let at = |(line, character): (usize, usize)| json!({ "textDocument": document, "position": { "line": line, "character": character } });
    let completion_at = at(position(7, "order.total", "order.".len()));
    let definition_at = at(position(11, "benchTotal", 2));
    // The first request waits for tsgo to load the project: not timed.
    client.request("textDocument/definition", definition_at.clone())?;
    let (mut completion, mut definition) = (Vec::new(), Vec::new());
    // An edit before each request, as a user types before asking, so tsgo
    // can't answer from what it worked out for the last one. A line break
    // at the end moves nothing the requests point at.
    let mut version = 1;
    let mut edit = |client: &mut Client| {
        version += 1;
        let text = if version % 2 == 0 { format!("{TEXT}\n") } else { TEXT.to_string() };
        client.change(path, version, &text)
    };
    for _ in 0..cx.options.lsp_requests {
        edit(&mut client)?;
        let (items, took) = client.request("textDocument/completion", completion_at.clone())?;
        let count = items["items"].as_array().or(items.as_array()).map_or(0, Vec::len);
        if count == 0 {
            return Err("tsgo offered no completions after `order.`".into());
        }
        completion.push(took.as_secs_f64() * 1000.0);
        edit(&mut client)?;
        let (target, took) = client.request("textDocument/definition", definition_at.clone())?;
        if target.is_null() || target.as_array().is_some_and(Vec::is_empty) {
            return Err("tsgo found no definition of benchTotal".into());
        }
        definition.push(took.as_secs_f64() * 1000.0);
        sleep(Duration::from_millis(50));
    }
    client.stop();
    Ok((completion, definition))
}
