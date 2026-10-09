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

use genea_core::{EditorView, Highlight, HighlightSpan, grid_pieces};
use slint::{Model, ModelRc, VecModel};
use unicode_width::UnicodeWidthChar;

use crate::{Line, ProjectWindow, Run, Span, SurfaceGeometry};

/// Menlo 13 pt × 1.2 (spec #19). Keep in step with `Theme.line-height` in
/// ui/theme.slint.
pub const LINE_HEIGHT: f32 = 15.6;

/// Line `y`s are relative to a base line so `f32` stays exact deep into a
/// huge file. The base moves (and every slot is rebuilt) past this distance.
const REBASE_LINES: usize = 10_000;

#[derive(PartialEq)]
struct SlotState {
    index: usize,
    base: usize,
    text: String,
    selections: Vec<Range<usize>>,
    highlights: Vec<HighlightSpan>,
    /// The cell width the runs and selections were laid out with.
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
}

impl Surface {
    pub fn new(window: &ProjectWindow, pane: usize) -> Self {
        let lines = Rc::new(VecModel::default());
        if pane == 0 {
            window.set_left_lines(ModelRc::from(lines.clone()));
        } else {
            window.set_right_lines(ModelRc::from(lines.clone()));
        }
        Surface { pane, lines, slots: Vec::new(), base: 0, rows: 0.0, scroll_top: 0.0 }
    }

    /// The viewport's height in rows, if it changed since the last call.
    pub fn take_viewport_change(&mut self, window: &ProjectWindow) -> Option<f64> {
        let rows = (window.get_viewport_height() / LINE_HEIGHT).max(1.0) as f64;
        (rows != self.rows).then(|| {
            self.rows = rows;
            rows
        })
    }

    /// The grid cell (line, display column) under a point in surface
    /// coordinates, rounding to the nearest cell boundary like a click.
    pub fn cell_at(&self, window: &ProjectWindow, x: f32, y: f32) -> (usize, usize) {
        let line = (self.scroll_top + (y / LINE_HEIGHT) as f64).floor().max(0.0) as usize;
        let char_width = window.get_char_width().max(1.0);
        let column = ((x - window.get_text_left()) / char_width).round().max(0.0) as usize;
        (line, column)
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
            let first = editor.lines.first().map_or(0, |l| l.index);
            if first < self.base || first - self.base > REBASE_LINES {
                self.base = first;
            }
            for line in &editor.lines {
                wanted[line.index % slot_count] = Some(SlotState {
                    index: line.index,
                    base: self.base,
                    text: line.text.clone(),
                    selections: line.selections.clone(),
                    highlights: line.highlights.clone(),
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
                    y: (s.index - s.base) as f32 * LINE_HEIGHT,
                    number: (s.index + 1).to_string().into(),
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
                },
            };
            self.lines.set_row_data(slot, row);
            self.slots[slot] = state;
        }

        let base = self.base as f64;
        let mut geometry = if self.pane == 0 { window.get_left_geometry() } else { window.get_right_geometry() };
        geometry.offset_y = -((self.scroll_top - base) * LINE_HEIGHT as f64) as f32;
        if let Some(editor) = editor {
            // While composing, the caret is drawn after the preedit.
            let preedit = editor.preedit.map_or(0, |p| p.width);
            geometry.compose_x = editor.caret.column as f32 * char_width;
            geometry.preedit_width = preedit as f32 * char_width;
            geometry.caret_x = (editor.caret.column + preedit) as f32 * char_width;
            geometry.caret_y = ((editor.caret.line as f64 - base) * LINE_HEIGHT as f64) as f32;
        }
        set_geometry(window, self.pane, geometry);
    }
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
