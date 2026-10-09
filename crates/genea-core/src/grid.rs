//! Laying grid text out on the editor's monospace grid.
//!
//! The grid gives every character its Unicode width in columns: CJK and
//! emoji take two (spec #19, Encoding). Their glyphs come from fallback
//! fonts whose advances aren't two Menlo cells, so text drawn as one run
//! drifts off the grid after them. The view therefore draws each wide
//! character on its own, at its column.

use unicode_width::UnicodeWidthChar;

/// A stretch of grid text drawn as one item, starting at a grid column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridPiece<'a> {
    /// The 0-based display column it starts at, counted from the start of
    /// the text passed to [`grid_pieces`].
    pub column: usize,
    pub text: &'a str,
}

/// Splits grid text (a [`crate::VisibleLine`]'s `text`, or a slice of it)
/// into pieces that each start at their own grid column: runs of narrow
/// characters, and every wide character alone. Zero-width characters
/// (combining marks, variation selectors) stay with the character before
/// them.
pub fn grid_pieces(text: &str) -> Vec<GridPiece<'_>> {
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut start_column = 0;
    let mut column = 0;
    let mut previous_wide = false;
    for (index, c) in text.char_indices() {
        let width = c.width().unwrap_or(0);
        let wide = width > 1;
        if width > 0 && (wide || previous_wide) && index > start {
            pieces.push(GridPiece { column: start_column, text: &text[start..index] });
            start = index;
            start_column = column;
        }
        if width > 0 {
            previous_wide = wide;
        }
        column += width;
    }
    if start < text.len() {
        pieces.push(GridPiece { column: start_column, text: &text[start..] });
    }
    pieces
}
