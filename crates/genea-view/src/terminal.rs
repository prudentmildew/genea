//! The terminal pane's Rust half (tickets #38, #39): maps the core's
//! `TerminalView` onto the pane's tab strip and row slots in
//! `ui/terminal-pane.slint`, and turns the pane's keys and mouse into
//! commands.
//!
//! Repaint rules as for the editor surface (ADR 0004): each row's last
//! state is cached and only changed rows are pushed, so an idle terminal
//! never repaints.

use std::rc::Rc;

use genea_core::{
    Command, Modifiers as TermModifiers, MouseAction, MouseButton, TerminalColor, TerminalKey, TerminalLine,
    TerminalRun, TerminalStatus, TerminalView, grid_pieces,
};
use slint::{Color, Model, ModelRc, SharedString, VecModel, platform::Key};
use unicode_width::UnicodeWidthStr;

use crate::{
    ProjectWindow, TermCursor, TermLink, TermRow, TermRun as SlintRun, TermTab, fonts, keys::Modifiers,
    surface::LINE_HEIGHT,
};

pub struct TerminalSurface {
    rows: Rc<VecModel<TermRow>>,
    /// Each row as last pushed, and the cell width it was laid out with.
    slots: Vec<Option<(TerminalLine, f32)>>,
    /// The size last reported to the core.
    size: Option<(usize, usize)>,
    /// The lines last shown, to find a link under a ⌘-click.
    lines: Vec<TerminalLine>,
    /// The button held over the grid, for drag reports.
    held: Option<MouseButton>,
    /// Scroll-wheel distance not yet a whole row.
    scroll_rest: f32,
    /// The tabs as last pushed.
    tabs: Vec<TermTab>,
}

impl TerminalSurface {
    pub fn new(window: &ProjectWindow) -> Self {
        let rows = Rc::new(VecModel::default());
        window.set_terminal_rows(ModelRc::from(rows.clone()));
        TerminalSurface {
            rows,
            slots: Vec::new(),
            size: None,
            lines: Vec::new(),
            held: None,
            scroll_rest: 0.0,
            tabs: Vec::new(),
        }
    }

    /// The grid's size in cells, if it changed since the last call.
    pub fn take_size_change(&mut self, window: &ProjectWindow) -> Option<(usize, usize)> {
        let char_width = window.get_terminal_char_width();
        if char_width <= 0.0 || window.get_terminal_grid_width() <= 0.0 {
            return None;
        }
        let columns = ((window.get_terminal_grid_width() / char_width).floor() as usize).max(2);
        let rows = ((window.get_terminal_grid_height() / LINE_HEIGHT).floor() as usize).max(1);
        (self.size != Some((rows, columns))).then(|| {
            self.size = Some((rows, columns));
            (rows, columns)
        })
    }

    /// The cell under a point in grid coordinates.
    pub fn cell_at(&self, window: &ProjectWindow, x: f32, y: f32) -> (usize, usize) {
        let char_width = window.get_terminal_char_width().max(1.0);
        ((y / LINE_HEIGHT).max(0.0) as usize, (x / char_width).max(0.0) as usize)
    }

    /// The link under a cell, if any.
    pub fn link_at(&self, line: usize, column: usize) -> Option<String> {
        let run = self.lines.get(line)?.runs.iter().find(|run| run.columns.contains(&column))?;
        run.style.link.clone()
    }

    /// A mouse event over the grid (`kind` 0 press, 1 release, 2 move;
    /// `button` 0 none, 1 left, 2 middle, 3 right) as a command.
    pub fn mouse(&mut self, kind: i32, button: i32, line: usize, column: usize, m: Modifiers) -> Option<Command> {
        let button = match button {
            1 => Some(MouseButton::Left),
            2 => Some(MouseButton::Middle),
            3 => Some(MouseButton::Right),
            _ => None,
        };
        let action = match (kind, button) {
            (0, Some(button)) => {
                self.held = Some(button);
                MouseAction::Press(button)
            }
            (1, Some(button)) => {
                self.held = None;
                MouseAction::Release(button)
            }
            (2, _) => self.held.map_or(MouseAction::Move, MouseAction::Drag),
            _ => return None,
        };
        let modifiers = TermModifiers { shift: m.shift, alt: m.alt, ctrl: m.ctrl };
        Some(Command::TerminalMouse { action, line, column, modifiers })
    }

    /// The wheel over the grid, as whole rows (negative is up).
    pub fn scroll(&mut self, delta_y: f32) -> Option<i32> {
        self.scroll_rest -= delta_y / LINE_HEIGHT;
        let rows = self.scroll_rest.trunc();
        self.scroll_rest -= rows;
        (rows != 0.0).then_some(rows as i32)
    }

    pub fn sync(&mut self, window: &ProjectWindow, view: &TerminalView) {
        window.set_terminal_visible(view.visible);
        window.set_terminal_focused(view.focused);
        let tabs: Vec<TermTab> = view
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| TermTab {
                title: tab.title.as_str().into(),
                active: index == view.active_tab,
                ended: matches!(tab.status, TerminalStatus::Exited { .. } | TerminalStatus::Failed(_)),
            })
            .collect();
        if self.tabs != tabs {
            window.set_terminal_tabs(ModelRc::new(VecModel::from(tabs.clone())));
            self.tabs = tabs;
        }
        let shell = view.tabs.get(view.active_tab).is_none_or(|tab| tab.shell);
        window.set_terminal_message(message(&view.status, shell, &view.title).into());
        window.set_terminal_mouse_reporting(view.mouse_reporting);

        let char_width = window.get_terminal_char_width();
        if self.rows.row_count() != view.lines.len() {
            self.rows.set_vec(vec![TermRow::default(); view.lines.len()]);
            self.slots = vec![None; view.lines.len()];
        }
        for (index, line) in view.lines.iter().enumerate() {
            if self.slots[index].as_ref().is_some_and(|(last, width)| last == line && *width == char_width) {
                continue;
            }
            fonts::prepare(&line.text);
            let row = TermRow {
                y: index as f32 * LINE_HEIGHT,
                runs: if line.runs.is_empty() {
                    ModelRc::default()
                } else {
                    ModelRc::new(VecModel::from(runs(&line.runs, char_width)))
                },
                links: if line.links.is_empty() {
                    ModelRc::default()
                } else {
                    let links = line.links.iter().map(|link| TermLink {
                        x: link.columns.start as f32 * char_width,
                        width: link.columns.len() as f32 * char_width,
                    });
                    ModelRc::new(VecModel::from_iter(links))
                },
            };
            self.rows.set_row_data(index, row);
            self.slots[index] = Some((line.clone(), char_width));
        }
        self.lines = view.lines.clone();

        let preedit = view.preedit.clone().unwrap_or_default();
        let cursor = match view.cursor {
            Some(cursor) if view.status == TerminalStatus::Running => TermCursor {
                visible: true,
                x: cursor.column as f32 * char_width,
                y: cursor.line as f32 * LINE_HEIGHT,
                preedit_width: preedit.width() as f32 * char_width,
                preedit: preedit.into(),
            },
            _ => TermCursor::default(),
        };
        window.set_terminal_cursor(cursor);
    }
}

/// What the pane says over the grid while the showing tab's program
/// isn't running: the shell's, or a command's (`title`).
fn message(status: &TerminalStatus, shell: bool, title: &str) -> String {
    if !shell {
        return match status {
            TerminalStatus::Starting => format!("Starting {title}…"),
            TerminalStatus::Running => String::new(),
            TerminalStatus::Exited { code: Some(0) } => format!("{title} finished."),
            TerminalStatus::Exited { code: Some(code) } => format!("{title} failed with code {code}."),
            TerminalStatus::Exited { code: None } => format!("{title} was stopped."),
            TerminalStatus::Failed(reason) => reason.clone(),
        };
    }
    match status {
        TerminalStatus::Starting => "Starting your shell…".into(),
        TerminalStatus::Running => String::new(),
        TerminalStatus::Exited { code: Some(code) } => {
            format!("The shell exited with code {code}. Press Return to start a new one.")
        }
        TerminalStatus::Exited { code: None } => "The shell was stopped. Press Return to start a new one.".into(),
        TerminalStatus::Failed(reason) => format!("{reason}. Press Return to try again."),
    }
}

/// A row's runs as Slint draws them. A run with wide characters is drawn
/// as its background, then each piece at its own column (their fallback
/// fonts' advances aren't two Menlo cells). Blank runs with nothing to
/// paint are left out.
fn runs(runs: &[TerminalRun], char_width: f32) -> Vec<SlintRun> {
    let mut out = Vec::new();
    for run in runs {
        let style = &run.style;
        let blank = run.text.trim().is_empty();
        let painted = style.background != TerminalColor::Background || style.underline || style.strikeout;
        if blank && !painted {
            continue;
        }
        let (fg_index, fg) = color(style.foreground);
        let (bg_index, bg) = color(style.background);
        let base = SlintRun {
            x: run.columns.start as f32 * char_width,
            width: run.columns.len() as f32 * char_width,
            text: SharedString::default(),
            fg_index,
            fg,
            bg_index,
            bg,
            bold: style.bold,
            italic: style.italic,
            underline: style.underline,
            strikeout: style.strikeout,
            dim: style.dim,
            link: style.link.is_some(),
        };
        let pieces = grid_pieces(&run.text);
        if pieces.len() <= 1 {
            out.push(SlintRun { text: run.text.as_str().into(), ..base });
            continue;
        }
        // The background and lines, then the text piece by piece.
        out.push(base.clone());
        for piece in pieces.into_iter().filter(|piece| !piece.text.trim().is_empty()) {
            out.push(SlintRun {
                x: (run.columns.start + piece.column) as f32 * char_width,
                width: piece.text.width() as f32 * char_width,
                text: piece.text.into(),
                bg_index: 1,
                underline: false,
                strikeout: false,
                link: false,
                ..base.clone()
            });
        }
    }
    out
}

/// A colour as an index into `Theme.terminal-palette`, or -1 and the exact
/// colour.
fn color(color: TerminalColor) -> (i32, Color) {
    match color {
        TerminalColor::Foreground => (0, Color::default()),
        TerminalColor::Background => (1, Color::default()),
        TerminalColor::Ansi(n) => (2 + i32::from(n.min(15)), Color::default()),
        TerminalColor::Rgb(r, g, b) => (-1, Color::from_rgb_u8(r, g, b)),
    }
}

/// A key press in the terminal as a command. Menu shortcuts (⌘V, ⌥F12)
/// never get here; other ⌘ keys do nothing, except ⌘← and ⌘→ (start and
/// end of the line) and ⌘⌫ (delete the line), as in the macOS terminals.
pub fn command_for(text: &str, m: Modifiers) -> Option<Command> {
    let is = |key: Key| text == SharedString::from(key).as_str();
    let modifiers = TermModifiers { shift: m.shift, alt: m.alt, ctrl: m.ctrl };
    let ctrl = |c: char| Some(Command::TerminalKey(TerminalKey::Char(c), TermModifiers { ctrl: true, ..Default::default() }));
    if m.cmd {
        return if is(Key::LeftArrow) {
            ctrl('a')
        } else if is(Key::RightArrow) {
            ctrl('e')
        } else if is(Key::Backspace) {
            ctrl('u')
        } else {
            None
        };
    }
    let named = [
        (Key::Return, TerminalKey::Enter),
        (Key::Backspace, TerminalKey::Backspace),
        (Key::Tab, TerminalKey::Tab),
        (Key::Backtab, TerminalKey::Tab),
        (Key::Escape, TerminalKey::Escape),
        (Key::UpArrow, TerminalKey::Up),
        (Key::DownArrow, TerminalKey::Down),
        (Key::LeftArrow, TerminalKey::Left),
        (Key::RightArrow, TerminalKey::Right),
        (Key::Home, TerminalKey::Home),
        (Key::End, TerminalKey::End),
        (Key::PageUp, TerminalKey::PageUp),
        (Key::PageDown, TerminalKey::PageDown),
        (Key::Insert, TerminalKey::Insert),
        (Key::Delete, TerminalKey::Delete),
        (Key::F1, TerminalKey::F(1)),
        (Key::F2, TerminalKey::F(2)),
        (Key::F3, TerminalKey::F(3)),
        (Key::F4, TerminalKey::F(4)),
        (Key::F5, TerminalKey::F(5)),
        (Key::F6, TerminalKey::F(6)),
        (Key::F7, TerminalKey::F(7)),
        (Key::F8, TerminalKey::F(8)),
        (Key::F9, TerminalKey::F(9)),
        (Key::F10, TerminalKey::F(10)),
        (Key::F11, TerminalKey::F(11)),
        (Key::F12, TerminalKey::F(12)),
    ];
    if let Some((key, terminal_key)) = named.into_iter().find(|(key, _)| is(key.clone())) {
        let shift = m.shift || key == Key::Backtab;
        return Some(Command::TerminalKey(terminal_key, TermModifiers { shift, ..modifiers }));
    }
    let mut chars = text.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return typed(text);
    };
    if m.ctrl {
        // With ⌃ held, the key's text may be the letter or its control
        // character.
        let c = if c.is_ascii_control() && (c as u32) < 27 { char::from(b'a' + c as u8 - 1) } else { c };
        return Some(Command::TerminalKey(TerminalKey::Char(c), modifiers));
    }
    // ⌥ with a character types what macOS composed (⌥E is a dead key), so
    // it is text; Slint's private-use codes are keys without a mapping.
    typed(text)
}

/// Text to type, if it is printable.
fn typed(text: &str) -> Option<Command> {
    let printable =
        !text.is_empty() && text.chars().all(|c| !c.is_control() && !('\u{E000}'..='\u{F8FF}').contains(&c));
    printable.then(|| Command::TerminalText(text.to_owned()))
}
