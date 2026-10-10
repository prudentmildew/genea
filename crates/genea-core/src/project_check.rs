//! The **project check** (ticket #48, spec #19 user stories 74–75): a
//! whole-project type check the user runs on demand, `tsc -b --noEmit`
//! with the project's own TypeScript 7 (the binary tsgo runs from). It
//! runs in the background; its results replace the previous check's in
//! Problems (`ProblemSource::ProjectCheck`) and stay until the next one.
//!
//! This module runs the process and reads what it prints; the project's
//! side (when to run, staleness, open files) is in `project/check.rs`.
//!
//! **TS6310.** TypeScript 7 refuses to check a project that references
//! other projects under `-b --noEmit` ("Referenced project may not disable
//! emit", reported at the referencing `tsconfig.json`) and checks only the
//! projects without references. Nothing in the user's config is wrong and
//! there is no flag that avoids it without writing build output, so the
//! check doesn't list TS6310 as a problem: it names the skipped projects
//! ([`Finished::Checked::skipped`]) for a notice. Live diagnostics are
//! unaffected (tsgo's language server checks references from source).

use std::{
    collections::HashMap,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use genea_host::{ProcessControl, ProcessSpec, SharedHost};
use ropey::Rope;

use crate::{
    lsp::text::{self, Encoding},
    problems::{Problem, Severity, TextPosition},
};

/// What `tsc` is run with. `--pretty false` gives one plain line per
/// diagnostic, `file(line,column): error TS1234: message`, whatever stdout
/// is.
pub(crate) const ARGS: [&str; 4] = ["-b", "--noEmit", "--pretty", "false"];

/// "Referenced project may not disable emit".
const REFERENCES_NEED_EMIT: u32 = 6310;

/// How a check ended.
#[derive(Debug)]
pub(crate) enum Finished {
    /// tsc checked the project: these are its results.
    Checked {
        /// The diagnostics in files, as Problems.
        problems: Vec<Problem>,
        /// The `tsconfig.json`s of projects tsc skipped because they
        /// reference other projects (TS6310), relative to the root.
        skipped: Vec<PathBuf>,
        /// Diagnostics about no file in particular, beside the results.
        general: Vec<String>,
    },
    /// tsc couldn't check the project (it didn't start, crashed, or found
    /// no tsconfig.json): why, as a sentence.
    Failed(String),
}

/// A running check's process, to stop it from the main thread.
#[derive(Clone, Default)]
pub(crate) struct Cancel(Arc<Mutex<CancelState>>);

#[derive(Default)]
struct CancelState {
    cancelled: bool,
    control: Option<Box<dyn ProcessControl>>,
}

impl Cancel {
    /// Kills the process (or keeps it from starting).
    pub(crate) fn cancel(&self) {
        let mut state = self.0.lock().unwrap();
        state.cancelled = true;
        if let Some(control) = &mut state.control {
            let _ = control.kill();
        }
    }
}

/// Runs `spec` (with the project environment applied) to the end and reads
/// its diagnostics, unless `cancel` stops it. Blocks: call it on a
/// background thread.
pub(crate) fn run(host: &SharedHost, spec: &ProcessSpec, root: &Path, cancel: &Cancel) -> Finished {
    // Spawning under the lock: a cancel either comes first or finds the
    // process to kill.
    let (mut out, err) = {
        let mut state = cancel.0.lock().unwrap();
        if state.cancelled {
            return Finished::Failed("it was stopped.".into());
        }
        let mut child = match host.processes().spawn(spec) {
            Ok(child) => child,
            Err(error) => return Finished::Failed(format!("tsc didn't start: {error}.")),
        };
        drop(child.stdin.take());
        state.control = Some(child.control);
        (child.stdout.take(), child.stderr.take())
    };
    // Drain stderr so a chatty tsc can't block on a full pipe.
    let stderr = err.map(|mut err| {
        std::thread::spawn(move || {
            let _ = io::copy(&mut err, &mut io::sink());
        })
    });
    let mut stdout = String::new();
    if let Some(out) = &mut out {
        let _ = out.read_to_string(&mut stdout);
    }
    // Its output is closed, so it is ending; wait outside the lock, which
    // the main thread takes to cancel.
    let control = cancel.0.lock().unwrap().control.take();
    let exit = control.map(|mut control| control.wait());
    if let Some(stderr) = stderr {
        let _ = stderr.join();
    }
    let exit = match exit {
        Some(Ok(exit)) => exit,
        Some(Err(error)) => return Finished::Failed(format!("{error}.")),
        None => return Finished::Failed("it was stopped.".into()),
    };
    let diagnostics = parse(&stdout);
    let (placed, general): (Vec<_>, Vec<_>) = diagnostics.into_iter().partition(|d| d.place.is_some());
    let general: Vec<String> = general.into_iter().map(|d| d.message).collect();
    if placed.is_empty() && !exit.success() {
        return Finished::Failed(match (general.first(), exit.code) {
            (Some(message), _) => message.clone(),
            (None, Some(code)) => format!("tsc exited with code {code}."),
            (None, None) => "tsc was killed.".into(),
        });
    }
    let (skipped, placed): (Vec<_>, Vec<_>) = placed.into_iter().partition(|d| d.code == REFERENCES_NEED_EMIT);
    let skipped = skipped.into_iter().filter_map(|d| d.place).map(|(path, ..)| relative(&path, root)).collect();
    Finished::Checked { problems: problems(&placed, root), skipped, general }
}

/// One diagnostic line of `tsc --pretty false`, with its continuation
/// lines folded into the message.
#[derive(Debug, PartialEq)]
struct Diagnostic {
    /// The file as tsc names it (relative to the root it ran in), and the
    /// 1-based line and UTF-16 column; `None` for a diagnostic about no
    /// file (a missing tsconfig.json).
    place: Option<(PathBuf, u32, u32)>,
    severity: Severity,
    code: u32,
    message: String,
}

/// Reads tsc's output: `file(line,col): error TS2322: message` or
/// `error TS6053: message`, each optionally followed by indented lines that
/// continue its message. Information and suggestions are left out.
fn parse(output: &str) -> Vec<Diagnostic> {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut continues = false;
    for line in output.lines() {
        if line.starts_with(' ') {
            if continues && let Some(last) = diagnostics.last_mut() {
                last.message.push('\n');
                last.message.push_str(line.strip_prefix("  ").unwrap_or(line));
            }
            continue;
        }
        let diagnostic = parse_line(line);
        continues = diagnostic.is_some();
        diagnostics.extend(diagnostic);
    }
    diagnostics
}

fn parse_line(line: &str) -> Option<Diagnostic> {
    for (category, severity) in [("error", Severity::Error), ("warning", Severity::Warning)] {
        let marker = format!("{category} TS");
        let (head, rest) = if let Some(rest) = line.strip_prefix(&marker) {
            (None, rest)
        } else if let Some(at) = line.find(&format!("): {marker}")) {
            (Some(&line[..at]), &line[at + 3 + marker.len()..])
        } else {
            continue;
        };
        let (code, message) = rest.split_once(": ")?;
        let place = match head {
            None => None,
            Some(head) => {
                let open = head.rfind('(')?;
                let (line, column) = head[open + 1..].split_once(',')?;
                Some((PathBuf::from(&head[..open]), line.parse().ok()?, column.parse().ok()?))
            }
        };
        return Some(Diagnostic { place, severity, code: code.parse().ok()?, message: message.to_owned() });
    }
    None
}

/// A path tsc printed, relative to the root when it is inside it.
fn relative(path: &Path, root: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_owned()
}

/// The diagnostics in files as Problems, with paths relative to `root`
/// and columns counted in chars (each file is read to convert them).
fn problems(diagnostics: &[Diagnostic], root: &Path) -> Vec<Problem> {
    let mut texts: HashMap<PathBuf, Rope> = HashMap::new();
    let mut problems = Vec::new();
    for diagnostic in diagnostics {
        let Some((path, line, column)) = &diagnostic.place else { continue };
        let path = relative(path, root);
        let text = texts.entry(path.clone()).or_insert_with(|| {
            std::fs::read_to_string(root.join(&path)).map(|text| Rope::from_str(&text)).unwrap_or_default()
        });
        let start: TextPosition =
            text::text_position(text, line.saturating_sub(1), column.saturating_sub(1), Encoding::Utf16);
        problems.push(Problem {
            severity: diagnostic.severity,
            path,
            start,
            end: start,
            message: diagnostic.message.clone(),
        });
    }
    problems
}
