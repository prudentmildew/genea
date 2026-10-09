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

use genea_core::{EditorView, grid_pieces};
use slint::{Color, Model, ModelRc, VecModel};

use crate::{Line, ProjectWindow, Run, Span, SurfaceGeometry};

/// Menlo 13 pt × 1.2 (spec #19). Keep in step with `Theme.line-height` in
/// ui/theme.slint.
pub const LINE_HEIGHT: f32 = 15.6;

/// Line `y`s are relative to a base line so `f32` stays exact deep into a
/// huge file. The base moves (and every slot is rebuilt) past this distance.
const REBASE_LINES: usize = 10_000;

/// `Theme.editor-foreground`. Highlighting gives runs their own colours.
const FOREGROUND: u32 = 0x24292f;

#[derive(PartialEq)]
struct SlotState {
    index: usize,
    base: usize,
    text: String,
    selections: Vec<Range<usize>>,
    /// Display columns of the carets on the line other than the primary.
    carets: Vec<usize>,
    /// The cell width the selections were laid out with.
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
                    carets: editor
                        .carets
                        .iter()
                        .filter(|c| c.line == line.index && **c != editor.caret)
                        .map(|c| c.column)
                        .collect(),
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
                        ModelRc::new(VecModel::from(grid_runs(&s.text, char_width)))
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
                    carets: if s.carets.is_empty() {
                        ModelRc::default()
                    } else {
                        ModelRc::new(VecModel::from(
                            s.carets.iter().map(|&column| column as f32 * char_width).collect::<Vec<_>>(),
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

/// A line's runs, one per grid piece: wide characters (CJK, emoji) are
/// drawn from fallback fonts whose advances aren't two Menlo cells, so each
/// is placed at its own column to keep the rest of the line on the grid.
fn grid_runs(text: &str, char_width: f32) -> Vec<Run> {
    grid_pieces(text)
        .into_iter()
        .map(|piece| Run {
            x: piece.column as f32 * char_width,
            text: piece.text.into(),
            color: Color::from_argb_encoded(0xff00_0000 | FOREGROUND),
        })
        .collect()
}
