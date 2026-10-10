//! Project search (ticket #63): first results, and complete.
//!
//! On each reference workspace, Genea opens with its file and is left until
//! it has read the project's files. Then the workspace's query is searched
//! for again and again (literal, any case) from the Search view, clearing
//! the results in between. Each search is timed from Genea dispatching it
//! to the first frame showing a match, and to the first frame after it
//! finished, as Genea's work (the wait for the display-link tick left out).

use std::time::Duration;

use serde_json::json;

use super::{Context, Reference, Scenario, large_missing, quiesce, reaction_ms, require_visible, sleep};
use crate::{
    budgets::{self, Check},
    journal::Window,
    stats::{Summary, summary_json},
    sys::now_ns,
};

pub struct ProjectSearch;

const SEARCH_TIMEOUT: Duration = Duration::from_secs(30);

impl Scenario for ProjectSearch {
    fn name(&self) -> &'static str {
        "search"
    }

    fn run(&self, cx: &mut Context) -> Result<Vec<Check>, String> {
        let mut checks = Vec::new();
        for reference in cx.references() {
            let (first, done) = measure(cx, &reference)?;
            let p95 = |samples: &[f64]| Summary::of(samples).map(|s| s.p95);
            let note = format!("{} searches for {:?}", first.len(), reference.search);
            checks.push(
                reference
                    .budget(budgets::SEARCH_FIRST_TYPICAL, budgets::SEARCH_FIRST_LARGE)
                    .check(p95(&first))
                    .with_note(note.clone()),
            );
            checks.push(
                reference.budget(budgets::SEARCH_DONE_TYPICAL, budgets::SEARCH_DONE_LARGE).check(p95(&done)).with_note(note),
            );
        }
        if cx.large.is_none() {
            checks.push(large_missing(budgets::SEARCH_FIRST_LARGE));
            checks.push(large_missing(budgets::SEARCH_DONE_LARGE));
        }
        Ok(checks)
    }
}

/// `search_runs` searches; Genea's work to the first results and to the
/// end, per search.
fn measure(cx: &mut Context, reference: &Reference) -> Result<(Vec<f64>, Vec<f64>), String> {
    let mut genea = cx.launch_on(reference)?;
    genea.wait_content()?;
    require_visible(&mut genea)?;
    let settled = quiesce(&genea, Duration::from_secs(120))?;

    let query = reference.search;
    let from = now_ns();
    let mut sent = Vec::new();
    for _ in 0..cx.options.search_runs {
        genea.search("")?;
        sleep(Duration::from_millis(300));
        genea.expect("first", &format!("search-results {query}"))?;
        genea.expect("done", &format!("search-done {query}"))?;
        let at = genea.search(query)?;
        let first = genea.await_met("first", SEARCH_TIMEOUT)?;
        let done = genea.await_met("done", SEARCH_TIMEOUT)?;
        sent.push((at, first, done));
        sleep(Duration::from_millis(300));
    }
    let window = Window { from, to: now_ns() };
    let journal = genea.journal()?;
    genea.quit();

    let (mut first, mut done, mut first_present, mut done_present) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (at, f, d) in sent {
        let none = || "the journal has no frame showing the search results".to_string();
        let (work, present) = reaction_ms(&journal, at, f).ok_or_else(none)?;
        first.push(work);
        first_present.push(present);
        let (work, present) = reaction_ms(&journal, at, d).ok_or_else(none)?;
        done.push(work);
        done_present.push(present);
    }
    cx.record(
        "result",
        json!({
            "workspace": reference.name(),
            "query": query,
            "searches": first.len(),
            "settled_after_ms": settled.as_millis() as u64,
            "first_results_ms": summary_json(&first),
            "complete_ms": summary_json(&done),
            "first_results_to_present_ms": summary_json(&first_present),
            "complete_to_present_ms": summary_json(&done_present),
            "stalls": journal.stalls(window).to_json(window.from),
        }),
    );
    Ok((first, done))
}
