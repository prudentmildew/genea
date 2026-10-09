//! Fallback fonts registered lazily: Apple Color Emoji and Hiragino Sans GB
//! (spec #19, Editing core: Font).
//!
//! The Slint patch registers Menlo and Apple Symbols at start (ADR 0004,
//! patches/README.md). Apple Color Emoji is 192 MB and Hiragino Sans GB
//! 23 MB, and Slint's font registration reads font files whole, so
//! registering them up front would break the idle-memory budget. Instead,
//! before text is handed to Slint, `prepare` looks for glyphs that Menlo and
//! Apple Symbols are missing. The first time one is an emoji, the emoji font
//! is registered; the first time one is CJK, the CJK font is.
//!
//! They are registered from a memory map, not read, so registering costs
//! almost nothing. Skia still copies a font when it first draws a glyph
//! from it (`i-slint-renderer-skia`'s font cache), so memory grows by the
//! font's size only once such a glyph is on screen.

use std::{cell::RefCell, collections::HashSet, fs::File, sync::Arc};

use slint::fontique_011::fontique::{
    self, Blob, FallbackKey, FamilyId, GenericFamily, QueryFamily, QueryStatus, Script, SourceCache,
};

/// A system font registered the first time it is needed.
struct LazyFont {
    path: &'static str,
    /// Characters it is there for.
    wanted: fn(char) -> bool,
    /// Makes Slint's text layout (parley) use the registered families.
    install: fn(&mut fontique::Collection, &[FamilyId]),
}

const LAZY_FONTS: [LazyFont; 2] = [
    LazyFont {
        path: "/System/Library/Fonts/Apple Color Emoji.ttc",
        wanted: may_be_emoji,
        // Parley asks the emoji family for emoji clusters, and the fallbacks
        // of the Common script (where emoji are) for other symbols.
        install: |collection, families| {
            collection.append_generic_families(GenericFamily::Emoji, families.iter().copied());
            collection.append_fallbacks(FallbackKey::new(Script::from_bytes(*b"Zyyy"), None), families.iter().copied());
        },
    },
    LazyFont {
        path: "/System/Library/Fonts/Hiragino Sans GB.ttc",
        wanted: is_cjk,
        // Parley asks the fallbacks of a run's script: Han, kana and
        // bopomofo. It also asks the Han ones for punctuation in Common.
        install: |collection, families| {
            for script in [*b"Hani", *b"Hira", *b"Kana", *b"Bopo"] {
                collection.append_fallbacks(FallbackKey::new(Script::from_bytes(script), None), families.iter().copied());
            }
        },
    },
];

#[derive(Default)]
struct LazyFonts {
    /// Characters already looked up in the registered fonts.
    checked: HashSet<char>,
    source_cache: SourceCache,
    /// Per entry of `LAZY_FONTS`: tried (whether or not it worked), so
    /// never retried.
    registered: [bool; LAZY_FONTS.len()],
}

thread_local! {
    static FONTS: RefCell<LazyFonts> = RefCell::default();
}

/// Registers the fallback fonts that `text` needs: an emoji or CJK
/// character that the registered fonts can't draw. Call it with text before
/// Slint shapes it. Cheap for text it has seen before, and for ASCII.
pub fn prepare(text: &str) {
    if text.is_ascii() {
        return;
    }
    FONTS.with_borrow_mut(|fonts| {
        if fonts.registered.iter().all(|&done| done) {
            return;
        }
        let mut collection = None;
        for c in text.chars() {
            let pending: Vec<usize> =
                (0..LAZY_FONTS.len()).filter(|&i| !fonts.registered[i] && (LAZY_FONTS[i].wanted)(c)).collect();
            if pending.is_empty() || !fonts.checked.insert(c) {
                continue;
            }
            let collection = collection.get_or_insert_with(slint::fontique_011::shared_collection);
            if covered(collection, &mut fonts.source_cache, c) {
                continue;
            }
            for i in pending {
                fonts.registered[i] = true;
                register(collection, &LAZY_FONTS[i]);
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

fn register(collection: &mut fontique::Collection, font: &LazyFont) {
    let map = File::open(font.path).and_then(|file| {
        // SAFETY: the font is on the read-only system volume, so the file
        // can't change under the map while Genea runs.
        unsafe { memmap2::Mmap::map(&file) }
    });
    let map = match map {
        Ok(map) => map,
        Err(error) => {
            eprintln!("genea: couldn't map {}: {error}", font.path);
            return;
        }
    };
    // Only the families with face 0 (Apple Color Emoji's ".Apple Color
    // Emoji UI" is face 1): Skia's path for later faces of a .ttc copies the
    // font more than once (i-slint-renderer-skia).
    let families: Vec<_> = collection
        .register_fonts(Blob::new(Arc::new(map)), None)
        .into_iter()
        .filter(|(_, faces)| faces.iter().any(|face| face.index() == 0))
        .map(|(id, _)| id)
        .collect();
    (font.install)(collection, &families);
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

/// Chinese and Japanese characters (Hiragino Sans GB has no Hangul): CJK
/// radicals, punctuation, kana, bopomofo, ideographs and their extensions,
/// and full-width forms.
fn is_cjk(c: char) -> bool {
    matches!(
        c as u32,
        0x2E80..=0x2FDF
            | 0x3000..=0x312F
            | 0x31A0..=0x31FF
            | 0x3200..=0x33FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFFEF
            | 0x20000..=0x3134F
    )
}
