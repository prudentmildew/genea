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
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use crate::view::ProblemItem;

/// Where a problem comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProblemSource {
    /// `genea.jsonc`: syntax errors, bad values, unknown keys, nested
    /// configs.
    Config,
    /// The toolchain checks: a root lockfile that doesn't match
    /// `packageManager`, or both a pnpm and a Bun lockfile.
    Toolchain,
    /// tsgo's live diagnostics for open files (ticket #42).
    TypeScript,
    /// The last project check's results (ticket #48): `tsc -b --noEmit`
    /// over the whole project, kept until the next check.
    ProjectCheck,
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
    /// Files open on a language server: their live diagnostics take over
    /// from the project check's stored results, which are hidden there.
    live: BTreeSet<PathBuf>,
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
    pub(crate) fn replace_file(&mut self, source: ProblemSource, path: &Path, problems: Vec<Problem>) {
        let by_file = self.by_source.entry(source).or_default();
        by_file.remove(path);
        for problem in problems {
            by_file.entry(problem.path.clone()).or_default().push(problem);
        }
    }

    /// Sets the files open on a language server, whose project-check
    /// results are hidden while they are open.
    pub(crate) fn set_live(&mut self, paths: BTreeSet<PathBuf>) {
        self.live = paths;
    }

    /// Whether `source`'s problems in `path` show.
    fn shows(&self, source: ProblemSource, path: &Path) -> bool {
        source != ProblemSource::ProjectCheck || !self.live.contains(path)
    }

    fn all(&self) -> impl Iterator<Item = (ProblemSource, &Problem)> {
        self.by_source.iter().flat_map(move |(source, by_file)| {
            by_file.iter().filter(move |(path, _)| self.shows(*source, path)).flat_map(move |(_, problems)| {
                problems.iter().map(move |p| (*source, p))
            })
        })
    }

    /// The problems in one file, from every source.
    pub(crate) fn in_file<'a>(&'a self, path: &'a Path) -> impl Iterator<Item = &'a Problem> + 'a {
        self.by_source
            .iter()
            .filter(move |(source, _)| self.shows(**source, path))
            .filter_map(move |(_, by_file)| by_file.get(path))
            .flatten()
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
