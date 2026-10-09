//! Structural editing (ticket #25): what the syntax tree says about
//! indentation, matching brackets, foldable regions and the nodes around a
//! selection. Positions are bytes and tree-sitter rows of the buffer; the
//! editor converts them to chars.
//!
//! The rules are generic rather than per-language queries: a node delimits
//! a block when its first and last children are a bracket pair (`{}`, `[]`,
//! `()`, `${}`) or an element's start and end tags (HTML, JSX). That covers
//! the first-class languages, JSON, CSS and HTML; YAML and Markdown add a
//! few kinds of their own.

use std::ops::Range;

use tree_sitter::{Node, Tree, TreeCursor};

/// Opening brackets and the tokens that close them.
const BRACKETS: [(&str, &str); 4] = [("{", "}"), ("[", "]"), ("(", ")"), ("${", "}")];

/// What a line break at the caret should do to the new line's indentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Indent {
    /// The new line goes one level deeper than the caret's line.
    pub(crate) deeper: bool,
    /// The text after the caret starts with what closes that level, so it
    /// goes on a line of its own at the caret line's indentation.
    pub(crate) split: bool,
}

/// The block a line broken at `byte` (on `row`) opens: the innermost node
/// whose opening delimiter ends on that row at or before the caret and whose
/// closing one starts at or after it. `rest` is the byte the text after the
/// caret starts at, whitespace skipped.
pub(crate) fn indent(tree: &Tree, byte: usize, row: usize, rest: usize) -> Indent {
    let Some(mut node) = tree.root_node().descendant_for_byte_range(byte, byte) else { return Indent::default() };
    loop {
        if let Some((open, close)) = delimiters(node)
            && open.start_position().row == row
            && open.end_byte() <= byte
            && close.start_byte() >= byte
        {
            return Indent { deeper: true, split: !close.is_missing() && close.start_byte() == rest };
        }
        match node.parent() {
            Some(parent) => node = parent,
            None => return Indent::default(),
        }
    }
}

/// The token that closes an opening bracket.
pub(crate) fn closing_bracket(open: &str) -> Option<&'static str> {
    BRACKETS.iter().find(|(o, _)| *o == open).map(|(_, c)| *c)
}

/// A node's opening and closing delimiters, if it is a block: a bracket
/// pair or an element's tags.
fn delimiters(node: Node) -> Option<(Node, Node)> {
    if node.child_count() < 2 {
        return None;
    }
    let open = node.child(0)?;
    let close = node.child(node.child_count() - 1)?;
    let brackets = closing_bracket(open.kind()).is_some_and(|c| c == close.kind());
    let tags = matches!(open.kind(), "start_tag" | "jsx_opening_element")
        && matches!(close.kind(), "end_tag" | "jsx_closing_element");
    (brackets || tags).then_some((open, close))
}

/// The bracket token starting at `byte` and the one that matches it, as
/// byte ranges, if there is a bracket there.
pub(crate) fn matching_bracket(tree: &Tree, byte: usize) -> Option<(Range<usize>, Range<usize>)> {
    let node = tree.root_node().descendant_for_byte_range(byte, byte + 1)?;
    if node.start_byte() != byte || node.is_named() || node.is_missing() {
        return None;
    }
    let parent = node.parent()?;
    let kind = node.kind();
    let mut cursor = parent.walk();
    let siblings: Vec<Node> = parent.children(&mut cursor).collect();
    let index = siblings.iter().position(|s| s.id() == node.id())?;
    let matched = if let Some(close) = closing_bracket(kind) {
        siblings[index + 1..].iter().find(|s| s.kind() == close)
    } else {
        let opens: Vec<&str> = BRACKETS.iter().filter(|(_, c)| *c == kind).map(|(o, _)| *o).collect();
        if opens.is_empty() {
            return None;
        }
        siblings[..index].iter().rev().find(|s| opens.contains(&s.kind()))
    }?;
    (!matched.is_missing()).then(|| (node.byte_range(), matched.byte_range()))
}

/// The next larger stretch to select around `bytes`: the smallest node, or
/// the inside of a block between its delimiters, that contains it and is
/// larger. `None` when the whole file is selected already.
pub(crate) fn expansion(tree: &Tree, bytes: Range<usize>) -> Option<Range<usize>> {
    let root = tree.root_node();
    let mut node = root.descendant_for_byte_range(bytes.start, bytes.end)?;
    loop {
        let range = node.byte_range();
        let contains = range.start <= bytes.start && bytes.end <= range.end;
        if contains {
            // A block's inside comes before the block.
            if let Some((open, close)) = delimiters(node) {
                let inside = inner_range(node, open, close);
                if let Some(inside) = inside
                    && inside.start <= bytes.start
                    && bytes.end <= inside.end
                    && inside != bytes
                {
                    return Some(inside);
                }
            }
            if range != bytes {
                return Some(range);
            }
        }
        node = node.parent()?;
    }
}

/// The text between a block's delimiters, from its first child after the
/// opening one to its last before the closing one.
fn inner_range(node: Node, open: Node, close: Node) -> Option<Range<usize>> {
    let mut cursor = node.walk();
    let inner: Vec<Node> = node.children(&mut cursor).filter(|c| c.id() != open.id() && c.id() != close.id()).collect();
    Some(inner.first()?.start_byte()..inner.last()?.end_byte())
}

/// A region that can fold: its first row stays visible, the rows after it
/// up to `last` are hidden.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FoldRegion {
    /// The byte the region starts at (its opening delimiter), on the row
    /// that stays visible.
    pub(crate) start: usize,
    pub(crate) header: usize,
    /// The last hidden row.
    pub(crate) last: usize,
}

/// The region a node folds, if it folds: a block, a multi-line comment, a
/// Markdown section or code block, or a YAML key's multi-line value. A
/// block's closing row stays visible when its closing delimiter starts it.
fn fold_region(node: Node, line_start: &impl Fn(usize) -> bool) -> Option<FoldRegion> {
    let start = node.start_position();
    let end = node.end_position();
    // A node ending with its line break ends on the row before.
    let end_row = if end.column == 0 && end.row > start.row { end.row - 1 } else { end.row };
    if end_row <= start.row {
        return None;
    }
    let kind = node.kind();
    let (from, last) = if let Some((open, close)) = delimiters(node) {
        let header = open.end_position().row;
        let close_row = close.start_position().row;
        let last = if line_start(close.start_byte()) { close_row.saturating_sub(1) } else { close_row };
        (open, last.max(header))
    } else if kind.contains("comment")
        || matches!(kind, "section" | "fenced_code_block" | "block_mapping_pair" | "block_sequence_item")
    {
        (node, end_row)
    } else {
        return None;
    };
    let header = from.start_position().row;
    (last > header).then_some(FoldRegion { start: from.start_byte(), header, last })
}

/// The foldable regions that start on `rows`, at most one per row (the
/// largest), in row order.
pub(crate) fn fold_regions_in(
    tree: &Tree,
    rows: Range<usize>,
    line_start: &impl Fn(usize) -> bool,
) -> Vec<FoldRegion> {
    let mut found: Vec<FoldRegion> = Vec::new();
    let mut cursor = tree.walk();
    visit(&mut cursor, &rows, line_start, &mut found);
    found.sort_by_key(|r| (r.header, std::cmp::Reverse(r.last)));
    found.dedup_by_key(|r| r.header);
    found
}

/// Collects the fold regions of the cursor's node and its descendants that
/// start on `rows`, skipping subtrees outside them.
fn visit(cursor: &mut TreeCursor, rows: &Range<usize>, line_start: &impl Fn(usize) -> bool, found: &mut Vec<FoldRegion>) {
    let node = cursor.node();
    if node.end_position().row < rows.start || node.start_position().row >= rows.end {
        return;
    }
    if let Some(region) = fold_region(node, line_start)
        && rows.contains(&region.header)
    {
        found.push(region);
    }
    if cursor.goto_first_child() {
        loop {
            visit(cursor, rows, line_start, found);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
}

/// The foldable regions that contain `byte`'s row (header included),
/// innermost first.
pub(crate) fn fold_regions_at(tree: &Tree, byte: usize, line_start: &impl Fn(usize) -> bool) -> Vec<FoldRegion> {
    let Some(mut node) = tree.root_node().descendant_for_byte_range(byte, byte) else { return Vec::new() };
    let mut found = Vec::new();
    loop {
        if let Some(region) = fold_region(node, line_start) {
            found.push(region);
        }
        match node.parent() {
            Some(parent) => node = parent,
            None => return found,
        }
    }
}
