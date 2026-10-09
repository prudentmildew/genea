//! Dead keys: the IME bridge end to end (ADR 0004).
//!
//! On the Norwegian layout it types dead-key compositions into a new line
//! of [`super::FILE`] through synthesized key events, the same path a real
//! keyboard takes through AppKit's text input. After each dead key the
//! composition must show (the preedit), and after the next key the composed
//! character must be in the buffer at the caret. Any wrong composition fails
//! the run. If Norwegian isn't the current layout, the scenario selects it
//! and puts the user's layout back afterwards.

use std::time::Duration;

use serde_json::json;

use super::{Context, Scenario, require_visible, sleep};
use crate::{
    budgets::{self, Check},
    journal::Window,
    keys::{self, Key, norwegian},
    stats::summary_json,
    sys::now_ns,
};

pub struct DeadKeys;

/// Dead key, then key, then what they compose.
const COMPOSITIONS: [(Key, Key, &str); 7] = [
    (norwegian::ACUTE, Key { code: 14, flags: 0 }, "é"),
    (norwegian::DIAERESIS, Key { code: 32, flags: 0 }, "ü"),
    (norwegian::DIAERESIS, Key { code: 0, flags: 0 }, "ä"),
    (norwegian::ACUTE, keys::SPACE, "´"),
    (norwegian::GRAVE, keys::SPACE, "`"),
    (norwegian::CIRCUMFLEX, keys::SPACE, "^"),
    (norwegian::TILDE, keys::SPACE, "~"),
];

const SETTLE: Duration = Duration::from_millis(80);

impl Scenario for DeadKeys {
    fn name(&self) -> &'static str {
        "dead-keys"
    }

    fn run(&self, cx: &mut Context) -> Result<Vec<Check>, String> {
        let previous = keys::current_layout();
        if previous.as_deref() != Some(norwegian::LAYOUT) {
            if let Err(error) = keys::select_layout(norwegian::LAYOUT) {
                return Ok(vec![budgets::DEAD_KEYS.skipped(&error), budgets::DEAD_KEYS_STALL.skipped(&error)]);
            }
            sleep(Duration::from_millis(300));
        }
        let result = compose(cx);
        if let Some(previous) = previous.filter(|p| p != norwegian::LAYOUT)
            && let Err(error) = keys::select_layout(&previous)
        {
            eprintln!("genea-bench: couldn't restore the keyboard layout {previous}: {error}");
        }
        result
    }
}

fn compose(cx: &mut Context) -> Result<Vec<Check>, String> {
    let mut genea = cx.launch(true)?;
    genea.wait_content()?;
    require_visible(&mut genea)?;
    genea.place_caret(300, 0)?;
    genea.key(keys::RETURN)?;
    sleep(SETTLE);

    let from = now_ns();
    let mut results = Vec::new();
    for (dead, key, expected) in COMPOSITIONS {
        let (before, column, _) = genea.caret_line()?;
        genea.key(dead)?;
        sleep(SETTLE);
        let (_, _, composing) = genea.caret_line()?;
        genea.key(key)?;
        sleep(SETTLE);
        let (after, _, still_composing) = genea.caret_line()?;
        let mut wanted: Vec<char> = before.chars().collect();
        let at = column.min(wanted.len());
        wanted.splice(at..at, expected.chars());
        let wanted: String = wanted.into_iter().collect();
        let ok = composing && !still_composing && after == wanted;
        results.push(json!({
            "expected": expected,
            "showed_composition": composing,
            "ok": ok,
            "line_after": after,
        }));
    }
    let window = Window { from, to: now_ns() };
    let journal = genea.journal()?;
    genea.quit();

    let wrong = results.iter().filter(|r| r["ok"] != true).count();
    let latency: Vec<f64> = journal.keystrokes(window).iter().map(|k| k.genea_work_ms()).collect();
    let stalls = journal.stalls(window);
    cx.record(
        "result",
        json!({
            "layout": norwegian::LAYOUT,
            "compositions": results,
            "wrong": wrong,
            "genea_work_ms": summary_json(&latency),
            "stalls": stalls.to_json(window.from),
        }),
    );
    Ok(vec![
        budgets::DEAD_KEYS.check(Some(wrong as f64)).with_note(format!("{} compositions", COMPOSITIONS.len())),
        budgets::DEAD_KEYS_STALL.check(Some(stalls.max_ms)),
    ])
}
