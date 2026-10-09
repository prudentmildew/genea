//! PROTOTYPE (#18): warm process-global caches on a background thread while
//! the main thread brings up AppKit. `SPIKE_PREWARM=1`.
//!
//! CoreText loads font language metadata the first time it rasterizes a
//! glyph (~9 ms inside Skia's first flush). Rasterizing a few Menlo glyphs
//! here moves that off the main thread's critical path.

use std::ffi::c_void;

type CFTypeRef = *const c_void;

#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    fn CTFontCreateWithName(name: CFTypeRef, size: f64, matrix: *const c_void) -> CFTypeRef;
    fn CTFontGetGlyphsForCharacters(font: CFTypeRef, chars: *const u16, glyphs: *mut u16, count: isize) -> bool;
    fn CTFontDrawGlyphs(font: CFTypeRef, glyphs: *const u16, positions: *const [f64; 2], count: usize, ctx: CFTypeRef);
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGColorSpaceCreateDeviceGray() -> CFTypeRef;
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits: usize,
        row_bytes: usize,
        space: CFTypeRef,
        info: u32,
    ) -> CFTypeRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithBytes(alloc: CFTypeRef, bytes: *const u8, len: isize, encoding: u32, ext: bool) -> CFTypeRef;
    fn CFRelease(cf: CFTypeRef);
}

fn coretext() {
    unsafe {
        let name = CFStringCreateWithBytes(std::ptr::null(), b"Menlo".as_ptr(), 5, 0x0800_0100, false);
        let font = CTFontCreateWithName(name, 26.0, std::ptr::null());
        let chars: Vec<u16> = "abcXYZ019{}();".encode_utf16().collect();
        let mut glyphs = vec![0u16; chars.len()];
        CTFontGetGlyphsForCharacters(font, chars.as_ptr(), glyphs.as_mut_ptr(), chars.len() as isize);
        let positions: Vec<[f64; 2]> = (0..glyphs.len()).map(|i| [i as f64 * 16.0, 8.0]).collect();
        let space = CGColorSpaceCreateDeviceGray();
        let ctx = CGBitmapContextCreate(std::ptr::null_mut(), 256, 32, 8, 256, space, 0);
        CTFontDrawGlyphs(font, glyphs.as_ptr(), positions.as_ptr(), glyphs.len(), ctx);
        for cf in [ctx, space, font, name] {
            CFRelease(cf);
        }
    }
}

pub fn start() {
    if std::env::var_os("SPIKE_PREWARM").is_none() {
        return;
    }
    std::thread::spawn(|| {
        coretext();
        crate::trace("prewarm: coretext done");
    });
}
