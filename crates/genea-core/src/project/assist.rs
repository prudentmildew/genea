//! Completion, hover and signature help (ticket #43): the project's side.
//! The protocol is in `crate::lsp::assist`.
//!
//! Every request is sent after a command, once `sync_language` has given
//! the server the editor's text, and remembers the focused editor's file,
//! version and primary caret ([`Context`]). Its answer is used only if the
//! editor is still exactly there, and only if it is the latest request of
//! its kind: a slow answer never holds up typing, and a stale one is
//! dropped. Popups follow the same rule: one shown for another context
//! closes (or, for the completion list and signature help, is refiltered or
//! asked again).
//!
//! - **Completion** opens while typing a word (not one starting with a
//!   digit), after one of the server's trigger characters, or on
//!   `ShowCompletion`. Typing on narrows the list here, without asking
//!   again, unless the server said its list was incomplete. The selected
//!   item is resolved in the background for its documentation and its
//!   auto-import edit; accepting it inserts it and that edit as one undo
//!   step. An item accepted before its resolve answered gets its import as
//!   a second step when the answer comes. Only with a single caret.
//! - **Hover** asks about a grid cell (the pointer resting) or the caret
//!   (Quick Documentation) and closes when the text or the caret moves.
//! - **Signature help** opens on the server's trigger characters (`(`,
//!   `,`) or `ShowSignatureHelp`, asks again after every change while it is
//!   open, and closes when the server has nothing (the caret left the
//!   call).

use std::{ops::Range, path::PathBuf, time::Instant};

use ropey::Rope;
use serde_json::Value;

use super::Project;
use crate::{
    command::Command,
    editor::Editor,
    lsp::{
        Pending,
        assist::{self, Completion, Signature, SignatureTrigger, TextEdit},
        text::{self, Encoding},
    },
    view::{CompletionItem, CompletionView, EditorView, HoverView, MAX_COMPLETION_ITEMS, MarkupBlock, SignatureHelpView},
};

/// What a command means for these popups, worked out before it runs.
#[derive(Clone, Debug)]
pub(super) enum Trigger {
    /// One character typed.
    Typed(char),
    ShowCompletion,
    HoverAt { line: usize, column: usize },
    ShowHover,
    ShowSignatureHelp,
    Other,
}

impl Trigger {
    pub(super) fn of(command: &Command) -> Self {
        match command {
            Command::InsertText(text) => {
                let mut chars = text.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Trigger::Typed(c),
                    _ => Trigger::Other,
                }
            }
            Command::ShowCompletion => Trigger::ShowCompletion,
            Command::HoverAt { line, column } => Trigger::HoverAt { line: *line, column: *column },
            Command::ShowHover => Trigger::ShowHover,
            Command::ShowSignatureHelp => Trigger::ShowSignatureHelp,
            _ => Trigger::Other,
        }
    }
}

/// The focused editor as a request or a popup was for.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Context {
    path: PathBuf,
    version: u64,
    /// The primary caret (char index).
    caret: usize,
}

/// A request whose answer is wanted.
#[derive(Debug)]
struct Asked {
    tag: u64,
    context: Context,
    /// What it asked about (char index): the start of the word being
    /// completed, or the hovered char.
    at: usize,
}

/// The completion list and what it came from.
struct CompletionList {
    /// The tag of the request it answers.
    id: u64,
    /// What `shown` was last worked out for.
    context: Context,
    /// Where the word being completed starts.
    start: usize,
    /// The caret when the answer came, for items with their own range.
    caret_at_answer: usize,
    items: Vec<Entry>,
    incomplete: bool,
    /// Indices into `items` that match the typed word, best first.
    shown: Vec<usize>,
    /// Index into `shown`.
    selected: usize,
}

struct Entry {
    completion: Completion,
    /// The server's range for it (chars), as of the answer.
    range: Option<Range<usize>>,
    resolved: Option<Resolved>,
}

/// What resolving an item added, with its edits in chars of the text they
/// were made for.
struct Resolved {
    detail: Option<String>,
    documentation: Vec<MarkupBlock>,
    imports: Vec<(Range<usize>, String)>,
    snapshot: Rope,
}

/// A `completionItem/resolve` in flight.
struct Resolving {
    tag: u64,
    /// The list and item it is for, or `None` for an item already
    /// accepted, whose import goes in when the answer comes.
    item: Option<(u64, usize)>,
    path: PathBuf,
    /// The text when it was sent: the server's positions are in it.
    snapshot: Rope,
    encoding: Encoding,
}

/// A hover or signature help being shown.
struct Shown<T> {
    context: Context,
    /// Where it is anchored (char index).
    at: usize,
    content: T,
}

/// A project's completion, hover and signature help state.
#[derive(Default)]
pub(crate) struct Assist {
    next_tag: u64,
    completion: Option<CompletionList>,
    asked_completion: Option<Asked>,
    resolving: Option<Resolving>,
    /// An item accepted before it was resolved: resolve it after the sync.
    accepted: Option<Value>,
    hover: Option<Shown<Vec<MarkupBlock>>>,
    asked_hover: Option<Asked>,
    signature: Option<Shown<Signature>>,
    asked_signature: Option<Asked>,
}

impl Project {
    /// The completion, hover and signature help commands that change state
    /// right away. Requests go out later, in [`assist_after`](Self::assist_after).
    pub(super) fn assist_command(&mut self, command: Command, now: Instant) {
        match command {
            Command::MoveCompletionSelection(by) => {
                if let Some(list) = &mut self.assist.completion
                    && !list.shown.is_empty()
                {
                    let count = list.shown.len() as isize;
                    list.selected = (list.selected as isize + by).rem_euclid(count) as usize;
                }
            }
            Command::SelectCompletionItem(index) => {
                if let Some(list) = &mut self.assist.completion
                    && index < list.shown.len()
                {
                    list.selected = index;
                }
            }
            Command::AcceptCompletion => self.accept_completion(now),
            Command::CloseCompletion => {
                self.assist.completion = None;
                self.assist.asked_completion = None;
            }
            Command::HideHover => {
                self.assist.hover = None;
                self.assist.asked_hover = None;
            }
            Command::HideSignatureHelp => {
                self.assist.signature = None;
                self.assist.asked_signature = None;
            }
            _ => {}
        }
    }

    /// After a command and the sync that gave the server its text: drops
    /// popups the command made stale, narrows the completion list, and
    /// sends the requests the command calls for.
    pub(super) fn assist_after(&mut self, trigger: Trigger) {
        let Some(focus) = self.assist_focus() else {
            self.assist = Assist { next_tag: self.assist.next_tag, ..Assist::default() };
            return;
        };
        // Another file has the focus: nothing shown is for it.
        let elsewhere = |context: &Context| context.path != focus.path;
        if self.assist.completion.as_ref().is_some_and(|l| elsewhere(&l.context)) {
            self.assist.completion = None;
        }
        if self.assist.signature.as_ref().is_some_and(|s| elsewhere(&s.context)) {
            self.assist.signature = None;
        }
        let capabilities = self.language.server().map(|s| s.assist_capabilities().clone()).unwrap_or_default();
        let single_caret = self.editor.as_ref().is_some_and(|e| e.caret_count() == 1);

        // Hover closes when anything moves.
        if self.assist.hover.as_ref().is_some_and(|h| h.context != focus) {
            self.assist.hover = None;
        }

        // The completion list narrows while the caret stays in its word.
        let mut ask_completion: Option<Option<String>> = None;
        if let Some(list) = &mut self.assist.completion
            && list.context != focus
        {
            let editor = self.editor.as_ref().expect("a focused editor");
            if !single_caret || !in_word(editor.text(), list.start, focus.caret) {
                self.assist.completion = None;
            } else {
                let changed = list.context.version != focus.version;
                list.refilter(editor.text(), focus.clone());
                if list.incomplete && changed {
                    ask_completion = Some(None);
                } else if list.shown.is_empty() {
                    self.assist.completion = None;
                }
            }
        }

        let mut ask_signature: Option<SignatureTrigger> = None;
        let mut ask_hover: Option<usize> = None;
        let signature_open = self.assist.signature.is_some() || self.assist.asked_signature.is_some();
        match trigger {
            Trigger::Typed(c) => {
                let typed = c.to_string();
                if single_caret && capabilities.completion_triggers.contains(&typed) {
                    self.assist.completion = None;
                    ask_completion = Some(Some(typed.clone()));
                } else if single_caret && is_word(c) && self.assist.completion.is_none() {
                    let editor = self.editor.as_ref().expect("a focused editor");
                    let start = word_start(editor.text(), focus.caret);
                    if !editor.text().char(start).is_ascii_digit() {
                        ask_completion = Some(None);
                    }
                }
                if capabilities.signature_triggers.contains(&typed)
                    || (signature_open && capabilities.signature_retriggers.contains(&typed))
                {
                    ask_signature = Some(SignatureTrigger::Character(typed));
                }
            }
            Trigger::ShowCompletion if single_caret => ask_completion = Some(None),
            Trigger::HoverAt { line, column } => {
                match self.editor.as_ref().and_then(|e| e.char_at_cell(line, column)) {
                    Some(at) => ask_hover = Some(at),
                    None => {
                        self.assist.hover = None;
                        self.assist.asked_hover = None;
                    }
                }
            }
            Trigger::ShowHover => {
                let text = self.editor.as_ref().expect("a focused editor").text();
                let caret = focus.caret;
                let at_word = caret < text.len_chars() && is_word(text.char(caret));
                ask_hover = Some(if !at_word && caret > 0 && is_word(text.char(caret - 1)) { caret - 1 } else { caret });
            }
            Trigger::ShowSignatureHelp => ask_signature = Some(SignatureTrigger::Invoked),
            _ => {}
        }
        if ask_signature.is_none() && self.assist.signature.as_ref().is_some_and(|s| s.context != focus) {
            ask_signature = Some(SignatureTrigger::ContentChange);
        }

        if let Some(trigger) = ask_completion
            && capabilities.completion
        {
            self.ask_completion(&focus, trigger.as_deref());
        }
        if let Some(at) = ask_hover
            && capabilities.hover
        {
            self.ask_hover(&focus, at);
        }
        if let Some(trigger) = ask_signature
            && capabilities.signature_help
        {
            self.ask_signature(&focus, &trigger, signature_open);
        }
        if capabilities.resolve {
            self.resolve_selected();
        }
    }

    /// The focused editor, if it is one the language server knows.
    fn assist_focus(&self) -> Option<Context> {
        let editor = self.editor.as_ref()?;
        if editor.is_large() || !editor.is_loaded() || text::language_id(editor.path()).is_none() {
            return None;
        }
        Some(Context { path: editor.path().to_owned(), version: editor.version(), caret: editor.caret_position() })
    }

    /// Sends a request for the focused file; its tag, or `None` if the
    /// server isn't ready.
    fn assist_request(&mut self, method: &str, params: impl FnOnce(&str, &Rope, Encoding) -> Value) -> Option<u64> {
        let editor = self.editor.as_ref()?;
        let server = self.language.server_mut()?;
        let params = params(&server.uri(editor.path()), editor.text(), server.encoding());
        self.assist.next_tag += 1;
        let tag = self.assist.next_tag;
        server.request(method, params, Pending::Assist(tag))?;
        Some(tag)
    }

    fn ask_completion(&mut self, focus: &Context, trigger: Option<&str>) {
        let start = word_start(self.editor.as_ref().expect("a focused editor").text(), focus.caret);
        let tag = self.assist_request("textDocument/completion", |uri, text, encoding| {
            assist::completion_params(uri, text::lsp_position(text, focus.caret, encoding), trigger)
        });
        self.assist.asked_completion = tag.map(|tag| Asked { tag, context: focus.clone(), at: start });
    }

    fn ask_hover(&mut self, focus: &Context, at: usize) {
        let tag = self.assist_request("textDocument/hover", |uri, text, encoding| {
            assist::hover_params(uri, text::lsp_position(text, at, encoding))
        });
        self.assist.asked_hover = tag.map(|tag| Asked { tag, context: focus.clone(), at });
    }

    fn ask_signature(&mut self, focus: &Context, trigger: &SignatureTrigger, retrigger: bool) {
        let tag = self.assist_request("textDocument/signatureHelp", |uri, text, encoding| {
            assist::signature_help_params(uri, text::lsp_position(text, focus.caret, encoding), trigger, retrigger)
        });
        self.assist.asked_signature = tag.map(|tag| Asked { tag, context: focus.clone(), at: focus.caret });
    }

    /// Resolves an accepted item (for its auto-import), or else the
    /// selected one (for its documentation), unless that is under way.
    fn resolve_selected(&mut self) {
        let Some(editor) = self.editor.as_ref() else { return };
        let path = editor.path().to_owned();
        let snapshot = editor.text().clone();
        let (raw, item) = if let Some(raw) = self.assist.accepted.take() {
            (raw, None)
        } else {
            let Some(list) = &self.assist.completion else { return };
            let Some(&index) = list.shown.get(list.selected) else { return };
            let wanted = Some((list.id, index));
            if list.items[index].resolved.is_some() || self.assist.resolving.as_ref().is_some_and(|r| r.item == wanted) {
                return;
            }
            (list.items[index].completion.raw.clone(), wanted)
        };
        let Some(server) = self.language.server_mut() else { return };
        let encoding = server.encoding();
        self.assist.next_tag += 1;
        let tag = self.assist.next_tag;
        if server.request("completionItem/resolve", raw, Pending::Assist(tag)).is_some() {
            self.assist.resolving = Some(Resolving { tag, item, path, snapshot, encoding });
        }
    }

    /// Return or Tab in the completion list: inserts the selected item in
    /// place of the typed word, with its auto-import if it is resolved, as
    /// one undo step.
    fn accept_completion(&mut self, now: Instant) {
        let Some(list) = self.assist.completion.take() else { return };
        self.assist.asked_completion = None;
        let Some(&index) = list.shown.get(list.selected) else { return };
        let entry = &list.items[index];
        let resolve = self.language.server().is_some_and(|s| s.assist_capabilities().resolve);
        let rows = self.viewport_rows;
        let Some(editor) = self.editor.as_mut().filter(|e| *e.path() == list.context.path) else { return };
        let caret = editor.caret_position();
        let range = match &entry.range {
            Some(range) => {
                let end = range.end as isize + caret as isize - list.caret_at_answer as isize;
                range.start..(end.max(caret as isize) as usize)
            }
            None => list.start..caret,
        };
        let mut edits = vec![(range.clone(), entry.completion.insert.clone())];
        if let Some(resolved) = &entry.resolved {
            let imports = map_edits(&resolved.snapshot, editor.text(), &resolved.imports);
            edits.extend(imports.into_iter().filter(|(r, _)| r.end <= range.start || r.start >= range.end));
        }
        editor.apply_text_edits(edits, 0, now, rows);
        if entry.resolved.is_none() && resolve {
            self.assist.accepted = Some(entry.completion.raw.clone());
        }
    }

    /// A completion, resolve, hover or signature help answer.
    pub(super) fn assist_answered(&mut self, tag: u64, result: Result<Value, String>, now: Instant) {
        let focus = self.assist_focus();
        let current = |asked: &Option<Asked>| asked.as_ref().is_some_and(|a| a.tag == tag);
        if current(&self.assist.asked_completion) {
            let asked = self.assist.asked_completion.take().expect("checked");
            if focus.as_ref() == Some(&asked.context)
                && let Ok(result) = result
            {
                self.completion_answered(tag, asked, result);
            }
        } else if self.assist.resolving.as_ref().is_some_and(|r| r.tag == tag) {
            let resolving = self.assist.resolving.take().expect("checked");
            if let Ok(item) = result {
                self.resolve_answered(resolving, &item, now);
            }
        } else if current(&self.assist.asked_hover) {
            let asked = self.assist.asked_hover.take().expect("checked");
            if focus.as_ref() == Some(&asked.context) {
                let editor = self.editor.as_ref().expect("a focused editor");
                let encoding = self.language.server().map(|s| s.encoding()).unwrap_or_default();
                self.assist.hover = result.ok().as_ref().and_then(assist::hover).map(|(contents, start)| Shown {
                    at: start.map_or(asked.at, |(line, character)| text::char_index(editor.text(), line, character, encoding)),
                    context: asked.context,
                    content: contents,
                });
            }
        } else if current(&self.assist.asked_signature) {
            let asked = self.assist.asked_signature.take().expect("checked");
            if focus.as_ref() == Some(&asked.context) {
                self.assist.signature = result
                    .ok()
                    .as_ref()
                    .and_then(assist::signature_help)
                    .map(|content| Shown { at: asked.at, context: asked.context, content });
            }
        }
    }

    fn completion_answered(&mut self, tag: u64, asked: Asked, result: Value) {
        let Some(editor) = self.editor.as_ref() else { return };
        let encoding = self.language.server().map(|s| s.encoding()).unwrap_or_default();
        let completions = assist::completions(result);
        let text = editor.text();
        let range = |edit: &TextEdit| {
            text::char_index(text, edit.start.0, edit.start.1, encoding)
                ..text::char_index(text, edit.end.0, edit.end.1, encoding)
        };
        let items = completions
            .items
            .into_iter()
            .map(|completion| Entry { range: completion.edit.as_ref().map(range), completion, resolved: None })
            .collect();
        let mut list = CompletionList {
            id: tag,
            context: asked.context.clone(),
            start: asked.at,
            caret_at_answer: asked.context.caret,
            items,
            incomplete: completions.incomplete,
            shown: Vec::new(),
            selected: 0,
        };
        list.refilter(text, asked.context);
        self.assist.completion = (!list.shown.is_empty() || list.incomplete).then_some(list);
        if self.language.server().is_some_and(|s| s.assist_capabilities().resolve) {
            self.resolve_selected();
        }
    }

    fn resolve_answered(&mut self, resolving: Resolving, item: &Value, now: Instant) {
        let details = assist::resolved(item);
        let snapshot = &resolving.snapshot;
        let imports: Vec<(Range<usize>, String)> = details
            .additional
            .iter()
            .map(|edit| {
                let start = text::char_index(snapshot, edit.start.0, edit.start.1, resolving.encoding);
                let end = text::char_index(snapshot, edit.end.0, edit.end.1, resolving.encoding);
                (start..end, edit.text.clone())
            })
            .collect();
        match resolving.item {
            Some((id, index)) => {
                if let Some(list) = self.assist.completion.as_mut().filter(|l| l.id == id)
                    && let Some(entry) = list.items.get_mut(index)
                {
                    entry.resolved = Some(Resolved {
                        detail: details.detail,
                        documentation: details.documentation,
                        imports,
                        snapshot: resolving.snapshot,
                    });
                }
            }
            // Accepted already: its import goes in now, as its own step.
            None => {
                let rows = self.viewport_rows;
                if imports.is_empty() {
                    return;
                }
                let Some(editor) = self.editor.as_mut().filter(|e| *e.path() == resolving.path) else { return };
                let caret = editor.caret_position();
                let mut edits = vec![(caret..caret, String::new())];
                edits.extend(map_edits(snapshot, editor.text(), &imports).into_iter().filter(|(r, _)| {
                    !(r.start < caret && caret < r.end)
                }));
                if edits.len() > 1 {
                    editor.apply_text_edits(edits, 0, now, rows);
                }
            }
        }
    }

    /// The popups over the focused editor's view.
    pub(super) fn assist_view(&self, editor: &Editor, view: &mut EditorView) {
        let Some(focus) = self.assist_focus().filter(|f| f.path == editor.path()) else { return };
        view.completion = self.assist.completion.as_ref().filter(|l| l.context == focus).map(|list| {
            let selected = list.shown.get(list.selected).map(|&i| &list.items[i]);
            let resolved = selected.and_then(|e| e.resolved.as_ref());
            CompletionView {
                at: editor.cell_of(list.start),
                items: list
                    .shown
                    .iter()
                    .map(|&i| {
                        let completion = &list.items[i].completion;
                        CompletionItem {
                            label: completion.label.clone(),
                            detail: completion.label_detail.clone(),
                            source: completion.source.clone(),
                            kind: completion.kind,
                        }
                    })
                    .collect(),
                selected: list.selected,
                detail: resolved.and_then(|r| r.detail.clone()),
                documentation: resolved.map(|r| r.documentation.clone()).unwrap_or_default(),
            }
        });
        view.hover = self
            .assist
            .hover
            .as_ref()
            .filter(|h| h.context == focus)
            .map(|hover| HoverView { at: editor.cell_of(hover.at), contents: hover.content.clone() });
        view.signature_help = self.assist.signature.as_ref().map(|shown| {
            let signature = &shown.content;
            SignatureHelpView {
                at: editor.cell_of(focus.caret),
                label: signature.label.clone(),
                active_parameter: signature.active_parameter.clone(),
                documentation: signature.documentation.clone(),
                signature: signature.index,
                signatures: signature.count,
            }
        });
    }
}

impl CompletionList {
    /// Works out which items match the word typed so far (from each item's
    /// start to the caret), best first, and selects the best.
    fn refilter(&mut self, text: &Rope, context: Context) {
        let caret = context.caret;
        let mut ranked: Vec<(u8, &str, String, usize)> = Vec::new();
        for (i, entry) in self.items.iter().enumerate() {
            let start = entry.range.as_ref().map_or(self.start, |r| r.start);
            if start > caret {
                continue;
            }
            let typed = text.slice(start..caret).to_string();
            if let Some(class) = rank(&typed, &entry.completion.filter) {
                ranked.push((class, &entry.completion.sort, entry.completion.label.to_lowercase(), i));
            }
        }
        ranked.sort();
        self.shown = ranked.into_iter().take(MAX_COMPLETION_ITEMS).map(|(_, _, _, i)| i).collect();
        let typed_nothing = caret == self.start;
        self.selected = if typed_nothing {
            self.shown.iter().position(|&i| self.items[i].completion.preselect).unwrap_or(0)
        } else {
            0
        };
        self.context = context;
    }
}

/// How well `candidate` matches what was typed, best (0) first, or `None`
/// if it doesn't: its start as typed, its start in any case, the typed
/// chars in order from its first char, or from the start of a word in it
/// (`useState` matches `st`).
fn rank(typed: &str, candidate: &str) -> Option<u8> {
    if typed.is_empty() || candidate.starts_with(typed) {
        return Some(0);
    }
    let lower = typed.to_lowercase();
    let candidate_lower = candidate.to_lowercase();
    if candidate_lower.starts_with(&lower) {
        return Some(1);
    }
    let first = lower.chars().next()?;
    let chars: Vec<char> = candidate.chars().collect();
    let lowered: Vec<char> = candidate_lower.chars().collect();
    let word_starts = (0..chars.len()).filter(|&i| {
        i == 0
            || (chars[i].is_uppercase() && !chars[i - 1].is_uppercase())
            || (chars[i].is_alphanumeric() && !chars[i - 1].is_alphanumeric())
    });
    let subsequence = |from: usize| {
        let mut rest = lowered[from..].iter();
        lower.chars().all(|c| rest.any(|d| *d == c))
    };
    let mut class = 2;
    for start in word_starts {
        if lowered[start] == first && subsequence(start) {
            return Some(class);
        }
        class = 3;
    }
    None
}

/// A char of an identifier (TypeScript's, roughly).
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Where the word ending at `caret` starts.
fn word_start(text: &Rope, caret: usize) -> usize {
    let mut start = caret.min(text.len_chars());
    while start > 0 && is_word(text.char(start - 1)) {
        start -= 1;
    }
    start
}

/// Whether the caret is still in the word that starts at `start`.
fn in_word(text: &Rope, start: usize, caret: usize) -> bool {
    caret >= start && caret <= text.len_chars() && text.slice(start..caret).chars().all(is_word)
}

/// Moves edits made against `before` onto `after`: an edit before what
/// changed in between keeps its place, one after it shifts with it, and one
/// in it is dropped. What changed is found by comparing the two texts'
/// common start and end, which is cheap for the typing between a request
/// and its answer.
fn map_edits(before: &Rope, after: &Rope, edits: &[(Range<usize>, String)]) -> Vec<(Range<usize>, String)> {
    let (old_len, new_len) = (before.len_chars(), after.len_chars());
    let prefix = before.chars().zip(after.chars()).take_while(|(a, b)| a == b).count();
    let max_suffix = old_len.min(new_len) - prefix;
    let suffix = before
        .chars_at(old_len)
        .reversed()
        .zip(after.chars_at(new_len).reversed())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    let changed_end = old_len - suffix;
    let shift = new_len as isize - old_len as isize;
    edits
        .iter()
        .filter_map(|(range, text)| {
            if range.end <= prefix {
                Some((range.clone(), text.clone()))
            } else if range.start >= changed_end {
                let moved = |i: usize| (i as isize + shift) as usize;
                Some((moved(range.start)..moved(range.end), text.clone()))
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_rank_prefixes_before_word_starts() {
        assert_eq!(rank("to", "toString"), Some(0));
        assert_eq!(rank("To", "toString"), Some(1));
        assert_eq!(rank("tstr", "toString"), Some(2));
        assert_eq!(rank("st", "useState"), Some(3));
        assert_eq!(rank("x", "useState"), None);
        assert_eq!(rank("", "anything"), Some(0));
    }

    #[test]
    fn edits_move_with_the_text_typed_in_between() {
        let before = Rope::from_str("let a;\nfoo\n");
        let after = Rope::from_str("let a;\nfoobar\n");
        let edits = vec![(0..0, "import x;\n".to_owned()), (11..11, "!".to_owned()), (9..11, "O".to_owned())];
        assert_eq!(map_edits(&before, &after, &edits), [(0..0, "import x;\n".to_owned()), (14..14, "!".to_owned())]);
    }
}
