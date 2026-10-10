//! Completion, hover and signature help in the view (ticket #43): the
//! popups of `ui/assist-popups.slint`, the keys they take while open, and
//! the pointer resting over text for hover.
//!
//! Repaint rules as for the surface: popups are pushed only when what they
//! show (or where) changed, and the hover delay is a single-shot timer that
//! only runs after the pointer moved.

use std::time::Duration;

use genea_core::{Command, CompletionKind, EditorView, MarkupBlock};
use slint::{ModelRc, SharedString, TimerMode, VecModel, platform::Key};

use crate::{
    AssistPopups, CompletionRow, DocBlock, ProjectWindow,
    app::with_window,
    keys::Modifiers,
    window::WindowKey,
};

/// How long the pointer rests on code before its hover is asked for.
const HOVER_DELAY: Duration = Duration::from_millis(500);

/// What a pane's popups show and where, as last pushed.
#[derive(Clone, Default, PartialEq)]
pub struct Popups {
    shown: Option<(Option<genea_core::CompletionView>, Option<genea_core::HoverView>, Option<genea_core::SignatureHelpView>)>,
    /// The tops of the anchor lines and the cell width they were placed with.
    placed: Vec<f32>,
}

impl Popups {
    /// Whether the completion list is open: ↑, ↓, Return and Tab go to it.
    pub fn completion_open(&self) -> bool {
        self.shown.as_ref().is_some_and(|(c, _, _)| c.is_some())
    }

    /// Whether hover or signature help is open: Esc closes it.
    fn other_open(&self) -> (bool, bool) {
        self.shown.as_ref().map_or((false, false), |(_, h, s)| (h.is_some(), s.is_some()))
    }

    /// Pushes the focused editor's popups to a pane, if they changed.
    /// `line_top` is where a file line's row starts, relative to the
    /// surface's base line.
    pub fn sync(
        &mut self,
        window: &ProjectWindow,
        pane: usize,
        editor: Option<&EditorView>,
        line_top: &dyn Fn(usize) -> f32,
        line_height: f32,
    ) {
        let char_width = window.get_char_width();
        let shown = editor.map(|e| (e.completion.clone(), e.hover.clone(), e.signature_help.clone()));
        let (completion, hover, signature) = shown.clone().unwrap_or_default();
        let anchors = [
            completion.as_ref().map(|c| c.at),
            hover.as_ref().map(|h| h.at),
            signature.as_ref().map(|s| s.at),
        ];
        let mut placed: Vec<f32> = anchors.iter().flatten().map(|at| line_top(at.line)).collect();
        placed.push(char_width);
        let wanted = Popups { shown: shown.filter(|(c, h, s)| c.is_some() || h.is_some() || s.is_some()), placed };
        if wanted == *self {
            return;
        }
        let mut popups = AssistPopups::default();
        if let Some(completion) = &completion {
            let top = line_top(completion.at.line);
            popups.completion_shown = true;
            popups.completion_x = completion.at.column as f32 * char_width;
            popups.completion_line_top = top;
            popups.completion_line_bottom = top + line_height;
            let rows: Vec<CompletionRow> = completion
                .items
                .iter()
                .map(|item| CompletionRow {
                    kind: kind_letter(item.kind).into(),
                    label: item.label.as_str().into(),
                    detail: item.detail.as_deref().unwrap_or_default().into(),
                    source: item.source.as_deref().unwrap_or_default().into(),
                })
                .collect();
            popups.completion_rows = ModelRc::new(VecModel::from(rows));
            popups.completion_selected = completion.selected as i32;
            popups.completion_detail = completion.detail.as_deref().unwrap_or_default().into();
            popups.completion_docs = docs(&completion.documentation);
        }
        if let Some(hover) = &hover {
            let top = line_top(hover.at.line);
            popups.hover_shown = true;
            popups.hover_x = hover.at.column as f32 * char_width;
            popups.hover_line_top = top;
            popups.hover_line_bottom = top + line_height;
            popups.hover_docs = docs(&hover.contents);
        }
        if let Some(signature) = &signature {
            let top = line_top(signature.at.line);
            let chars: Vec<char> = signature.label.chars().collect();
            let active = signature.active_parameter.clone().unwrap_or(0..0);
            let (start, end) = (active.start.min(chars.len()), active.end.min(chars.len()));
            popups.signature_shown = true;
            popups.signature_x = signature.at.column as f32 * char_width;
            popups.signature_line_top = top;
            popups.signature_line_bottom = top + line_height;
            popups.signature_before = chars[..start].iter().collect::<String>().into();
            popups.signature_active = chars[start..end.max(start)].iter().collect::<String>().into();
            popups.signature_after = chars[end.max(start)..].iter().collect::<String>().into();
            popups.signature_count = if signature.signatures > 1 {
                format!("{}/{}", signature.signature + 1, signature.signatures).into()
            } else {
                SharedString::new()
            };
            popups.signature_docs = docs(&signature.documentation);
        }
        if pane == 0 {
            window.set_left_assist(popups);
        } else {
            window.set_right_assist(popups);
        }
        *self = wanted;
    }

    /// The command for a key while a popup is open, if it takes the key.
    pub fn command_for(&self, text: &str, m: Modifiers) -> Option<Command> {
        let is = |key: Key| text == SharedString::from(key).as_str();
        if m.cmd || m.alt || m.ctrl || m.shift {
            return None;
        }
        if self.completion_open() {
            if is(Key::UpArrow) {
                return Some(Command::MoveCompletionSelection(-1));
            }
            if is(Key::DownArrow) {
                return Some(Command::MoveCompletionSelection(1));
            }
            if is(Key::Return) || is(Key::Tab) {
                return Some(Command::AcceptCompletion);
            }
            if is(Key::Escape) {
                return Some(Command::CloseCompletion);
            }
        }
        let (hover, signature) = self.other_open();
        if is(Key::Escape) && signature {
            return Some(Command::HideSignatureHelp);
        }
        if is(Key::Escape) && hover {
            return Some(Command::HideHover);
        }
        None
    }
}

/// A completion kind's letter in the list.
fn kind_letter(kind: CompletionKind) -> &'static str {
    match kind {
        CompletionKind::Function => "f",
        CompletionKind::Method => "m",
        CompletionKind::Constructor => "c",
        CompletionKind::Property => "p",
        CompletionKind::Variable => "v",
        CompletionKind::Constant => "k",
        CompletionKind::Class => "C",
        CompletionKind::Interface => "I",
        CompletionKind::Enum => "E",
        CompletionKind::EnumMember => "e",
        CompletionKind::Module => "M",
        CompletionKind::Keyword => "w",
        CompletionKind::TypeParameter => "T",
        CompletionKind::File => "F",
        CompletionKind::Text => "·",
    }
}

fn docs(blocks: &[MarkupBlock]) -> ModelRc<DocBlock> {
    if blocks.is_empty() {
        return ModelRc::default();
    }
    let blocks: Vec<DocBlock> = blocks
        .iter()
        .map(|block| match block {
            MarkupBlock::Code(code) => DocBlock { code: true, text: code.as_str().into() },
            MarkupBlock::Text(text) => DocBlock { code: false, text: text.as_str().into() },
        })
        .collect();
    ModelRc::new(VecModel::from(blocks))
}

/// The pointer over a window's text: once it rests on a cell for
/// [`HOVER_DELAY`], that cell's hover is asked for.
#[derive(Default)]
pub struct HoverPointer {
    timer: slint::Timer,
    /// The pane and cell it is resting on.
    cell: Option<(usize, usize, usize)>,
}

impl HoverPointer {
    /// The pointer moved to a cell of a pane's text.
    pub fn moved(&mut self, key: WindowKey, pane: usize, line: usize, column: usize) {
        if self.cell == Some((pane, line, column)) {
            return;
        }
        self.cell = Some((pane, line, column));
        self.timer.start(TimerMode::SingleShot, HOVER_DELAY, move || {
            with_window(key, move |controller, workbench| controller.hover(workbench, pane, line, column));
        });
    }

    /// The pointer left the text.
    pub fn left(&mut self) {
        self.timer.stop();
        self.cell = None;
    }
}
