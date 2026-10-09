//! PROTOTYPE. Synthetic key presses posted into this app's own AppKit event
//! queue, so they travel the real path: NSApp -> NSWindow -> GPUI's view ->
//! NSTextInputClient. Posting to our own queue needs no Accessibility access.
#![allow(unexpected_cfgs, deprecated)]

use cocoa::{
    base::{NO, id, nil},
    foundation::{NSAutoreleasePool, NSPoint, NSString},
};
use objc::{class, msg_send, sel, sel_impl};

const KEY_DOWN: u64 = 10;
const KEY_UP: u64 = 11;

/// A key the typing bench presses: the characters it produces and its
/// ANSI virtual key code.
#[derive(Clone, Copy)]
pub struct Key(pub &'static str, pub u16);

pub const RETURN: Key = Key("\r", 36);
pub const BACKSPACE: Key = Key("\u{7f}", 51);

pub fn letter(c: char) -> Key {
    const CODES: [(char, u16, &str); 27] = [
        ('a', 0, "a"), ('s', 1, "s"), ('d', 2, "d"), ('f', 3, "f"), ('h', 4, "h"),
        ('g', 5, "g"), ('z', 6, "z"), ('x', 7, "x"), ('c', 8, "c"), ('v', 9, "v"),
        ('b', 11, "b"), ('q', 12, "q"), ('w', 13, "w"), ('e', 14, "e"), ('r', 15, "r"),
        ('y', 16, "y"), ('t', 17, "t"), ('o', 31, "o"), ('u', 32, "u"), ('i', 34, "i"),
        ('p', 35, "p"), ('l', 37, "l"), ('j', 38, "j"), ('k', 40, "k"), ('n', 45, "n"),
        ('m', 46, "m"), (' ', 49, " "),
    ];
    let (_, code, s) = CODES.iter().find(|(ch, _, _)| *ch == c).unwrap();
    Key(s, *code)
}

/// Posts a key down and key up for `key` to the key window.
pub fn press(key: Key) {
    unsafe {
        let pool = NSAutoreleasePool::new(nil);
        let app: id = msg_send![class!(NSApplication), sharedApplication];
        let window: id = msg_send![app, keyWindow];
        let number: isize = if window == nil { 0 } else { msg_send![window, windowNumber] };
        let info: id = msg_send![class!(NSProcessInfo), processInfo];
        let now: f64 = msg_send![info, systemUptime];
        let chars = NSString::alloc(nil).init_str(key.0);
        for kind in [KEY_DOWN, KEY_UP] {
            let event: id = msg_send![class!(NSEvent),
                keyEventWithType: kind
                location: NSPoint::new(0., 0.)
                modifierFlags: 0u64
                timestamp: now
                windowNumber: number
                context: nil
                characters: chars
                charactersIgnoringModifiers: chars
                isARepeat: NO
                keyCode: key.1];
            let _: () = msg_send![app, postEvent: event atStart: NO];
        }
        let _: () = msg_send![chars, release];
        pool.drain();
    }
}

/// The main screen's maximum refresh rate (60 on displays without ProMotion).
pub fn display_fps() -> f64 {
    unsafe {
        let screen: id = msg_send![class!(NSScreen), mainScreen];
        let fps: isize = msg_send![screen, maximumFramesPerSecond];
        fps as f64
    }
}

/// Whether any of this app's windows is at least partly visible on screen.
/// macOS throttles presentation for occluded windows (another Space, covered
/// by a full-screen app), which would ruin the frame benches.
pub fn window_visible() -> bool {
    unsafe {
        let app: id = msg_send![class!(NSApplication), sharedApplication];
        let windows: id = msg_send![app, windows];
        let count: usize = msg_send![windows, count];
        (0..count).any(|i| {
            let window: id = msg_send![windows, objectAtIndex: i];
            let state: u64 = msg_send![window, occlusionState];
            state & (1 << 1) != 0
        })
    }
}
