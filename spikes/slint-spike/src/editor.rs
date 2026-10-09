//! PROTOTYPE. A minimal editor surface: ropey buffer, tree-sitter highlighting
//! of the visible lines, one cursor, pixel scrolling, a blinking caret. The
//! view is `ui/editor.slint`: one Slint `Text` per highlight run on a
//! monospace grid, lines kept in a ring of slots.
//!
//! IME spike (#17): commits arrive through `commit`, the live composition
//! through `set_preedit`. The preedit is never in the rope; `sync` splices it
//! into the caret line's runs and the caret moves to its end.

use std::{
    ops::Range,
    path::Path,
    rc::Rc,
    time::{Duration, Instant},
};

use ropey::Rope;
use slint::{Color, Model, ModelRc, SharedString, VecModel, platform::Key};
use tree_sitter::InputEdit;
use unicode_width::UnicodeWidthChar;

use crate::{
    EditorWindow, Line, Run, app,
    syntax::{self, Kind, Syntax},
};

/// Files above this size open without syntax highlighting.
pub const HIGHLIGHT_LIMIT: usize = 8 * 1024 * 1024;
pub const LINE_HEIGHT: f32 = 20.0;
/// Longest line prefix laid out per row; the rest is clipped off-screen anyway.
const MAX_SHAPED_BYTES: usize = 1000;
/// Line y positions are relative to a base line, so f32 stays precise in a
/// 100 MB file. The base moves (and every slot is rebuilt) past this distance.
const REBASE_LINES: usize = 10_000;

#[derive(PartialEq)]
struct SlotState {
    line: usize,
    base: usize,
    runs: Vec<(usize, String, u32)>,
}

pub struct Editor {
    pub rope: Rope,
    pub syntax: Option<Syntax>,
    pub loading: bool,
    cursor: usize,
    pub scroll_top: f32,
    viewport_height: f32,
    char_width: f32,
    cursor_visible: bool,
    last_input: Instant,
    /// (start, end of edit, end of reparse, end of view sync) per edit.
    pub edit_log: Vec<(Instant, Instant, Instant, Instant)>,
    /// Benchmark auto-scroll: pixels per frame, until when.
    pub auto_scroll: Option<(f32, Instant)>,
    /// How long each auto-scroll step's view sync took.
    pub scroll_sync_log: Vec<Duration>,
    /// Bumped whenever new text is shown, so benches can wait for it.
    pub generation: u64,
    /// When the view was last synced after new text was set.
    pub text_set_at: Option<Instant>,
    /// The IME composition (marked text), shown at the cursor, not in the rope.
    pub preedit: String,
    /// The last few IME events, for the status line.
    ime_log: Vec<String>,
    lines: Rc<VecModel<Line>>,
    slots: Vec<Option<SlotState>>,
    base: usize,
}

impl Editor {
    pub fn new(window: &EditorWindow) -> Self {
        let lines = Rc::new(VecModel::default());
        window.set_lines(ModelRc::from(lines.clone()));
        Self {
            rope: Rope::new(),
            syntax: None,
            loading: false,
            cursor: 0,
            scroll_top: 0.,
            viewport_height: 800.,
            char_width: 7.8,
            cursor_visible: true,
            last_input: Instant::now(),
            edit_log: Vec::new(),
            auto_scroll: None,
            scroll_sync_log: Vec::new(),
            generation: 0,
            text_set_at: None,
            preedit: String::new(),
            ime_log: Vec::new(),
            lines,
            slots: Vec::new(),
            base: 0,
        }
    }

    /// Reads the file on the main thread (meant for files up to a few MB),
    /// shows it immediately and parses it in the background.
    pub fn open(&mut self, path: &Path, window: &EditorWindow) {
        let text = std::fs::read_to_string(path).unwrap();
        self.set_text(Rope::from_str(&text), window);
    }

    /// Reads and builds the rope off the main thread (the 100 MB case).
    pub fn open_in_background(&mut self, path: &Path, window: &EditorWindow) {
        let path = path.to_owned();
        self.loading = true;
        self.sync(window);
        std::thread::spawn(move || {
            let file = std::io::BufReader::new(std::fs::File::open(path).unwrap());
            let rope = Rope::from_reader(file).unwrap();
            slint::invoke_from_event_loop(move || {
                app::with(|editor, window| {
                    editor.loading = false;
                    editor.set_text(rope, window);
                })
            })
            .unwrap();
        });
    }

    fn set_text(&mut self, rope: Rope, window: &EditorWindow) {
        self.rope = rope;
        self.syntax = None;
        self.cursor = 0;
        self.scroll_top = 0.;
        self.generation += 1;
        self.sync(window);
        self.text_set_at = Some(Instant::now());
        if self.rope.len_bytes() > HIGHLIGHT_LIMIT {
            return;
        }
        let snapshot = self.rope.clone();
        let generation = self.generation;
        std::thread::spawn(move || {
            let syntax = Syntax::new(&snapshot);
            slint::invoke_from_event_loop(move || {
                app::with(|editor, window| {
                    if editor.generation == generation {
                        editor.syntax = Some(syntax);
                        editor.sync(window);
                    }
                })
            })
            .unwrap();
        });
    }

    /// The caret blink: 2 wake-ups a second while the window is up.
    pub fn blink(&mut self, window: &EditorWindow) {
        if self.last_input.elapsed() > Duration::from_millis(500) {
            self.cursor_visible = !self.cursor_visible;
            window.set_caret_visible(self.cursor_visible && !self.loading);
        }
    }

    pub fn max_scroll(&self) -> f32 {
        (LINE_HEIGHT * self.rope.len_lines() as f32 - self.viewport_height).max(0.)
    }

    pub fn scroll_by(&mut self, delta: f32, window: &EditorWindow) {
        self.scroll_top = (self.scroll_top + delta).clamp(0., self.max_scroll());
        self.sync(window);
    }

    /// One benchmark auto-scroll step; called once per rendered frame.
    pub fn auto_scroll_step(&mut self, window: &EditorWindow) {
        let Some((step, until)) = self.auto_scroll else { return };
        if Instant::now() < until {
            let t = Instant::now();
            self.scroll_top = (self.scroll_top + step).min(self.max_scroll());
            self.sync(window);
            self.scroll_sync_log.push(t.elapsed());
        } else {
            self.auto_scroll = None;
        }
    }

    pub fn move_cursor_to_line(&mut self, line: usize, window: &EditorWindow) {
        let line = line.min(self.rope.len_lines().saturating_sub(1));
        let char_idx = self.rope.line_to_char(line);
        let len = self.line_len_chars(line);
        self.cursor = char_idx + len / 2;
        self.reveal_cursor();
        self.sync(window);
    }

    fn line_len_chars(&self, line: usize) -> usize {
        let slice = self.rope.line(line);
        let mut len = slice.len_chars();
        while len > 0 && matches!(slice.char(len - 1), '\n' | '\r') {
            len -= 1;
        }
        len
    }

    fn touch(&mut self) {
        self.last_input = Instant::now();
        self.cursor_visible = true;
    }

    fn reveal_cursor(&mut self) {
        let y = LINE_HEIGHT * self.rope.char_to_line(self.cursor) as f32;
        if y < self.scroll_top {
            self.scroll_top = y;
        } else if y + LINE_HEIGHT > self.scroll_top + self.viewport_height {
            self.scroll_top = y + LINE_HEIGHT - self.viewport_height;
        }
    }

    /// Replaces `range` (chars) with `text`, reparses incrementally, then
    /// pushes the visible lines to Slint.
    fn edit(&mut self, range: Range<usize>, text: &str, window: &EditorWindow) {
        let started = Instant::now();
        let start_byte = self.rope.char_to_byte(range.start);
        let old_end_byte = self.rope.char_to_byte(range.end);
        let start_position = syntax::point(&self.rope, start_byte);
        let old_end_position = syntax::point(&self.rope, old_end_byte);
        self.rope.remove(range.clone());
        self.rope.insert(range.start, text);
        self.cursor = range.start + text.chars().count();
        let edited = Instant::now();
        if let Some(syntax) = &mut self.syntax {
            let new_end_byte = start_byte + text.len();
            syntax.edit(
                &self.rope,
                &InputEdit {
                    start_byte,
                    old_end_byte,
                    new_end_byte,
                    start_position,
                    old_end_position,
                    new_end_position: syntax::point(&self.rope, new_end_byte),
                },
            );
        }
        let reparsed = Instant::now();
        self.touch();
        self.reveal_cursor();
        self.sync(window);
        self.edit_log.push((started, edited, reparsed, Instant::now()));
    }

    /// Text committed by the input method (also plain typing while the IME
    /// is enabled: winit routes every inserted character through it).
    pub fn commit(&mut self, text: &str, window: &EditorWindow) {
        self.log_ime(format!("commit {text:?}"));
        self.preedit.clear();
        self.edit(self.cursor..self.cursor, text, window);
    }

    pub fn set_preedit(&mut self, text: &str, window: &EditorWindow) {
        if text == self.preedit {
            return;
        }
        self.log_ime(format!("preedit {text:?}"));
        self.preedit = text.to_owned();
        self.touch();
        self.sync(window);
    }

    fn log_ime(&mut self, entry: String) {
        if std::env::var_os("SPIKE_IME_LOG").is_some() {
            eprintln!("ime: {entry}");
        }
        self.ime_log.push(entry);
        if self.ime_log.len() > 4 {
            self.ime_log.remove(0);
        }
    }

    fn move_vertically(&mut self, rows: isize, window: &EditorWindow) {
        let line = self.rope.char_to_line(self.cursor);
        let column = self.cursor - self.rope.line_to_char(line);
        let target = (line as isize + rows).clamp(0, self.rope.len_lines() as isize - 1) as usize;
        self.cursor = self.rope.line_to_char(target) + column.min(self.line_len_chars(target));
        self.after_move(window);
    }

    fn after_move(&mut self, window: &EditorWindow) {
        self.touch();
        self.reveal_cursor();
        self.sync(window);
    }

    pub fn key(&mut self, event: &slint::private_unstable_api::re_exports::KeyEvent, window: &EditorWindow) -> bool {
        let text = event.text.as_str();
        let is = |key: Key| text == SharedString::from(key).as_str();
        let rows = (self.viewport_height / LINE_HEIGHT) as isize;
        self.log_ime(format!("key {:?}{}", text, if event.modifiers.meta { " +cmd" } else { "" }));
        if event.modifiers.control || event.modifiers.meta {
            if text == "q" {
                slint::quit_event_loop().ok();
                return true;
            }
            return false;
        }
        if is(Key::Backspace) {
            if self.cursor > 0 {
                self.edit(self.cursor - 1..self.cursor, "", window);
            }
        } else if is(Key::Return) {
            self.edit(self.cursor..self.cursor, "\n", window);
        } else if is(Key::LeftArrow) {
            self.cursor = self.cursor.saturating_sub(1);
            self.after_move(window);
        } else if is(Key::RightArrow) {
            self.cursor = (self.cursor + 1).min(self.rope.len_chars());
            self.after_move(window);
        } else if is(Key::UpArrow) {
            self.move_vertically(-1, window);
        } else if is(Key::DownArrow) {
            self.move_vertically(1, window);
        } else if is(Key::PageUp) {
            self.move_vertically(-rows, window);
        } else if is(Key::PageDown) {
            self.move_vertically(rows, window);
        } else if !text.is_empty()
            && text.chars().all(|c| !c.is_control() && !('\u{f700}'..='\u{f8ff}').contains(&c))
        {
            self.edit(self.cursor..self.cursor, text, window);
        } else {
            return false;
        }
        true
    }

    /// Pushes the visible lines, the scroll offset and the caret to Slint.
    /// Only slots whose line or content changed are replaced.
    pub fn sync(&mut self, window: &EditorWindow) {
        let vh = window.get_viewport_height();
        if vh > 0. {
            self.viewport_height = vh;
        }
        let cw = window.get_char_width();
        if cw > 0. && cw != self.char_width {
            self.char_width = cw;
            self.slots.iter_mut().for_each(|s| *s = None);
        }
        self.scroll_top = self.scroll_top.clamp(0., self.max_scroll());

        let n = (self.viewport_height / LINE_HEIGHT).ceil() as usize + 2;
        if self.lines.row_count() != n {
            self.lines.set_vec(vec![Line::default(); n]);
            self.slots = (0..n).map(|_| None).collect();
        }
        let rope = &self.rope;
        let len_lines = if self.loading { 0 } else { rope.len_lines() };
        let first = ((self.scroll_top / LINE_HEIGHT).floor() as usize).min(len_lines);
        if first < self.base || first - self.base > REBASE_LINES {
            self.base = first;
        }
        let base = self.base;
        let last = (first + n).min(len_lines);
        let byte_start = rope.line_to_byte(first);
        let kinds = self
            .syntax
            .as_ref()
            .filter(|_| last > first)
            .map(|s| s.kinds(rope, byte_start..rope.line_to_byte(last)));

        let cursor_line = rope.char_to_line(self.cursor);
        let column = columns(rope.slice(rope.line_to_char(cursor_line)..self.cursor).chars());
        let preedit_width = columns(self.preedit.chars());
        for slot in 0..n {
            let line = first + (slot + n - first % n) % n;
            let state = (line < last).then(|| {
                let mut runs = runs(rope, line, rope.line_to_byte(line) - byte_start, kinds.as_deref());
                if line == cursor_line && !self.preedit.is_empty() {
                    splice_preedit(&mut runs, column, &self.preedit, preedit_width);
                }
                SlotState { line, base, runs }
            });
            if self.slots[slot] == state {
                continue;
            }
            let row = match &state {
                None => Line { y: -1000., ..Default::default() },
                Some(s) => Line {
                    y: (s.line - base) as f32 * LINE_HEIGHT,
                    number: (s.line + 1).to_string().into(),
                    runs: ModelRc::new(VecModel::from(
                        s.runs
                            .iter()
                            .map(|(col, text, rgb)| Run {
                                x: *col as f32 * self.char_width,
                                text: text.as_str().into(),
                                color: Color::from_argb_encoded(0xff00_0000 | rgb),
                            })
                            .collect::<Vec<_>>(),
                    )),
                },
            };
            self.lines.set_row_data(slot, row);
            self.slots[slot] = state;
        }

        window.set_offset_y(-(self.scroll_top - base as f32 * LINE_HEIGHT));
        window.set_compose_x(column as f32 * self.char_width);
        window.set_preedit_width(preedit_width as f32 * self.char_width);
        window.set_caret_x((column + preedit_width) as f32 * self.char_width);
        window.set_status(
            format!(
                "line {} col {}  |  preedit {:?}  |  {}",
                cursor_line + 1,
                column + 1,
                self.preedit,
                self.ime_log.join("  ·  ")
            )
            .into(),
        );
        window.set_caret_y((cursor_line as f32 - base as f32) * LINE_HEIGHT);
        window.set_caret_visible(self.cursor_visible && !self.loading);
    }
}

/// Grid columns: tabs are 4, East Asian wide characters and emoji are 2.
fn columns(chars: impl Iterator<Item = char>) -> usize {
    chars
        .map(|c| if c == '\t' { 4 } else { c.width().unwrap_or(0) })
        .sum()
}

const PREEDIT_COLOR: u32 = 0x1f6feb;

/// Inserts the preedit into a line's runs at `at` (a column), splitting the run
/// it falls in and shifting everything after it right by `width` columns.
fn splice_preedit(runs: &mut Vec<(usize, String, u32)>, at: usize, preedit: &str, width: usize) {
    let mut out = Vec::with_capacity(runs.len() + 2);
    for (col, text, rgb) in runs.drain(..) {
        let end = col + columns(text.chars());
        if end <= at {
            out.push((col, text, rgb));
        } else if col >= at {
            out.push((col + width, text, rgb));
        } else {
            let mut c = col;
            let split = text
                .char_indices()
                .find(|(_, ch)| {
                    let hit = c >= at;
                    c += columns(std::iter::once(*ch));
                    hit
                })
                .map_or(text.len(), |(i, _)| i);
            let (head, tail) = text.split_at(split);
            let head_end = col + columns(head.chars());
            out.push((col, head.to_owned(), rgb));
            out.push((head_end + width, tail.to_owned(), rgb));
        }
    }
    out.push((at, preedit.to_owned(), PREEDIT_COLOR));
    *runs = out;
}

/// Highlight runs of one line as (column, text, rgb). Whitespace-only runs
/// are dropped: they draw nothing.
fn runs(rope: &Rope, line: usize, line_byte: usize, kinds: Option<&[Option<Kind>]>) -> Vec<(usize, String, u32)> {
    let mut text = rope.line(line).to_string();
    while text.ends_with(['\n', '\r']) {
        text.pop();
    }
    if text.len() > MAX_SHAPED_BYTES {
        let mut cut = MAX_SHAPED_BYTES;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
    }
    let kind_at = |i: usize| kinds.and_then(|k| k.get(line_byte + i).copied().flatten());
    let mut out = Vec::new();
    let mut col = 0;
    let mut run = String::new();
    let mut run_col = 0;
    let mut run_kind = None;
    for (i, c) in text.char_indices() {
        let kind = kind_at(i);
        if kind != run_kind && !run.is_empty() {
            if !run.trim().is_empty() {
                out.push((run_col, std::mem::take(&mut run), color(run_kind)));
            }
            run.clear();
        }
        if run.is_empty() {
            run_col = col;
            run_kind = kind;
        }
        if c == '\t' {
            run.push_str("    ");
        } else {
            run.push(c);
        }
        col += columns(std::iter::once(c));
    }
    if !run.trim().is_empty() {
        out.push((run_col, run, color(run_kind)));
    }
    out
}

fn color(kind: Option<Kind>) -> u32 {
    match kind {
        None => 0x24292f,
        Some(Kind::Keyword) => 0xcf222e,
        Some(Kind::String) => 0x0a3069,
        Some(Kind::Comment) => 0x6e7781,
        Some(Kind::Function) => 0x8250df,
        Some(Kind::Type) => 0x953800,
        Some(Kind::Number) | Some(Kind::Constant) => 0x0550ae,
        Some(Kind::Property) => 0x116329,
        Some(Kind::Operator) | Some(Kind::Punctuation) => 0x57606a,
    }
}
