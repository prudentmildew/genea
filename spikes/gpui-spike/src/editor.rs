//! PROTOTYPE. A minimal editor surface: ropey buffer, tree-sitter highlighting
//! of the visible lines, one cursor, pixel scrolling, a blinking caret.

use std::{
    ops::Range,
    path::Path,
    time::{Duration, Instant},
};

use gpui::{
    App, Bounds, Context, ElementId, ElementInputHandler, Entity, EntityInputHandler, FocusHandle,
    Focusable, GlobalElementId, Hsla, LayoutId, Pixels, Point, ScrollWheelEvent, ShapedLine,
    SharedString, Style, Task, TextRun, UTF16Selection, Window, actions, div, fill, font, point,
    prelude::*, px, relative, rgb, size,
};
use ropey::Rope;
use tree_sitter::InputEdit;

use crate::syntax::{self, Kind, Syntax};

actions!(spike, [Backspace, Newline, Left, Right, Up, Down, PageUp, PageDown, Quit]);

/// Files above this size open without syntax highlighting.
pub const HIGHLIGHT_LIMIT: usize = 8 * 1024 * 1024;
const FONT_SIZE: f32 = 13.0;
const LINE_HEIGHT: f32 = 20.0;
const GUTTER: f32 = 64.0;
/// Longest line prefix shaped per row; the rest is clipped off-screen anyway.
const MAX_SHAPED_BYTES: usize = 1000;

pub struct Editor {
    pub focus_handle: FocusHandle,
    pub rope: Rope,
    pub syntax: Option<Syntax>,
    pub loading: bool,
    cursor: usize,
    pub scroll_top: Pixels,
    viewport: Size,
    cursor_visible: bool,
    last_input: Instant,
    last_cursor_bounds: Option<Bounds<Pixels>>,
    _blink: Task<()>,
    _parse: Option<Task<()>>,
    /// (start, end of edit, end of reparse) per edit, for the typing bench.
    pub edit_log: Vec<(Instant, Instant, Instant)>,
    /// Benchmark auto-scroll: pixels per frame, until when.
    pub auto_scroll: Option<(f32, Instant)>,
    /// Bumped whenever new text is shown, so benches can wait for it.
    pub generation: u64,
}

#[derive(Default, Clone, Copy)]
struct Size {
    height: Pixels,
}

impl Editor {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            rope: Rope::new(),
            syntax: None,
            loading: false,
            cursor: 0,
            scroll_top: px(0.),
            viewport: Size::default(),
            cursor_visible: true,
            last_input: Instant::now(),
            last_cursor_bounds: None,
            _blink: Self::blink(cx),
            _parse: None,
            edit_log: Vec::new(),
            auto_scroll: None,
            generation: 0,
        }
    }

    /// Reads the file on the main thread (meant for files up to a few MB),
    /// shows it immediately and parses it in the background.
    pub fn open(&mut self, path: &Path, cx: &mut Context<Self>) {
        let text = std::fs::read_to_string(path).unwrap();
        self.set_text(Rope::from_str(&text), cx);
    }

    /// Reads and builds the rope off the main thread (the 100 MB case).
    pub fn open_in_background(&mut self, path: &Path, cx: &mut Context<Self>) {
        let path = path.to_owned();
        self.loading = true;
        cx.notify();
        self._parse = Some(cx.spawn(async move |this, cx| {
            let rope = cx
                .background_spawn(async move {
                    let file = std::io::BufReader::new(std::fs::File::open(path).unwrap());
                    Rope::from_reader(file).unwrap()
                })
                .await;
            this.update(cx, |this, cx| {
                this.loading = false;
                this.set_text(rope, cx);
            })
            .ok();
        }));
    }

    fn set_text(&mut self, rope: Rope, cx: &mut Context<Self>) {
        self.rope = rope;
        self.syntax = None;
        self.cursor = 0;
        self.scroll_top = px(0.);
        self.generation += 1;
        cx.notify();
        if self.rope.len_bytes() > HIGHLIGHT_LIMIT {
            self._parse = None;
            return;
        }
        let snapshot = self.rope.clone();
        let generation = self.generation;
        self._parse = Some(cx.spawn(async move |this, cx| {
            let syntax = cx.background_spawn(async move { Syntax::new(&snapshot) }).await;
            this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.syntax = Some(syntax);
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// The caret blink is the only timer: 2 wake-ups a second while focused.
    fn blink(cx: &mut Context<Self>) -> Task<()> {
        if std::env::var_os("SPIKE_NO_BLINK").is_some() {
            return Task::ready(());
        }
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(530)).await;
                let alive = this.update(cx, |this, cx| {
                    if this.last_input.elapsed() > Duration::from_millis(500) {
                        this.cursor_visible = !this.cursor_visible;
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
    }

    pub fn line_height() -> Pixels {
        px(LINE_HEIGHT)
    }

    pub fn max_scroll(&self) -> Pixels {
        (Self::line_height() * self.rope.len_lines() as f32 - self.viewport.height).max(px(0.))
    }

    pub fn scroll_by(&mut self, delta: Pixels, cx: &mut Context<Self>) {
        self.scroll_top = (self.scroll_top + delta).clamp(px(0.), self.max_scroll());
        cx.notify();
    }

    pub fn move_cursor_to_line(&mut self, line: usize, cx: &mut Context<Self>) {
        let line = line.min(self.rope.len_lines().saturating_sub(1));
        let char_idx = self.rope.line_to_char(line);
        let len = self.line_len_chars(line);
        self.cursor = char_idx + len / 2;
        self.reveal_cursor();
        cx.notify();
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
        let lh = Self::line_height();
        let y = lh * self.rope.char_to_line(self.cursor) as f32;
        if y < self.scroll_top {
            self.scroll_top = y;
        } else if y + lh > self.scroll_top + self.viewport.height {
            self.scroll_top = y + lh - self.viewport.height;
        }
    }

    /// Replaces `range` (chars) with `text`, then reparses incrementally.
    fn edit(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
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
        self.edit_log.push((started, edited, Instant::now()));
        self.touch();
        self.reveal_cursor();
        cx.notify();
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.cursor > 0 {
            self.edit(self.cursor - 1..self.cursor, "", cx);
        }
    }

    fn newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(self.cursor..self.cursor, "\n", cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        self.cursor = self.cursor.saturating_sub(1);
        self.after_move(cx);
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        self.cursor = (self.cursor + 1).min(self.rope.len_chars());
        self.after_move(cx);
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertically(-1, cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertically(1, cx);
    }

    fn page_up(&mut self, _: &PageUp, _: &mut Window, cx: &mut Context<Self>) {
        let rows = (self.viewport.height / Self::line_height()) as isize;
        self.move_vertically(-rows, cx);
    }

    fn page_down(&mut self, _: &PageDown, _: &mut Window, cx: &mut Context<Self>) {
        let rows = (self.viewport.height / Self::line_height()) as isize;
        self.move_vertically(rows, cx);
    }

    fn move_vertically(&mut self, rows: isize, cx: &mut Context<Self>) {
        let line = self.rope.char_to_line(self.cursor);
        let column = self.cursor - self.rope.line_to_char(line);
        let target = (line as isize + rows).clamp(0, self.rope.len_lines() as isize - 1) as usize;
        self.cursor = self.rope.line_to_char(target) + column.min(self.line_len_chars(target));
        self.after_move(cx);
    }

    fn after_move(&mut self, cx: &mut Context<Self>) {
        self.touch();
        self.reveal_cursor();
        cx.notify();
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(Self::line_height());
        self.scroll_by(-delta.y, cx);
    }

    fn utf16_to_char(&self, offset: usize) -> usize {
        self.rope
            .utf16_cu_to_char(offset.min(self.rope.len_utf16_cu()))
    }

    fn char_to_utf16(&self, offset: usize) -> usize {
        self.rope.char_to_utf16_cu(offset)
    }
}

impl EntityInputHandler for Editor {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.utf16_to_char(range.start)..self.utf16_to_char(range.end);
        actual.replace(self.char_to_utf16(range.start)..self.char_to_utf16(range.end));
        Some(self.rope.slice(range).to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let cursor = self.char_to_utf16(self.cursor);
        Some(UTF16Selection {
            range: cursor..cursor,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|r| self.utf16_to_char(r.start)..self.utf16_to_char(r.end))
            .unwrap_or(self.cursor..self.cursor);
        self.edit(range, text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace_text_in_range(range, text, window, cx);
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.last_cursor_bounds
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Focusable for Editor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some((step, until)) = self.auto_scroll {
            if Instant::now() < until {
                self.scroll_top = (self.scroll_top + px(step)).min(self.max_scroll());
                window.request_animation_frame();
            } else {
                self.auto_scroll = None;
            }
        }
        div()
            .key_context("Editor")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::page_up))
            .on_action(cx.listener(Self::page_down))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .size_full()
            .bg(rgb(0xfafafa))
            .child(EditorElement {
                editor: cx.entity(),
            })
    }
}

struct EditorElement {
    editor: Entity<Editor>,
}

impl IntoElement for EditorElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

struct Row {
    y: Pixels,
    number: ShapedLine,
    text: ShapedLine,
}

struct Prepaint {
    rows: Vec<Row>,
    cursor: Option<Bounds<Pixels>>,
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        self.editor.update(cx, |editor, _| {
            editor.viewport.height = bounds.size.height;
            editor.scroll_top = editor.scroll_top.min(editor.max_scroll());
        });
        let editor = self.editor.read(cx);
        let rope = &editor.rope;
        let lh = Editor::line_height();
        let font_size = px(FONT_SIZE);
        let mono = font("Menlo");
        let run = |len: usize, color: Hsla| TextRun {
            len,
            font: mono.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };

        let first = (editor.scroll_top / lh).floor() as usize;
        let visible = (bounds.size.height / lh).ceil() as usize + 1;
        let last = (first + visible).min(rope.len_lines());
        let first = first.min(last);
        let byte_start = rope.line_to_byte(first);
        let kinds = editor
            .syntax
            .as_ref()
            .map(|s| s.kinds(rope, byte_start..rope.line_to_byte(last)));

        let text_system = window.text_system().clone();
        let mut rows = Vec::with_capacity(last - first);
        let mut cursor = None;
        let cursor_line = rope.char_to_line(editor.cursor);
        for line in first..last {
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
            let line_byte = rope.line_to_byte(line) - byte_start;
            let mut runs = Vec::new();
            let mut i = 0;
            while i < text.len() {
                let kind = kinds.as_ref().and_then(|k| k[line_byte + i]);
                let mut j = i + 1;
                while j < text.len() && kinds.as_ref().and_then(|k| k[line_byte + j]) == kind {
                    j += 1;
                }
                runs.push(run(j - i, color(kind)));
                i = j;
            }
            let y = bounds.top() + lh * line as f32 - editor.scroll_top;
            let shaped = text_system.shape_line(SharedString::from(text), font_size, &runs, None);
            if line == cursor_line && editor.cursor_visible && !editor.loading {
                let column = rope.char_to_byte(editor.cursor) - rope.line_to_byte(line);
                let x = shaped.x_for_index(column.min(shaped.len()));
                cursor = Some(Bounds::new(
                    point(bounds.left() + px(GUTTER) + x, y),
                    size(px(2.), lh),
                ));
            }
            let number = (line + 1).to_string();
            let number_runs = [run(number.len(), rgb(0x9a9a9a).into())];
            rows.push(Row {
                y,
                number: text_system.shape_line(number.into(), font_size, &number_runs, None),
                text: shaped,
            });
        }
        Prepaint { rows, cursor }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.editor.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.editor.clone()),
            cx,
        );
        let lh = Editor::line_height();
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for row in &prepaint.rows {
                let number_x = bounds.left() + px(GUTTER - 12.) - row.number.width;
                row.number
                    .paint(point(number_x, row.y), lh, gpui::TextAlign::Left, None, window, cx)
                    .ok();
                row.text
                    .paint(
                        point(bounds.left() + px(GUTTER), row.y),
                        lh,
                        gpui::TextAlign::Left,
                        None,
                        window,
                        cx,
                    )
                    .ok();
            }
            if focus_handle.is_focused(window)
                && let Some(cursor) = prepaint.cursor
            {
                window.paint_quad(fill(cursor, rgb(0x1f6feb)));
            }
        });
        let cursor = prepaint.cursor;
        self.editor.update(cx, |editor, _| editor.last_cursor_bounds = cursor);
    }
}

fn color(kind: Option<Kind>) -> Hsla {
    rgb(match kind {
        None => 0x24292f,
        Some(Kind::Keyword) => 0xcf222e,
        Some(Kind::String) => 0x0a3069,
        Some(Kind::Comment) => 0x6e7781,
        Some(Kind::Function) => 0x8250df,
        Some(Kind::Type) => 0x953800,
        Some(Kind::Number) | Some(Kind::Constant) => 0x0550ae,
        Some(Kind::Property) => 0x116329,
        Some(Kind::Operator) | Some(Kind::Punctuation) => 0x57606a,
    })
    .into()
}
