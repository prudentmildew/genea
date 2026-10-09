//! Typing: keystroke to frame and main-thread stalls.
//!
//! With the caret in the middle of [`super::FILE`], it presses keys at 25 a
//! second (one every 40 ms) through Genea's real input path: every 23rd is
//! Return, every 11th Backspace, the rest letters and spaces of a sentence.
//! For each key the journal gives Genea's work: from the key's arrival to
//! the view state reaching Slint, plus the frame that shows it (the wait
//! for the display-link tick in between is the display's, not Genea's).
//!
//! `typing-silent-lsp` types the same way with a language server that never
//! answers (ticket #42: typing and rendering never wait on a server). It
//! runs in `bench/workspaces/out/silent-lsp` (made on each run): the same
//! file, in a project whose tsgo is `genea-fake-lsp` scripted to stay
//! silent. The Typical workspace has TypeScript 7 installed, so `typing`
//! itself runs with the real tsgo.

use std::{fs, path::Path, time::Duration};

use serde_json::json;

use super::{Context, FILE, Scenario, require_visible, sleep};
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
        let genea = cx.launch(true)?;
        measure(cx, genea, "")
    }
}

/// Typing while the project's language server never answers.
pub struct TypingSilentServer;

impl Scenario for TypingSilentServer {
    fn name(&self) -> &'static str {
        "typing-silent-lsp"
    }

    fn run(&self, cx: &mut Context) -> Result<Vec<Check>, String> {
        let folder = cx.workspace().join("../silent-lsp");
        silent_project(&folder, &cx.workspace().join(FILE), &cx.fake_lsp)
            .map_err(|e| format!("couldn't set up {}: {e}", folder.display()))?;
        let genea = crate::genea::Genea::launch(&cx.genea, &folder, FILE, true)?;
        measure(cx, genea, "; a language server that never answers")
    }
}

/// A project with [`FILE`] and TypeScript 7 "installed", where tsgo is the
/// fake LSP server, scripted to never answer.
fn silent_project(folder: &Path, file: &Path, fake_lsp: &Path) -> std::io::Result<()> {
    let tsgo = folder.join("node_modules/@typescript/typescript-darwin-arm64/lib/tsc");
    let typescript = folder.join("node_modules/typescript");
    for dir in [tsgo.parent().expect("a folder"), typescript.as_path(), folder.join(FILE).parent().expect("a folder")] {
        fs::create_dir_all(dir)?;
    }
    fs::write(folder.join("package.json"), r#"{ "name": "silent-lsp", "devDependencies": { "typescript": "^7.0.2" } }"#)?;
    fs::write(typescript.join("package.json"), r#"{ "name": "typescript", "version": "7.0.2" }"#)?;
    fs::copy(fake_lsp, &tsgo)?;
    fs::write(tsgo.with_file_name("tsc.json"), r#"{ "silent": true }"#)?;
    fs::copy(file, folder.join(FILE))?;
    Ok(())
}

/// Types into a launched Genea and checks the typing budgets; `note` says
/// what was special about the run.
fn measure(cx: &mut Context, mut genea: crate::genea::Genea, note: &str) -> Result<Vec<Check>, String> {
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
    Ok(vec![budgets::KEYSTROKE.check(p95).with_note(format!("{} keys{note}", keystrokes.len())), stall])
}
