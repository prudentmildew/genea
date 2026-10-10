//! The editor surface's Rust half: maps the core's `EditorView` onto the
//! ring of line slots in `ui/editor-surface.slint`.
//!
//! Repaint rules (ADR 0004, "an idle caret must not repaint"):
//! - push to Slint only what changed. `VecModel::set_row_data` always
//!   notifies, so each slot's last state is cached and compared first;
//!   property setters already skip equal values;
//! - no timers. Scrolling and caret moves repaint because input changed
//!   something, never on a schedule.

use std::{ops::Range, rc::Rc};

use genea_core::{EditorView, Fold, Highlight, HighlightSpan, HunkView, LineChange, Severity, grid_pieces};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use unicode_width::UnicodeWidthChar;

use crate::{HunkPopup, Line, Mark, ProjectWindow, Run, Span, SurfaceGeometry, Theme};

/// A text row's height in the editor and the terminal: the zoomed font
/// size × 1.2 (spec #19; `Theme.line-height` in ui/theme.slint).
pub fn line_height(window: &ProjectWindow) -> f32 {
    window.global::<Theme>().get_line_height().max(1.0)
}

/// Lines at HEAD a change's popover lists; it says how many more there are.
const MAX_HUNK_LINES: usize = 20;

/// Line `y`s are relative to a base line so `f32` stays exact deep into a
/// huge file. The base moves (and every slot is rebuilt) past this distance.
const REBASE_LINES: usize = 10_000;

/// The gutter's fold-marker column, at its right edge: keep in step with
/// the marker's width in ui/editor-surface.slint.
const FOLD_MARKER_WIDTH: f32 = 14.0;

#[derive(PartialEq)]
struct SlotState {
    index: usize,
    /// The row it is drawn on: lines hidden in folds take none (#25).
    row: usize,
    base: usize,
    /// A fold region starts here: 0 none, 1 expanded, 2 collapsed.
    fold: i32,
    /// Display columns of highlighted matching brackets on the line.
    brackets: Vec<usize>,
    text: String,
    selections: Vec<Range<usize>>,
    /// Problems underlined on the line: columns, and whether it's an error.
    problems: Vec<(Range<usize>, bool)>,
    highlights: Vec<HighlightSpan>,
    /// Display columns of the carets on the line other than the primary.
    carets: Vec<usize>,
    /// The git gutter marker, as `Line.change` has it.
    change: i32,
    /// The cell width the runs, selections and carets were laid out with.
    char_width: f32,
}

/// One pane's surface (ticket #31): the left one is pane 0, the right one 1.
pub struct Surface {
    pane: usize,
    lines: Rc<VecModel<Line>>,
    slots: Vec<Option<SlotState>>,
    base: usize,
    rows: f64,
    scroll_top: f64,
    /// The shown git change and its popover's `y`, as last pushed.
    hunk: Option<(HunkView, f32)>,
}

impl Surface {
    pub fn new(window: &ProjectWindow, pane: usize) -> Self {
        let lines = Rc::new(VecModel::default());
        if pane == 0 {
            window.set_left_lines(ModelRc::from(lines.clone()));
        } else {
            window.set_right_lines(ModelRc::from(lines.clone()));
        }
        Surface { pane, lines, slots: Vec::new(), base: 0, rows: 0.0, scroll_top: 0.0, hunk: None }
    }

    /// The viewport's height in rows, if it changed since the last call.
    pub fn take_viewport_change(&mut self, window: &ProjectWindow) -> Option<f64> {
        let rows = (window.get_viewport_height() / line_height(window)).max(1.0) as f64;
        (rows != self.rows).then(|| {
            self.rows = rows;
            rows
        })
    }

    /// The grid cell (line, display column) under a point in surface
    /// coordinates, rounding to the nearest cell boundary like a click.
    pub fn cell_at(&self, window: &ProjectWindow, x: f32, y: f32) -> (usize, usize) {
        let row = (self.scroll_top + (y / line_height(window)) as f64).floor().max(0.0) as usize;
        let char_width = window.get_char_width().max(1.0);
        let column = ((x - window.get_text_left()) / char_width).round().max(0.0) as usize;
        (self.line_at_row(row), column)
    }

    /// The file line drawn on a row (rows skip folded lines). Rows below
    /// the last line drawn count on from it; the core clamps them.
    fn line_at_row(&self, row: usize) -> usize {
        let drawn = self.slots.iter().flatten();
        if let Some(slot) = drawn.clone().find(|s| s.row == row) {
            return slot.index;
        }
        match drawn.max_by_key(|s| s.row) {
            Some(last) if row > last.row => last.index + (row - last.row),
            _ => row,
        }
    }

    /// The row a file line is drawn on, or would be: a line hidden in a
    /// fold counts as the row after the last line drawn above it, and
    /// lines outside the drawn ones count on from its ends.
    fn row_of_line(&self, line: usize) -> f64 {
        let drawn = self.slots.iter().flatten();
        let above = drawn.clone().filter(|s| s.index <= line).max_by_key(|s| s.index);
        match above {
            Some(s) if s.index == line => s.row as f64,
            Some(s) if drawn.clone().any(|d| d.index > line) => s.row as f64 + 1.0,
            Some(s) => (s.row + (line - s.index)) as f64,
            None => match drawn.min_by_key(|s| s.index) {
                Some(first) => first.row as f64 - (first.index - line) as f64,
                None => line as f64,
            },
        }
    }

    /// The line whose fold marker is under a point in the gutter, if any.
    pub fn fold_marker_at(&self, window: &ProjectWindow, x: f32, y: f32) -> Option<usize> {
        let text_left = window.get_text_left();
        if x >= text_left || x < text_left - FOLD_MARKER_WIDTH {
            return None;
        }
        let row = (self.scroll_top + (y / line_height(window)) as f64).floor().max(0.0) as usize;
        self.slots.iter().flatten().find(|s| s.row == row && s.fold != 0).map(|s| s.index)
    }

    /// The grid cell a point is inside, for picking the word under it.
    pub fn cell_under(&self, window: &ProjectWindow, x: f32, y: f32) -> (usize, usize) {
        let (line, _) = self.cell_at(window, x, y);
        let char_width = window.get_char_width().max(1.0);
        (line, ((x - window.get_text_left()) / char_width).floor().max(0.0) as usize)
    }

    pub fn sync(&mut self, window: &ProjectWindow, editor: Option<&EditorView>) {
        let slot_count = self.rows.max(1.0).ceil() as usize + 1;
        if self.lines.row_count() != slot_count {
            self.lines.set_vec(vec![Line::default(); slot_count]);
            self.slots = (0..slot_count).map(|_| None).collect();
        }

        let char_width = window.get_char_width();
        let mut wanted: Vec<Option<SlotState>> = (0..slot_count).map(|_| None).collect();
        if let Some(editor) = editor {
            let first = editor.lines.first().map_or(0, |l| l.row);
            if first < self.base || first - self.base > REBASE_LINES {
                self.base = first;
            }
            for line in &editor.lines {
                wanted[line.row % slot_count] = Some(SlotState {
                    index: line.index,
                    row: line.row,
                    base: self.base,
                    fold: match line.fold {
                        None => 0,
                        Some(Fold::Expanded) => 1,
                        Some(Fold::Collapsed) => 2,
                    },
                    brackets: editor.brackets.iter().filter(|b| b.line == line.index).map(|b| b.column).collect(),
                    text: line.text.clone(),
                    selections: line.selections.clone(),
                    problems: editor
                        .problems
                        .iter()
                        .filter(|p| p.line == line.index)
                        .map(|p| (p.columns.clone(), p.severity == Severity::Error))
                        .collect(),
                    highlights: line.highlights.clone(),
                    carets: editor
                        .carets
                        .iter()
                        .filter(|c| c.line == line.index && **c != editor.caret)
                        .map(|c| c.column)
                        .collect(),
                    change: editor.gutter.iter().find(|m| m.line == line.index).map_or(0, |m| match m.change {
                        LineChange::Added => 1,
                        LineChange::Modified => 2,
                        LineChange::Deleted => 3,
                    }),
                    char_width,
                });
            }
            self.scroll_top = editor.scroll_top;
        }

        for (slot, state) in wanted.into_iter().enumerate() {
            if self.slots[slot] == state {
                continue;
            }
            let row = match &state {
                None => Line { y: -1000.0, ..Line::default() },
                Some(s) => Line {
                    y: (s.row - s.base) as f32 * line_height(window),
                    number: (s.index + 1).to_string().into(),
                    fold: s.fold,
                    fold_x: s.text.chars().map(|c| c.width().unwrap_or(0)).sum::<usize>() as f32 * char_width,
                    brackets: if s.brackets.is_empty() {
                        ModelRc::default()
                    } else {
                        ModelRc::new(VecModel::from(
                            s.brackets.iter().map(|&column| column as f32 * char_width).collect::<Vec<_>>(),
                        ))
                    },
                    runs: if s.text.trim().is_empty() {
                        ModelRc::default()
                    } else {
                        ModelRc::new(VecModel::from(runs(&s.text, &s.highlights, char_width)))
                    },
                    selections: if s.selections.is_empty() {
                        ModelRc::default()
                    } else {
                        ModelRc::new(VecModel::from(
                            s.selections
                                .iter()
                                .map(|r| Span { x: r.start as f32 * char_width, width: r.len() as f32 * char_width })
                                .collect::<Vec<_>>(),
                        ))
                    },
                    problems: if s.problems.is_empty() {
                        ModelRc::default()
                    } else {
                        ModelRc::new(VecModel::from(
                            s.problems
                                .iter()
                                .map(|(r, error)| Mark {
                                    x: r.start as f32 * char_width,
                                    width: r.len() as f32 * char_width,
                                    error: *error,
                                })
                                .collect::<Vec<_>>(),
                        ))
                    },
                    carets: if s.carets.is_empty() {
                        ModelRc::default()
                    } else {
                        ModelRc::new(VecModel::from(
                            s.carets.iter().map(|&column| column as f32 * char_width).collect::<Vec<_>>(),
                        ))
                    },
                    change: s.change,
                },
            };
            self.lines.set_row_data(slot, row);
            self.slots[slot] = state;
        }

        let base = self.base as f64;
        let mut geometry = if self.pane == 0 { window.get_left_geometry() } else { window.get_right_geometry() };
        geometry.offset_y = -((self.scroll_top - base) * line_height(window) as f64) as f32;
        if let Some(editor) = editor {
            // While composing, the caret is drawn after the preedit.
            let preedit = editor.preedit.map_or(0, |p| p.width);
            geometry.compose_x = editor.caret.column as f32 * char_width;
            geometry.preedit_width = preedit as f32 * char_width;
            geometry.caret_x = (editor.caret.column + preedit) as f32 * char_width;
            // The caret's row; off the surface when its line isn't drawn.
            let caret_row = match editor.lines.iter().find(|l| l.index == editor.caret.line) {
                Some(line) => line.row as f64,
                None if editor.lines.first().is_some_and(|l| editor.caret.line < l.index) => base - 1e6,
                None => base + 1e6,
            };
            geometry.caret_y = ((caret_row - base) * line_height(window) as f64) as f32;
        }
        set_geometry(window, self.pane, geometry);
        self.sync_hunk(window, editor.and_then(|e| e.hunk.as_ref()));
    }

    /// Shows or hides the git change's popover, below the change's lines
    /// (or on the edge where lines were deleted), when it changed.
    fn sync_hunk(&mut self, window: &ProjectWindow, hunk: Option<&HunkView>) {
        let wanted = hunk.map(|h| {
            let below = if h.lines.is_empty() { h.lines.start } else { h.lines.end };
            (h.clone(), (self.row_of_line(below) - self.base as f64) as f32 * line_height(window))
        });
        if wanted == self.hunk {
            return;
        }
        let popup = match hunk {
            None => HunkPopup::default(),
            Some(hunk) => {
                let lines: Vec<&str> = if hunk.head.is_empty() { Vec::new() } else { hunk.head.split('\n').collect() };
                let head: Vec<slint::SharedString> =
                    lines.iter().take(MAX_HUNK_LINES).map(|line| expand_tabs(line).into()).collect();
                let more = lines.len().saturating_sub(MAX_HUNK_LINES);
                HunkPopup {
                    shown: true,
                    y: wanted.as_ref().map_or(0.0, |(_, y)| *y),
                    head: ModelRc::new(VecModel::from(head)),
                    more: if more == 0 { "".into() } else { format!("{more} more lines at HEAD").into() },
                }
            }
        };
        if self.pane == 0 {
            window.set_left_hunk(popup);
        } else {
            window.set_right_hunk(popup);
        }
        self.hunk = wanted;
    }
}

/// A line with its tabs expanded to the grid's tab stops (every 4 columns).
fn expand_tabs(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut column = 0;
    for c in line.chars() {
        if c == '\t' {
            let next = (column / 4 + 1) * 4;
            out.extend(std::iter::repeat_n(' ', next - column));
            column = next;
        } else {
            out.push(c);
            column += c.width().unwrap_or(0);
        }
    }
    out
}

/// Sets a pane's geometry; Slint skips an equal value, so nothing repaints.
fn set_geometry(window: &ProjectWindow, pane: usize, geometry: SurfaceGeometry) {
    if pane == 0 {
        window.set_left_geometry(geometry);
    } else {
        window.set_right_geometry(geometry);
    }
}

/// Splits a line's grid text into runs: one per highlight (and per plain
/// stretch between them), each split further by `grid_pieces` so that every
/// wide character (CJK, emoji), whose fallback font's advance isn't two
/// Menlo cells, sits at its own column and the rest of the line stays on the
/// grid. Blank runs are left out.
fn runs(text: &str, highlights: &[HighlightSpan], char_width: f32) -> Vec<Run> {
    let mut runs = Vec::new();
    let mut push = |segment: &str, column: usize, highlight: i32| {
        for piece in grid_pieces(segment) {
            if !piece.text.trim().is_empty() {
                let x = (column + piece.column) as f32 * char_width;
                runs.push(Run { x, text: piece.text.into(), highlight });
            }
        }
    };
    let mut spans = highlights.iter().peekable();
    let (mut start, mut start_column, mut highlight) = (0, 0, 0);
    let mut column = 0;
    for (index, c) in text.char_indices() {
        let width = c.width().unwrap_or(0);
        // A zero-width character stays with the one before it.
        if width > 0 {
            while spans.next_if(|s| s.columns.end <= column).is_some() {}
            let here = spans.peek().filter(|s| s.columns.start <= column).map_or(0, |s| highlight_index(s.highlight));
            if index > start && here != highlight {
                push(&text[start..index], start_column, highlight);
                (start, start_column) = (index, column);
            }
            if index == start {
                highlight = here;
            }
        }
        column += width;
    }
    push(&text[start..], start_column, highlight);
    runs
}

/// A highlight's index into `Theme.syntax` (ui/theme.slint); 0 is plain.
fn highlight_index(highlight: Highlight) -> i32 {
    match highlight {
        Highlight::Comment => 1,
        Highlight::Keyword => 2,
        Highlight::Operator => 3,
        Highlight::Punctuation => 4,
        Highlight::String => 5,
        Highlight::StringSpecial => 6,
        Highlight::Escape => 7,
        Highlight::Number => 8,
        Highlight::Constant => 9,
        Highlight::Function => 10,
        Highlight::Constructor => 11,
        Highlight::Type => 12,
        Highlight::TypeBuiltin => 13,
        Highlight::Variable => 14,
        Highlight::VariableBuiltin => 15,
        Highlight::Parameter => 16,
        Highlight::Property => 17,
        Highlight::Tag => 18,
        Highlight::Attribute => 19,
        Highlight::Label => 20,
        Highlight::Heading => 21,
        Highlight::Emphasis => 22,
        Highlight::Strong => 23,
        Highlight::Link => 24,
        Highlight::Literal => 25,
    }
}
