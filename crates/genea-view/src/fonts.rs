//! Apple Color Emoji, registered lazily (spec #19, Editing core: Font).
//!
//! The Slint patch registers Menlo and Apple Symbols at start (ADR 0004,
//! patches/README.md). Apple Color Emoji is 192 MB, and Slint's font
//! registration reads font files whole, so registering it up front would
//! break the idle-memory budget. Instead, before text is handed to Slint,
//! `prepare` looks for a glyph that Menlo and Apple Symbols are missing.
//! The first time one is an emoji, the font is registered as the emoji
//! family and as the fallback for symbols.
//!
//! It is registered from a memory map, not read, so registering costs
//! almost nothing. Skia still copies the font when it first draws an emoji
//! (`i-slint-renderer-skia`'s font cache), so memory grows by the font's
//! size only once an emoji is on screen.

use std::{cell::RefCell, collections::HashSet, fs::File, sync::Arc};

use slint::fontique_011::fontique::{
    self, Blob, FallbackKey, GenericFamily, QueryFamily, QueryStatus, Script, SourceCache,
};

const EMOJI_FONT: &str = "/System/Library/Fonts/Apple Color Emoji.ttc";

#[derive(Default)]
struct LazyFonts {
    /// Characters already looked up in the registered fonts.
    checked: HashSet<char>,
    source_cache: SourceCache,
    /// Tried (whether or not it worked): never retried.
    emoji_registered: bool,
}

thread_local! {
    static FONTS: RefCell<LazyFonts> = RefCell::default();
}

/// Registers the emoji font if `text` has an emoji that the registered
/// fonts can't draw. Call it with text before Slint shapes it. Cheap for
/// text it has seen before, and for ASCII.
pub fn prepare(text: &str) {
    if text.is_ascii() {
        return;
    }
    FONTS.with_borrow_mut(|fonts| {
        if fonts.emoji_registered {
            return;
        }
        let mut collection = None;
        for c in text.chars().filter(|&c| may_be_emoji(c)) {
            if !fonts.checked.insert(c) {
                continue;
            }
            let collection = collection.get_or_insert_with(slint::fontique_011::shared_collection);
            if !covered(collection, &mut fonts.source_cache, c) {
                fonts.emoji_registered = true;
                register_emoji(collection);
                return;
            }
        }
    });
}

/// Whether Slint's generic families (Menlo, then Apple Symbols) have a
/// glyph for `c`.
fn covered(collection: &mut fontique::Collection, source_cache: &mut SourceCache, c: char) -> bool {
    let mut query = collection.query(source_cache);
    query.set_families([QueryFamily::Generic(GenericFamily::SansSerif)]);
    let mut found = false;
    query.matches_with(|font| {
        found = font.charmap().and_then(|charmap| charmap.map(c)).is_some_and(|glyph| glyph != 0);
        if found { QueryStatus::Stop } else { QueryStatus::Continue }
    });
    found
}

fn register_emoji(collection: &mut fontique::Collection) {
    let map = File::open(EMOJI_FONT).and_then(|file| {
        // SAFETY: the font is on the read-only system volume, so the file
        // can't change under the map while Genea runs.
        unsafe { memmap2::Mmap::map(&file) }
    });
    let map = match map {
        Ok(map) => map,
        Err(error) => {
            eprintln!("genea: couldn't map {EMOJI_FONT}: {error}");
            return;
        }
    };
    // Only face 0, "Apple Color Emoji". Face 1 (".Apple Color Emoji UI")
    // would take Skia's slower path for faces past the first in a .ttc,
    // which copies the font more than once (i-slint-renderer-skia).
    let families: Vec<_> = collection
        .register_fonts(Blob::new(Arc::new(map)), None)
        .into_iter()
        .filter(|(_, faces)| faces.iter().any(|face| face.index() == 0))
        .map(|(id, _)| id)
        .collect();
    // Parley asks the emoji family for emoji clusters, and the fallbacks of
    // the Common script (where emoji are) for other symbols.
    collection.append_generic_families(GenericFamily::Emoji, families.iter().copied());
    collection.append_fallbacks(FallbackKey::new(Script::from_bytes(*b"Zyyy"), None), families.into_iter());
}

/// Characters an emoji font might draw: the symbol, pictograph and emoji
/// blocks. Menlo and Apple Symbols cover many of them; only the ones they
/// miss register the emoji font.
fn may_be_emoji(c: char) -> bool {
    matches!(
        c as u32,
        0x00A9 | 0x00AE | 0x203C | 0x2049 | 0x20E3 | 0x2100..=0x2BFF | 0x3030 | 0x303D | 0x3297 | 0x3299
            | 0x1F000..=0x1FAFF
    )
}
