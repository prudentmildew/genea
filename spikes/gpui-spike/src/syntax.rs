//! PROTOTYPE. tree-sitter over a ropey buffer: full parse, incremental
//! reparse after an edit, and highlight kinds for a visible byte range.

use std::ops::Range;

use ropey::Rope;
use streaming_iterator::StreamingIterator;
use tree_sitter::{InputEdit, Language, Node, Parser, Point, Query, QueryCursor, TextProvider, Tree};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Keyword,
    String,
    Comment,
    Function,
    Type,
    Number,
    Property,
    Constant,
    Operator,
    Punctuation,
}

pub struct Syntax {
    parser: Parser,
    query: Query,
    kinds: Vec<Option<Kind>>,
    pub tree: Tree,
}

impl Syntax {
    /// Full parse. Called off the main thread.
    pub fn new(rope: &Rope) -> Self {
        let language: Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        let mut parser = Parser::new();
        parser.set_language(&language).unwrap();
        // The TypeScript grammar inherits JavaScript's; its highlight query is
        // meant to be layered on top of JavaScript's.
        let source = format!(
            "{}\n{}",
            tree_sitter_typescript::HIGHLIGHTS_QUERY,
            tree_sitter_javascript::HIGHLIGHT_QUERY
        );
        let query = Query::new(&language, &source)
            .or_else(|_| Query::new(&language, tree_sitter_typescript::HIGHLIGHTS_QUERY))
            .unwrap();
        let kinds = query.capture_names().iter().map(|name| kind_for(name)).collect();
        let tree = parse(&mut parser, rope, None);
        Self {
            parser,
            query,
            kinds,
            tree,
        }
    }

    /// Incremental reparse after `edit` has been applied to `rope`.
    pub fn edit(&mut self, rope: &Rope, edit: &InputEdit) {
        self.tree.edit(edit);
        self.tree = parse(&mut self.parser, rope, Some(&self.tree));
    }

    /// One highlight kind per byte of `range` (first matching pattern wins).
    pub fn kinds(&self, rope: &Rope, range: Range<usize>) -> Vec<Option<Kind>> {
        let mut out = vec![None; range.len()];
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(range.clone());
        let mut captures = cursor.captures(&self.query, self.tree.root_node(), RopeText(rope));
        while let Some((m, index)) = captures.next() {
            let capture = m.captures[*index];
            let Some(kind) = self.kinds[capture.index as usize] else {
                continue;
            };
            let node = capture.node.byte_range();
            let start = node.start.max(range.start) - range.start;
            let end = node.end.min(range.end).saturating_sub(range.start);
            for slot in out.iter_mut().take(end).skip(start) {
                if slot.is_none() {
                    *slot = Some(kind);
                }
            }
        }
        out
    }
}

fn kind_for(name: &str) -> Option<Kind> {
    Some(match name.split('.').next()? {
        "keyword" => Kind::Keyword,
        "string" => Kind::String,
        "comment" => Kind::Comment,
        "function" | "constructor" => Kind::Function,
        "type" => Kind::Type,
        "number" => Kind::Number,
        "property" => Kind::Property,
        "constant" => Kind::Constant,
        "variable" if name == "variable.builtin" => Kind::Constant,
        "operator" => Kind::Operator,
        "punctuation" => Kind::Punctuation,
        _ => return None,
    })
}

fn parse(parser: &mut Parser, rope: &Rope, old: Option<&Tree>) -> Tree {
    parser
        .parse_with_options(
            &mut |byte, _| {
                if byte >= rope.len_bytes() {
                    return &[][..];
                }
                let (chunk, start, _, _) = rope.chunk_at_byte(byte);
                &chunk.as_bytes()[byte - start..]
            },
            old,
            None,
        )
        .unwrap()
}

/// tree-sitter position of `byte` in `rope`.
pub fn point(rope: &Rope, byte: usize) -> Point {
    let row = rope.byte_to_line(byte);
    Point::new(row, byte - rope.line_to_byte(row))
}

struct RopeText<'a>(&'a Rope);

impl<'a> TextProvider<&'a [u8]> for RopeText<'a> {
    type I = Chunks<'a>;

    fn text(&mut self, node: Node) -> Self::I {
        Chunks(self.0.byte_slice(node.byte_range()).chunks())
    }
}

struct Chunks<'a>(ropey::iter::Chunks<'a>);

impl<'a> Iterator for Chunks<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(str::as_bytes)
    }
}
