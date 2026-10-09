//! Copying the emulator's visible grid into view state: text in grid
//! columns, runs of one style, the cursor.

use std::ops::Range;

use alacritty_terminal::{
    Term,
    event::EventListener,
    grid::Dimensions,
    term::{
        TermMode,
        cell::{Cell, Flags},
        color::Colors,
    },
    vte::ansi::{Color, CursorShape, NamedColor},
};

use super::links;
use crate::view::{TerminalColor, TerminalCursor, TerminalFileLink, TerminalLine, TerminalRun, TerminalStyle};

/// What the terminal shows, as the user sees it.
pub(super) struct Screen {
    pub(super) lines: Vec<TerminalLine>,
    pub(super) cursor: Option<TerminalCursor>,
    pub(super) history: usize,
    pub(super) scrolled_back: usize,
    pub(super) mode: TermMode,
}

/// Copies what the terminal shows.
pub(super) fn snapshot<T: EventListener>(term: &Term<T>) -> Screen {
    let content = term.renderable_content();
    let offset = content.display_offset as i32;
    let rows = term.screen_lines();
    let colors = content.colors;
    let mut builders: Vec<Row> = (0..rows).map(|_| Row::default()).collect();
    for indexed in content.display_iter {
        let Ok(row) = usize::try_from(indexed.point.line.0 + offset) else { continue };
        let Some(builder) = builders.get_mut(row) else { continue };
        // The second half of a wide character.
        if indexed.cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        builder.push(indexed.point.column.0, indexed.cell, colors);
        if indexed.cell.flags.contains(Flags::WRAPLINE) {
            builder.wraps = true;
        }
    }
    let mut lines: Vec<TerminalLine> = builders.iter().map(Row::finish).collect();
    find_links(&builders, &mut lines);
    let cursor = content.cursor;
    let line = cursor.point.line.0 + offset;
    let cursor = (cursor.shape != CursorShape::Hidden && (0..rows as i32).contains(&line))
        .then(|| TerminalCursor { line: line as usize, column: cursor.point.column.0 });
    Screen {
        lines,
        cursor,
        history: term.grid().history_size(),
        scrolled_back: content.display_offset,
        mode: content.mode,
    }
}

/// A row being copied.
#[derive(Default)]
struct Row {
    text: String,
    cells: Vec<Placed>,
    /// Columns up to the last one with text.
    text_end: usize,
    /// Columns up to the last one with something to paint: text, a
    /// background, an underline.
    paint_end: usize,
    /// The row's text goes on in the next row (it was wrapped).
    wraps: bool,
}

/// A cell's place on the row.
struct Placed {
    column: usize,
    /// Its bytes in `Row::text`.
    bytes: Range<usize>,
    width: usize,
    style: TerminalStyle,
}

impl Row {
    fn push(&mut self, column: usize, cell: &Cell, colors: &Colors) {
        let style = style(cell, colors);
        let blank_cell = cell.flags.intersects(Flags::HIDDEN | Flags::LEADING_WIDE_CHAR_SPACER);
        let c = if blank_cell || cell.c == '\t' || cell.c == '\0' { ' ' } else { cell.c };
        let start = self.text.len();
        self.text.push(c);
        if let Some(zero_width) = cell.zerowidth() {
            self.text.extend(zero_width);
        }
        let width = if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
        if c != ' ' {
            self.text_end = column + width;
        }
        if style.background != TerminalColor::Background || style.underline || style.strikeout {
            self.paint_end = column + width;
        }
        self.cells.push(Placed { column, bytes: start..self.text.len(), width, style });
    }

    fn finish(&self) -> TerminalLine {
        let text_bytes =
            self.cells.iter().take_while(|cell| cell.column < self.text_end).last().map_or(0, |cell| cell.bytes.end);
        let end = self.text_end.max(self.paint_end);
        let mut runs: Vec<TerminalRun> = Vec::new();
        for cell in self.cells.iter().take_while(|cell| cell.column < end) {
            let text = &self.text[cell.bytes.clone()];
            match runs.last_mut() {
                Some(run) if run.style == cell.style && run.columns.end == cell.column => {
                    run.columns.end += cell.width;
                    run.text.push_str(text);
                }
                _ => runs.push(TerminalRun {
                    columns: cell.column..cell.column + cell.width,
                    text: text.to_owned(),
                    style: cell.style.clone(),
                }),
            }
        }
        TerminalLine { text: self.text[..text_bytes].to_owned(), runs, links: Vec::new() }
    }

    /// The columns of the cells with text in `bytes` (of `Row::text`).
    fn columns(&self, bytes: Range<usize>) -> Option<Range<usize>> {
        let first = self.cells.iter().find(|cell| cell.bytes.end > bytes.start)?;
        let last = self.cells.iter().rev().find(|cell| cell.bytes.start < bytes.end)?;
        (first.column <= last.column).then(|| first.column..last.column + last.width)
    }
}

/// Finds the `path:line:col` references on each line of output, which
/// may run over several rows when it was wrapped, and puts each on the
/// rows it covers.
fn find_links(rows: &[Row], lines: &mut [TerminalLine]) {
    let mut first = 0;
    while first < rows.len() {
        let last = (first..rows.len()).find(|&row| !rows[row].wraps).unwrap_or(rows.len() - 1);
        // The output line's text, and where each row's starts in it.
        let mut text = String::new();
        let mut starts = Vec::new();
        for row in &rows[first..=last] {
            starts.push(text.len());
            text.push_str(&row.text);
        }
        for found in links::find(&text) {
            for (index, row) in rows[first..=last].iter().enumerate() {
                let start = starts[index];
                let bytes = found.bytes.start.max(start) - start..found.bytes.end.min(start + row.text.len()).max(start) - start;
                if bytes.is_empty() {
                    continue;
                }
                if let Some(columns) = row.columns(bytes) {
                    let link = TerminalFileLink { columns, path: found.path.clone(), at: found.at };
                    lines[first + index].links.push(link);
                }
            }
        }
        first = last + 1;
    }
}

/// A cell's style, with inverse and hidden applied.
fn style(cell: &Cell, colors: &Colors) -> TerminalStyle {
    let flags = cell.flags;
    let (mut foreground, mut background) = (color(cell.fg, colors), color(cell.bg, colors));
    if flags.contains(Flags::INVERSE) {
        (foreground, background) = (background, foreground);
    }
    if flags.contains(Flags::HIDDEN) {
        foreground = background;
    }
    TerminalStyle {
        foreground,
        background,
        bold: flags.contains(Flags::BOLD),
        italic: flags.contains(Flags::ITALIC),
        underline: flags.intersects(Flags::ALL_UNDERLINES),
        strikeout: flags.contains(Flags::STRIKEOUT),
        dim: flags.contains(Flags::DIM),
        link: cell.hyperlink().map(|link| link.uri().to_owned()),
    }
}

fn color(color: Color, colors: &Colors) -> TerminalColor {
    match color {
        Color::Spec(rgb) => TerminalColor::Rgb(rgb.r, rgb.g, rgb.b),
        Color::Indexed(index) => indexed(usize::from(index), colors),
        Color::Named(named) => {
            let index = named as usize;
            match named {
                _ if index < 16 => indexed(index, colors),
                NamedColor::Background => colors[index].map_or(TerminalColor::Background, rgb),
                NamedColor::DimBlack
                | NamedColor::DimRed
                | NamedColor::DimGreen
                | NamedColor::DimYellow
                | NamedColor::DimBlue
                | NamedColor::DimMagenta
                | NamedColor::DimCyan
                | NamedColor::DimWhite => indexed(index - NamedColor::DimBlack as usize, colors),
                _ => colors[NamedColor::Foreground as usize].map_or(TerminalColor::Foreground, rgb),
            }
        }
    }
}

/// A 256-colour index: the program may have redefined it (OSC 4); 0–15
/// are the theme's, the rest xterm's cube and grey ramp.
fn indexed(index: usize, colors: &Colors) -> TerminalColor {
    if let Some(defined) = colors[index] {
        return rgb(defined);
    }
    match index {
        0..16 => TerminalColor::Ansi(index as u8),
        16..232 => {
            let cube = index - 16;
            let level = |n: usize| if n == 0 { 0 } else { (55 + 40 * n) as u8 };
            TerminalColor::Rgb(level(cube / 36), level(cube / 6 % 6), level(cube % 6))
        }
        _ => {
            let grey = (8 + 10 * (index.min(255) - 232)) as u8;
            TerminalColor::Rgb(grey, grey, grey)
        }
    }
}

fn rgb(rgb: alacritty_terminal::vte::ansi::Rgb) -> TerminalColor {
    TerminalColor::Rgb(rgb.r, rgb.g, rgb.b)
}
