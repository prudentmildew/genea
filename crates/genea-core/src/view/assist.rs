//! View state of completion, hover and signature help (ticket #43): the
//! popups over the focused editor. Each comes from the language server
//! asynchronously and is dropped if the buffer moved on before it arrived.

use std::ops::Range;

use super::Caret;

/// The most items the completion list holds: typing narrows it.
pub const MAX_COMPLETION_ITEMS: usize = 100;

/// The completion list, below the word being completed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionView {
    /// Where the word being completed starts: the list opens below it.
    pub at: Caret,
    /// The server's items that match what has been typed, best first, at
    /// most [`MAX_COMPLETION_ITEMS`](crate::MAX_COMPLETION_ITEMS).
    pub items: Vec<CompletionItem>,
    /// Index into `items` of the one Return or Tab inserts.
    pub selected: usize,
    /// More about the selected item, once the server has said: its type or
    /// signature, or for an auto-import "Add import from …".
    pub detail: Option<String>,
    /// The selected item's documentation, once the server has sent it.
    pub documentation: Vec<MarkupBlock>,
}

/// One completion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    /// What is inserted (usually) and shown.
    pub label: String,
    /// Shown right after the label, dimmed: a signature like `()`.
    pub detail: Option<String>,
    /// Shown at the right: where it comes from, like the module an
    /// auto-import would import it from.
    pub source: Option<String>,
    pub kind: CompletionKind,
}

/// What a completion is, for its icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CompletionKind {
    Function,
    Method,
    Constructor,
    Property,
    Variable,
    Constant,
    Class,
    Interface,
    Enum,
    EnumMember,
    Module,
    Keyword,
    TypeParameter,
    /// File and folder names (in import paths).
    File,
    /// Anything else.
    Text,
}

/// What the server says about the code under the pointer (or the caret,
/// with Quick Documentation).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoverView {
    /// Where the hovered word starts: the popup opens above it.
    pub at: Caret,
    pub contents: Vec<MarkupBlock>,
}

/// A piece of the server's Markdown: code (a type, a signature), drawn in
/// the editor font, or prose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkupBlock {
    Code(String),
    Text(String),
}

/// The signature of the call the caret is in, with the argument it is at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureHelpView {
    /// The primary caret: the popup opens above it.
    pub at: Caret,
    /// The whole signature, e.g. `max(...values: number[]): number`.
    pub label: String,
    /// The active parameter's chars in `label`, to emphasise.
    pub active_parameter: Option<Range<usize>>,
    /// The signature's and the active parameter's documentation.
    pub documentation: Vec<MarkupBlock>,
    /// Which overload this is (0-based), of how many.
    pub signature: usize,
    pub signatures: usize,
}
