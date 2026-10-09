//! The system clipboard: the general pasteboard, plain text only.
//!
//! The core reaches the clipboard through its host (`genea_host::Clipboard`),
//! but no core crate may link AppKit (ADR 0004), so the app hands this one
//! to `RealHost::with_clipboard`.

use genea_host::Clipboard;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::NSString;

pub struct Pasteboard;

impl Clipboard for Pasteboard {
    fn read_text(&self) -> Option<String> {
        // SAFETY: an AppKit constant, valid for the life of the process.
        let string_type = unsafe { NSPasteboardTypeString };
        NSPasteboard::generalPasteboard().stringForType(string_type).map(|s| s.to_string())
    }

    fn write_text(&self, text: &str) {
        // SAFETY: as above.
        let string_type = unsafe { NSPasteboardTypeString };
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        pasteboard.setString_forType(&NSString::from_str(text), string_type);
    }
}
