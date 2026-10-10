//! The **project check** (ticket #48, spec #19 user stories 74–75): a
//! whole-project type check the user runs on demand, `tsc -b --noEmit`
//! with the project's own TypeScript 7 (the binary tsgo runs from). It
//! runs in the background; its results replace the previous check's in
//! Problems (`ProblemSource::ProjectCheck`) and stay until the next one.
//!
//! This module runs the process and reads what it prints; the project's
//! side (when to run, staleness, open files) is in `project/check.rs`.

use std::{
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
};

use genea_host::{Exit, ProcessSpec, SharedHost};
use ropey::Rope;

use crate::{
    lsp::text::{self, Encoding},
    problems::{Problem, Severity, TextPosition},
};

/// What `tsc` is run with. `--pretty false` gives one plain line per
/// diagnostic, `file(line,column): error TS1234: message`, whatever stdout
/// is.
pub(crate) const ARGS: [&str; 4] = ["-b", "--noEmit", "--pretty", "false"];

/// What a finished check found.
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    /// The diagnostics in files, as Problems.
    pub(crate) problems: Vec<Problem>,
}

/// Runs `spec` (with the project environment applied) to the end and reads
/// its diagnostics. Blocks: call it on a background thread.
pub(crate) fn run(host: &SharedHost, spec: &ProcessSpec, root: &Path) -> Result<Outcome, String> {
    let mut child = host.processes().spawn(spec).map_err(|e| e.to_string())?;
    drop(child.stdin.take());
    let mut stdout = String::new();
    if let Some(mut out) = child.stdout.take() {
        out.read_to_string(&mut stdout).map_err(|e| e.to_string())?;
    }
    let _exit: Exit = child.control.wait().map_err(|e| e.to_string())?;
    Ok(Outcome { problems: problems(&parse(&stdout), root) })
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
    for line in output.lines() {
        if line.starts_with(' ') {
            if let Some(last) = diagnostics.last_mut() {
                last.message.push('\n');
                last.message.push_str(line.strip_prefix("  ").unwrap_or(line));
            }
            continue;
        }
        if let Some(diagnostic) = parse_line(line) {
            diagnostics.push(diagnostic);
        }
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

/// The diagnostics in files as Problems, with paths relative to `root`
/// and columns counted in chars (each file is read to convert them).
fn problems(diagnostics: &[Diagnostic], root: &Path) -> Vec<Problem> {
    let mut texts: HashMap<PathBuf, Rope> = HashMap::new();
    let mut problems = Vec::new();
    for diagnostic in diagnostics {
        let Some((path, line, column)) = &diagnostic.place else { continue };
        let path = path.strip_prefix(root).unwrap_or(path).to_owned();
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
