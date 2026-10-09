//! Typing: keystroke to frame and main-thread stalls.
//!
//! With the caret in the middle of [`super::FILE`], it presses keys at 25 a
//! second (one every 40 ms) through Genea's real input path: every 23rd is
//! Return, every 11th Backspace, the rest letters and spaces of a sentence.
//! For each key the journal gives Genea's work: from the key's arrival to
//! the view state reaching Slint, plus the frame that shows it (the wait
//! for the display-link tick in between is the display's, not Genea's).

use std::time::Duration;

use serde_json::json;

use super::{Context, Scenario, require_visible, sleep};
use crate::{
    budgets::{self, Check},
    journal::{Window, ms},
    keys::{self, Key},
    stats::{round, summary_json},
    sys::now_ns,
};

pub struct Typing;

const SENTENCE: &str = "let total be the sum of parts and keep going ";
const INTERVAL: Duration = Duration::from_millis(40);

/// The `i`th key of the typing script.
fn key(i: usize) -> Key {
    if i % 23 == 22 {
        keys::RETURN
    } else if i % 11 == 10 {
        keys::BACKSPACE
    } else {
        let c = SENTENCE.chars().nth(i % SENTENCE.chars().count()).unwrap_or(' ');
        keys::letter(c).unwrap_or(keys::SPACE)
    }
}

impl Scenario for Typing {
    fn name(&self) -> &'static str {
        "typing"
    }

    fn run(&self, cx: &mut Context) -> Result<Vec<Check>, String> {
        let mut genea = cx.launch(true)?;
        genea.wait_content()?;
        let info = require_visible(&mut genea)?;
        genea.place_caret(300, 0)?;
        sleep(Duration::from_millis(500));

        let pressed = cx.options.typing_keys;
        let from = now_ns();
        for i in 0..pressed {
            genea.key(key(i))?;
            sleep(INTERVAL);
        }
        sleep(Duration::from_millis(300));
        let window = Window { from, to: now_ns() };
        let journal = genea.journal()?;
        genea.quit();

        let keystrokes = journal.keystrokes(window);
        if keystrokes.len() < pressed * 9 / 10 {
            return Err(format!("only {} of {pressed} keys reached a frame", keystrokes.len()));
        }
        let work: Vec<f64> = keystrokes.iter().map(|k| k.genea_work_ms()).collect();
        let present: Vec<f64> = keystrokes.iter().map(|k| k.input_to_present_ms()).collect();
        let frame: Vec<f64> = keystrokes.iter().map(|k| k.frame.work_ms()).collect();
        let stalls = journal.stalls(window);
        cx.record(
            "result",
            json!({
                "keys_pressed": pressed,
                "keys_shown": keystrokes.len(),
                "display_hz": info.display_hz,
                "genea_work_ms": summary_json(&work),
                "input_to_present_ms": summary_json(&present),
                "frame_work_ms": summary_json(&frame),
                "stalls": stalls.to_json(window.from),
            }),
        );
        let p95 = crate::stats::Summary::of(&work).map(|s| s.p95);
        let mut stall = budgets::TYPING_STALL.check(Some(stalls.max_ms));
        if let Some((start, end)) = stalls.longest {
            // Where it was, to find what caused it.
            let first_key = keystrokes.first().is_some_and(|k| start <= k.key && k.key <= end);
            stall = stall.with_note(if first_key {
                "the longest holds the first key".to_string()
            } else {
                format!("the longest starts {} ms in", round(ms(start.saturating_sub(window.from))))
            });
        }
        Ok(vec![budgets::KEYSTROKE.check(p95).with_note(format!("{} keys", keystrokes.len())), stall])
    }
}
