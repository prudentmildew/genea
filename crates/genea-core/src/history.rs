//! Undo history: a linear stack of transactions per buffer (spec #19,
//! Editing core; ticket #22).
//!
//! A transaction is one undo step: the changes one or more edits made, the
//! selection before and after them, and the buffer versions on either side
//! (so undoing back to the saved text shows the file as saved again).
//! Consecutive edits of the same kind group into one transaction until a
//! pause of [`GROUP_PAUSE`] on the host clock, a caret jump (the edit starts
//! somewhere other than where the last one left the caret), a switch
//! between typing and deleting, or an undo or redo.
//!
//! The history is independent of saving: it lasts as long as the buffer.

use std::time::{Duration, Instant};

use ropey::Rope;

/// A pause in typing at least this long starts a new undo step.
pub(crate) const GROUP_PAUSE: Duration = Duration::from_secs(1);

/// The selection as char indices: `caret` moves, `anchor` stays. Equal
/// when nothing is selected. Multi-caret (#52) turns this into a list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Selection {
    pub(crate) anchor: usize,
    pub(crate) caret: usize,
}

impl Selection {
    fn is_empty(self) -> bool {
        self.anchor == self.caret
    }
}

/// What kind of edit made a change, which decides whether it can join the
/// previous undo step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditKind {
    /// Typed text and line breaks: groups with more typing.
    Typing,
    /// Backspace, Delete and their word variants: groups with more deleting.
    Deleting,
    /// Paste, Cut, and anything else that is a step of its own.
    Other,
}

/// One primitive change to the text, in char indices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Change {
    Insert { at: usize, text: String },
    Remove { at: usize, text: String },
}

impl Change {
    fn apply(&self, rope: &mut Rope) {
        match self {
            Change::Insert { at, text } => rope.insert(*at, text),
            Change::Remove { at, text } => rope.remove(*at..*at + text.chars().count()),
        }
    }

    fn inverse(&self) -> Change {
        match self {
            Change::Insert { at, text } => Change::Remove { at: *at, text: text.clone() },
            Change::Remove { at, text } => Change::Insert { at: *at, text: text.clone() },
        }
    }
}

/// One edit as the editor made it, ready to be recorded.
pub(crate) struct Edit {
    pub(crate) kind: EditKind,
    /// On the host clock.
    pub(crate) at: Instant,
    pub(crate) changes: Vec<Change>,
    pub(crate) before: Selection,
    pub(crate) after: Selection,
    pub(crate) version_before: u64,
    pub(crate) version_after: u64,
}

/// One undo step.
struct Transaction {
    kind: EditKind,
    /// When its last edit was made.
    last_edit: Instant,
    /// In the order they were made.
    changes: Vec<Change>,
    before: Selection,
    after: Selection,
    version_before: u64,
    version_after: u64,
}

impl Transaction {
    /// Adds a change, merging it with the last one when it continues it
    /// (typing on, Backspace or Delete again), so a long run of typing stays
    /// a handful of changes.
    fn push(&mut self, change: Change) {
        match (self.changes.last_mut(), change) {
            (Some(Change::Insert { at, text }), Change::Insert { at: next, text: more })
                if next == *at + text.chars().count() =>
            {
                text.push_str(&more);
            }
            (Some(Change::Remove { at, text }), Change::Remove { at: next, text: more })
                if next + more.chars().count() == *at =>
            {
                text.insert_str(0, &more);
                *at = next;
            }
            (Some(Change::Remove { at, text }), Change::Remove { at: next, text: more }) if next == *at => {
                text.push_str(&more);
            }
            (_, change) => self.changes.push(change),
        }
    }
}

/// Where the buffer is after an undo or redo.
pub(crate) struct Restored {
    pub(crate) selection: Selection,
    pub(crate) version: u64,
}

#[derive(Default)]
pub(crate) struct History {
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    /// The top undo step can't take more edits (after an undo or redo).
    sealed: bool,
}

impl History {
    /// Records an edit: it joins the last undo step if it continues it,
    /// otherwise it starts a new one. Either way, redo is no longer possible.
    pub(crate) fn record(&mut self, edit: Edit) {
        if edit.changes.is_empty() {
            return;
        }
        self.redo.clear();
        let continues = |last: &Transaction| {
            edit.kind != EditKind::Other
                && edit.kind == last.kind
                && edit.at.saturating_duration_since(last.last_edit) < GROUP_PAUSE
                && edit.before.is_empty()
                && edit.before == last.after
                && edit.version_before == last.version_after
        };
        match self.undo.last_mut() {
            Some(last) if !self.sealed && continues(last) => {
                for change in edit.changes {
                    last.push(change);
                }
                last.last_edit = edit.at;
                last.after = edit.after;
                last.version_after = edit.version_after;
            }
            _ => {
                let mut transaction = Transaction {
                    kind: edit.kind,
                    last_edit: edit.at,
                    changes: Vec::new(),
                    before: edit.before,
                    after: edit.after,
                    version_before: edit.version_before,
                    version_after: edit.version_after,
                };
                for change in edit.changes {
                    transaction.push(change);
                }
                self.undo.push(transaction);
                self.sealed = false;
            }
        }
    }

    /// Reverts the last undo step in `text`.
    pub(crate) fn undo(&mut self, text: &mut Rope) -> Option<Restored> {
        let transaction = self.undo.pop()?;
        for change in transaction.changes.iter().rev() {
            change.inverse().apply(text);
        }
        let restored = Restored { selection: transaction.before, version: transaction.version_before };
        self.redo.push(transaction);
        self.sealed = true;
        Some(restored)
    }

    /// Makes the last undone step again in `text`.
    pub(crate) fn redo(&mut self, text: &mut Rope) -> Option<Restored> {
        let transaction = self.redo.pop()?;
        for change in &transaction.changes {
            change.apply(text);
        }
        let restored = Restored { selection: transaction.after, version: transaction.version_after };
        self.undo.push(transaction);
        self.sealed = true;
        Some(restored)
    }
}
