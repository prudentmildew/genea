//! Highlights: what kind of code a stretch of text is, and how a parsed
//! file becomes highlight spans.
//!
//! A file's highlights are computed in the background from its syntax tree
//! (and from the trees of embedded languages, such as a `<script>` in HTML
//! or a fenced code block in Markdown). They are kept as [`Spans`]: sorted,
//! non-overlapping byte ranges, which the main thread shifts on every edit
//! until the next parse lands.

use std::{cmp::Reverse, ops::Range};

use streaming_iterator::StreamingIterator;
use tree_sitter::{InputEdit, Node, Parser, QueryCursor};

use super::language::{Language, LanguageConfig};

/// What kind of code a highlight span is. The view gives each its colour
/// from the light or dark palette.
///
/// Syntax highlighting produces these from tree-sitter captures; semantic
/// highlighting (#46) refines them with the language server's tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Highlight {
    Comment = 1,
    Keyword,
    Operator,
    /// Brackets, delimiters and markup punctuation.
    Punctuation,
    String,
    /// Regular expressions, CSS colours.
    StringSpecial,
    /// Escape sequences in strings.
    Escape,
    Number,
    /// Named constants, `true`, `false`, `null`, `undefined`.
    Constant,
    Function,
    /// A capitalised name used as a value: a class or constructor.
    Constructor,
    Type,
    /// Built-in types: `string`, `number`, …
    TypeBuiltin,
    Variable,
    /// `this`, `super` and well-known globals.
    VariableBuiltin,
    Parameter,
    /// Object properties, JSON and YAML keys, CSS properties and selectors.
    Property,
    /// HTML and JSX tags.
    Tag,
    Attribute,
    /// YAML anchors and aliases.
    Label,
    /// Markdown headings.
    Heading,
    Emphasis,
    Strong,
    /// Markdown link targets and references.
    Link,
    /// Markdown code spans and code blocks.
    Literal,
    /// Inlay hints and code lenses (ticket #46): text the editor shows
    /// that isn't in the file.
    Hint,
}

impl Highlight {
    const ALL: [Highlight; 26] = [
        Highlight::Comment,
        Highlight::Keyword,
        Highlight::Operator,
        Highlight::Punctuation,
        Highlight::String,
        Highlight::StringSpecial,
        Highlight::Escape,
        Highlight::Number,
        Highlight::Constant,
        Highlight::Function,
        Highlight::Constructor,
        Highlight::Type,
        Highlight::TypeBuiltin,
        Highlight::Variable,
        Highlight::VariableBuiltin,
        Highlight::Parameter,
        Highlight::Property,
        Highlight::Tag,
        Highlight::Attribute,
        Highlight::Label,
        Highlight::Heading,
        Highlight::Emphasis,
        Highlight::Strong,
        Highlight::Link,
        Highlight::Literal,
        Highlight::Hint,
    ];

    fn from_paint(paint: Paint) -> Option<Highlight> {
        Highlight::ALL.get(usize::from(paint).checked_sub(1)?).copied()
    }
}

/// A byte's highlight while painting: 0 is plain text, otherwise a
/// [`Highlight`]'s discriminant.
pub(super) type Paint = u8;

const PLAIN: Paint = 0;

/// What a query capture paints: `None` for captures that aren't highlights
/// (`@injection.content`, `@_name`, …).
///
/// Names are matched most specific first: `string.special.key` before
/// `string.special` before `string`.
pub(super) fn capture_paint(name: &str) -> Option<Paint> {
    let mut name = name;
    loop {
        if let Some(paint) = exact_capture_paint(name) {
            return Some(paint);
        }
        name = &name[..name.rfind('.')?];
    }
}

fn exact_capture_paint(name: &str) -> Option<Paint> {
    use Highlight::*;
    let highlight = match name {
        // Regions that go back to plain text inside a highlighted one: a
        // template substitution in a string, a code block's contents.
        "embedded" | "none" => return Some(PLAIN),
        "comment" => Comment,
        "keyword" => Keyword,
        "operator" => Operator,
        "punctuation" => Punctuation,
        "string.special.key" => Property,
        "string.special" => StringSpecial,
        "string.escape" | "escape" => Escape,
        "string" => String,
        "number" => Number,
        "boolean" | "constant" => Constant,
        "function" => Function,
        "constructor" => Constructor,
        "type.builtin" => TypeBuiltin,
        "type" => Type,
        "variable.builtin" => VariableBuiltin,
        "variable.parameter" => Parameter,
        "variable" => Variable,
        "property" => Property,
        "tag" => Tag,
        "attribute" => Attribute,
        "label" => Label,
        "text.title" => Heading,
        "text.emphasis" => Emphasis,
        "text.strong" => Strong,
        "text.uri" | "text.reference" => Link,
        "text.literal" => Literal,
        _ => return None,
    };
    Some(highlight as Paint)
}

/// One highlighted byte range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Span {
    pub(crate) start: u32,
    pub(crate) end: u32,
    pub(crate) highlight: Highlight,
}

/// A file's highlight spans: sorted, non-overlapping and non-empty.
///
/// Offsets are `u32` to halve the memory of a big file's spans; files above
/// [`crate::editor::LARGE_FILE_BYTES`] get no syntax at all.
#[derive(Clone, Debug, Default)]
pub(crate) struct Spans(Vec<Span>);

impl Spans {
    /// Run-length encodes painted bytes.
    pub(super) fn from_paint(paint: &[Paint]) -> Self {
        let mut spans = Vec::new();
        let mut start = 0;
        while start < paint.len() {
            let value = paint[start];
            let end = paint[start..].iter().position(|&p| p != value).map_or(paint.len(), |n| start + n);
            if let Some(highlight) = Highlight::from_paint(value) {
                spans.push(Span { start: start as u32, end: end as u32, highlight });
            }
            start = end;
        }
        Spans(spans)
    }

    /// Spans from any list: sorted, with empty ones and ones overlapping
    /// an earlier one left out.
    pub(crate) fn from_spans(spans: impl IntoIterator<Item = Span>) -> Self {
        let mut spans: Vec<Span> = spans.into_iter().filter(|s| s.start < s.end).collect();
        spans.sort_by_key(|s| s.start);
        let mut end = 0;
        spans.retain(|s| {
            let keep = s.start >= end;
            if keep {
                end = s.end;
            }
            keep
        });
        Spans(spans)
    }

    /// The spans that overlap a byte range, in order.
    pub(crate) fn overlapping(&self, bytes: Range<usize>) -> &[Span] {
        let first = self.0.partition_point(|s| (s.end as usize) <= bytes.start);
        let end = first + self.0[first..].partition_point(|s| (s.start as usize) < bytes.end);
        &self.0[first..end]
    }

    /// Shifts the spans over an edit, so they stay on the text they covered
    /// until the next parse replaces them. A span the edit falls inside
    /// grows or shrinks with it; text inserted between spans, or at either
    /// edge of one, is plain.
    pub(crate) fn edit(&mut self, edit: &InputEdit) {
        let (start, old_end, new_end) = (edit.start_byte as i64, edit.old_end_byte as i64, edit.new_end_byte as i64);
        let delta = new_end - old_end;
        let first = self.0.partition_point(|s| (s.end as i64) <= start);
        for span in &mut self.0[first..] {
            let (s, e) = (span.start as i64, span.end as i64);
            let s = if s < start { s } else if s >= old_end { s + delta } else { new_end };
            let e = if e >= old_end && e > start { e + delta } else { e.min(start) };
            span.start = s as u32;
            span.end = e.max(s) as u32;
        }
        self.0.retain(|s| s.start < s.end);
    }
}

/// How deep embedded languages nest (Markdown → HTML → JavaScript).
const MAX_INJECTION_DEPTH: usize = 3;

/// Paints `root`'s highlights, and those of the languages embedded in it,
/// into `paint` (one value per byte of `text`).
///
/// Where captures overlap, the innermost node wins. For the same node, the
/// language's queries decide whether the first or the last pattern wins
/// ([`LanguageConfig::last_pattern_wins`]).
pub(super) fn paint_tree(config: &LanguageConfig, root: Node, text: &[u8], paint: &mut [Paint], depth: usize) {
    let mut captures = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.captures(&config.highlights, root, text);
    while let Some((found, index)) = matches.next() {
        let capture = found.captures()[*index];
        if let Some(value) = config.paints[capture.index as usize] {
            let node = capture.node;
            let pattern = found.pattern_index;
            let rank = if config.last_pattern_wins { pattern } else { usize::MAX - pattern };
            captures.push((node.start_byte(), node.end_byte(), rank, value));
        }
    }
    captures.sort_unstable_by_key(|&(start, end, rank, _)| (start, Reverse(end), rank));
    for (start, end, _, value) in captures {
        let end = end.min(paint.len());
        paint[start..end].fill(value);
    }

    if depth < MAX_INJECTION_DEPTH {
        for (language, range) in injections(config, root, text) {
            let Some(config) = language.config() else { continue };
            let mut parser = Parser::new();
            if parser.set_language(&config.language).is_err() || parser.set_included_ranges(&[range]).is_err() {
                continue;
            }
            if let Some(tree) = parser.parse(text, None) {
                paint_tree(config, tree.root_node(), text, paint, depth + 1);
            }
        }
    }
}

/// The embedded languages in a tree and where they are.
fn injections(config: &LanguageConfig, root: Node, text: &[u8]) -> Vec<(Language, tree_sitter::Range)> {
    let Some(query) = &config.injections else { return Vec::new() };
    let mut found = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, text);
    while let Some(m) = matches.next() {
        let mut language = query
            .property_settings(m.pattern_index)
            .iter()
            .find(|p| &*p.key == "injection.language")
            .and_then(|p| p.value.as_deref())
            .and_then(Language::from_name);
        let mut content = None;
        for capture in m.captures() {
            match query.capture_names()[capture.index as usize] {
                "injection.language" => {
                    let name = String::from_utf8_lossy(&text[capture.node.byte_range()]);
                    language = Language::from_name(name.trim());
                }
                "injection.content" => content = Some(capture.node.range()),
                _ => {}
            }
        }
        if let (Some(language), Some(range)) = (language, content) {
            found.push((language, range));
        }
    }
    found
}
