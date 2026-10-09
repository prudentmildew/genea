//! Problems: every error and warning Genea knows about in a project, from
//! every source, in one list (spec #19, Language intelligence → Problems).
//!
//! Each source owns its problems and replaces them wholesale, so sources
//! never step on each other:
//!
//! - a source that reports for the whole project at once (the config, the
//!   toolchain checks, the project check) calls [`Problems::replace`];
//! - a source that reports per file (tsgo's and Oxlint's
//!   `publishDiagnostics`) calls [`Problems::replace_file`].
//!
//! The Problems view, the status-bar counts and the editor's inline
//! underlines are all read from here. To add a source, add a
//! [`ProblemSource`] variant.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use crate::view::ProblemItem;

/// Where a problem comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProblemSource {
    /// `genea.jsonc`: syntax errors, bad values, unknown keys, nested
    /// configs.
    Config,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    Warning,
}

/// A place in a file: a 0-based line and a 0-based column counted in
/// chars (Unicode scalar values), not bytes, UTF-16 units or display
/// columns. Sources convert to it; the editor converts it to grid columns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextPosition {
    pub line: usize,
    pub column: usize,
}

impl TextPosition {
    /// The position of a byte offset into `text`. An offset inside a
    /// character, or past the end, counts as the next character boundary.
    pub(crate) fn of_byte_offset(text: &str, offset: usize) -> Self {
        let offset = offset.min(text.len());
        let before = &text.as_bytes()[..offset];
        let line = before.iter().filter(|&&b| b == b'\n').count();
        let line_start = before.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        let column = text[line_start..].char_indices().take_while(|(i, _)| line_start + i < offset).count();
        TextPosition { line, column }
    }
}

/// One error or warning, as a source reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Problem {
    pub(crate) severity: Severity,
    /// The file, relative to the project root.
    pub(crate) path: PathBuf,
    /// Where it starts and ends; the end is exclusive. An empty range marks
    /// a single place.
    pub(crate) start: TextPosition,
    pub(crate) end: TextPosition,
    pub(crate) message: String,
}

/// Every source's problems in a project.
#[derive(Default)]
pub(crate) struct Problems {
    by_source: BTreeMap<ProblemSource, BTreeMap<PathBuf, Vec<Problem>>>,
}

impl Problems {
    /// Replaces all of `source`'s problems, in every file.
    pub(crate) fn replace(&mut self, source: ProblemSource, problems: Vec<Problem>) {
        let mut by_file: BTreeMap<PathBuf, Vec<Problem>> = BTreeMap::new();
        for problem in problems {
            by_file.entry(problem.path.clone()).or_default().push(problem);
        }
        self.by_source.insert(source, by_file);
    }

    /// Replaces `source`'s problems in one file, leaving its other files'.
    /// Problems whose path isn't `path` are put under their own path anyway.
    #[allow(dead_code)] // For the per-file sources: tsgo (#42) and Oxlint (#49).
    pub(crate) fn replace_file(&mut self, source: ProblemSource, path: &Path, problems: Vec<Problem>) {
        let by_file = self.by_source.entry(source).or_default();
        by_file.remove(path);
        for problem in problems {
            by_file.entry(problem.path.clone()).or_default().push(problem);
        }
    }

    fn all(&self) -> impl Iterator<Item = (ProblemSource, &Problem)> {
        self.by_source.iter().flat_map(|(source, by_file)| by_file.values().flatten().map(|p| (*source, p)))
    }

    /// The problems in one file, from every source.
    pub(crate) fn in_file<'a>(&'a self, path: &'a Path) -> impl Iterator<Item = &'a Problem> + 'a {
        self.by_source.values().filter_map(move |by_file| by_file.get(path)).flatten()
    }

    /// (errors, warnings) over every source.
    pub(crate) fn counts(&self) -> (usize, usize) {
        self.all().fold((0, 0), |(errors, warnings), (_, p)| match p.severity {
            Severity::Error => (errors + 1, warnings),
            Severity::Warning => (errors, warnings + 1),
        })
    }

    /// The Problems view's items: by file, then by place in the file.
    pub(crate) fn items(&self) -> Vec<ProblemItem> {
        let mut problems: Vec<_> = self.all().collect();
        problems.sort_by(|(a_source, a), (b_source, b)| {
            (&a.path, a.start, a.severity, a_source).cmp(&(&b.path, b.start, b.severity, b_source))
        });
        problems
            .into_iter()
            .map(|(source, p)| ProblemItem {
                source,
                severity: p.severity,
                path: p.path.clone(),
                position: p.start,
                location: format!("{}:{}", p.start.line + 1, p.start.column + 1),
                message: p.message.clone(),
            })
            .collect()
    }
}
