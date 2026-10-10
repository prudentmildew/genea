//! External changes (ticket #63): a file changed on disk outside Genea,
//! and a new one, shown in the open editor and the tree.
//!
//! On each reference workspace, Genea opens with its file and is left until
//! it has read the project's files. Then, `external_runs` times, the
//! harness writes the open file with a new first line (the editor has no
//! unsaved edits, so it reloads) and creates a file at the project root
//! (the tree shows the root's entries), both at once, as a git checkout or
//! an agent would. Each change is timed from the writes to the first frame
//! showing both, as Genea's work (the wait for the display-link tick left
//! out): the watcher's latency counts. Then it puts the file back, deletes
//! the new one and waits for Genea to show that too.
//!
//! The writes are inside the workspace, so a run that is killed half-way
//! can leave them behind: `git status` in the workspace shows them.

use std::{fs, time::Duration};

use serde_json::json;

use super::{Context, Reference, Scenario, large_missing, quiesce, reaction_ms, require_visible, sleep};
use crate::{
    budgets::{self, Check},
    journal::Window,
    stats::{Summary, summary_json},
    sys::now_ns,
};

pub struct ExternalChange;

/// The file created at the project root.
const NEW_FILE: &str = "genea-bench-external.md";
const CHANGE_TIMEOUT: Duration = Duration::from_secs(10);

impl Scenario for ExternalChange {
    fn name(&self) -> &'static str {
        "external-change"
    }

    fn run(&self, cx: &mut Context) -> Result<Vec<Check>, String> {
        let mut checks = Vec::new();
        for reference in cx.references() {
            let original = reference.root.join(reference.file);
            let text = fs::read_to_string(&original).map_err(|e| format!("{}: {e}", original.display()))?;
            let measured = measure(cx, &reference, &text);
            // Whatever happened, leave the workspace as it was.
            let restored = fs::write(&original, &text);
            let _ = fs::remove_file(reference.root.join(NEW_FILE));
            restored.map_err(|e| format!("couldn't restore {}: {e}", original.display()))?;
            let work = measured?;
            let p95 = Summary::of(&work).map(|s| s.p95);
            checks.push(
                reference
                    .budget(budgets::EXTERNAL_TYPICAL, budgets::EXTERNAL_LARGE)
                    .check(p95)
                    .with_note(format!("{} changes", work.len())),
            );
        }
        if cx.large.is_none() {
            checks.push(large_missing(budgets::EXTERNAL_LARGE));
        }
        Ok(checks)
    }
}

/// Genea's work per change, until both the editor and the tree show it.
fn measure(cx: &mut Context, reference: &Reference, original: &str) -> Result<Vec<f64>, String> {
    let file = reference.root.join(reference.file);
    let new_file = reference.root.join(NEW_FILE);
    let mut genea = cx.launch_on(reference)?;
    genea.wait_content()?;
    require_visible(&mut genea)?;
    let settled = quiesce(&genea, Duration::from_secs(120))?;

    let from = now_ns();
    let (mut work, mut present, mut editor_ms, mut tree_ms) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut journal_runs = Vec::new();
    for run in 0..cx.options.external_runs {
        let marker = format!("// genea-bench external change {run}");
        genea.expect("editor", &format!("editor-has {} {marker}", reference.file))?;
        genea.expect("tree", &format!("tree-has {NEW_FILE}"))?;
        let at = now_ns();
        fs::write(&file, format!("{marker}\n{original}")).map_err(|e| format!("{}: {e}", file.display()))?;
        fs::write(&new_file, format!("{marker}\n")).map_err(|e| format!("{}: {e}", new_file.display()))?;
        let editor = genea.await_met("editor", CHANGE_TIMEOUT).map_err(|e| format!("the editor didn't reload: {e}"))?;
        let tree = genea.await_met("tree", CHANGE_TIMEOUT).map_err(|e| format!("the tree didn't show it: {e}"))?;
        journal_runs.push((at, editor, tree));
        sleep(Duration::from_millis(300));

        // Back as it was, before the next one.
        genea.expect("editor", &format!("editor-lacks {} {marker}", reference.file))?;
        genea.expect("tree", &format!("tree-lacks {NEW_FILE}"))?;
        fs::write(&file, original).map_err(|e| format!("{}: {e}", file.display()))?;
        fs::remove_file(&new_file).map_err(|e| format!("{}: {e}", new_file.display()))?;
        genea.await_met("editor", CHANGE_TIMEOUT).map_err(|e| format!("the editor didn't reload: {e}"))?;
        genea.await_met("tree", CHANGE_TIMEOUT).map_err(|e| format!("the tree didn't drop it: {e}"))?;
        sleep(Duration::from_millis(300));
    }
    let window = Window { from, to: now_ns() };
    let journal = genea.journal()?;
    genea.quit();

    for (at, editor, tree) in journal_runs {
        let none = || "the journal has no frame showing the change".to_string();
        let (editor_work, editor_present) = reaction_ms(&journal, at, editor).ok_or_else(none)?;
        let (tree_work, tree_present) = reaction_ms(&journal, at, tree).ok_or_else(none)?;
        work.push(editor_work.max(tree_work));
        present.push(editor_present.max(tree_present));
        editor_ms.push(editor_work);
        tree_ms.push(tree_work);
    }
    cx.record(
        "result",
        json!({
            "workspace": reference.name(),
            "changes": work.len(),
            "settled_after_ms": settled.as_millis() as u64,
            "both_shown_ms": summary_json(&work),
            "both_shown_to_present_ms": summary_json(&present),
            "editor_ms": summary_json(&editor_ms),
            "tree_ms": summary_json(&tree_ms),
            "stalls": journal.stalls(window).to_json(window.from),
        }),
    );
    Ok(work)
}
