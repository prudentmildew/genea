//! Line diffs of an open file against a base text, kept up to date in the
//! background as the buffer changes. The git gutter (ticket #56; the base
//! is the file at HEAD) and the inline diff (ticket #55; the base is the
//! review baseline) both use it.
//!
//! A [`BaseDiff`] holds one file's base and the hunks last worked out
//! against it. A [`DiffJob`] compares the base's lines with a snapshot of
//! the buffer on a background thread (`imara-diff`, histogram); one runs per
//! file at a time, and the next starts when it lands if the text (or the
//! base) moved on meanwhile, like the syntax parse. Until then the last
//! result is shown, so a diff can be a keystroke behind, but typing never
//! waits for it.

use std::{
    borrow::Cow,
    ops::Range,
    sync::atomic::{AtomicU64, Ordering},
};

use imara_diff::{Algorithm, Diff, InternedInput};
use ropey::Rope;

use crate::view::LineChange;

/// A run of lines that differ from the base.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Hunk {
    /// The base's lines (0-based, end exclusive); empty for added lines.
    pub(crate) base: Range<usize>,
    /// The buffer's lines; empty for deleted lines, which were just above
    /// `lines.start`.
    pub(crate) lines: Range<usize>,
}

impl Hunk {
    pub(crate) fn change(&self) -> LineChange {
        if self.lines.is_empty() {
            LineChange::Deleted
        } else if self.base.is_empty() {
            LineChange::Added
        } else {
            LineChange::Modified
        }
    }

    /// The hunk's lines in the base, with their line breaks.
    pub(crate) fn base_text(&self, base: &Rope) -> String {
        base.slice(base.line_to_char(self.base.start)..base.line_to_char(self.base.end)).to_string()
    }
}

/// One file's base and the hunks last worked out against it.
#[derive(Default)]
pub(crate) struct BaseDiff {
    /// `None`: there is nothing to compare with (not in HEAD, no
    /// repository, not read yet).
    base: Option<Rope>,
    /// Identifies `base`: every base set gets a fresh one.
    base_id: u64,
    /// How the buffer differed from the base, top to bottom.
    hunks: Vec<Hunk>,
    /// The buffer version and base id `hunks` are for.
    diffed: Option<(u64, u64)>,
    /// A diff job is running.
    running: bool,
}

/// A diff to run in the background: a file's base against a snapshot of
/// its buffer.
pub(crate) struct DiffJob {
    base: Rope,
    text: Rope,
    version: u64,
    base_id: u64,
}

/// A finished [`DiffJob`].
pub(crate) struct Diffed {
    hunks: Vec<Hunk>,
    version: u64,
    base_id: u64,
}

/// A base id no base has had before.
fn fresh_base_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl BaseDiff {
    /// Replaces the base (`None`: none). Without one there are no hunks.
    pub(crate) fn set_base(&mut self, base: Option<Rope>) {
        if base.is_none() {
            self.hunks.clear();
        }
        self.base = base;
        self.base_id = fresh_base_id();
    }

    pub(crate) fn base(&self) -> Option<&Rope> {
        self.base.as_ref()
    }

    /// The hunks last worked out, which may be behind the buffer.
    pub(crate) fn hunks(&self) -> &[Hunk] {
        if self.base.is_some() { &self.hunks } else { &[] }
    }

    /// A diff to run, if there is a base, the hunks are behind the buffer
    /// (`version`, `text`) or the base, and none is running.
    pub(crate) fn start(&mut self, version: u64, text: Rope) -> Option<DiffJob> {
        let base = self.base.clone()?;
        if self.running || self.diffed == Some((version, self.base_id)) {
            return None;
        }
        self.running = true;
        Some(DiffJob { base, text, version, base_id: self.base_id })
    }

    /// Takes a finished diff. Hunks against a base that has been replaced
    /// meanwhile are dropped.
    pub(crate) fn finish(&mut self, diffed: Diffed) {
        self.running = false;
        if diffed.base_id == self.base_id {
            self.hunks = diffed.hunks;
            self.diffed = Some((diffed.version, diffed.base_id));
        }
    }

    /// The hunks and the base, if the hunks are up to date with the
    /// buffer (`version`).
    pub(crate) fn current(&self, version: u64) -> Option<(&[Hunk], &Rope)> {
        let base = self.base.as_ref()?;
        (self.diffed == Some((version, self.base_id))).then_some((&self.hunks[..], base))
    }
}

impl DiffJob {
    pub(crate) fn run(self) -> Diffed {
        let mut input = InternedInput::default();
        input.update_before(lines(&self.base));
        input.update_after(lines(&self.text));
        let mut diff = Diff::compute(Algorithm::Histogram, &input);
        diff.postprocess_lines(&input);
        let hunks = diff
            .hunks()
            .map(|h| Hunk {
                base: h.before.start as usize..h.before.end as usize,
                lines: h.after.start as usize..h.after.end as usize,
            })
            .collect();
        Diffed { hunks, version: self.version, base_id: self.base_id }
    }
}

/// A line of text with its line break, as a diff token.
#[derive(Default, PartialEq, Eq, Hash)]
struct Line<'a>(Cow<'a, str>);

impl AsRef<[u8]> for Line<'_> {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

/// The text's lines as the editor counts them, each with its line break.
/// The empty line after a final line break isn't one.
fn lines(text: &Rope) -> impl Iterator<Item = Line<'_>> {
    text.lines().filter(|line| line.len_bytes() > 0).map(|line| Line(line.into()))
}
