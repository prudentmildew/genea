//! Keys the scenarios press, and the keyboard layout they need.
//!
//! A key is a macOS virtual key code (the physical key) plus `CGEventFlags`;
//! the characters come from the current keyboard layout. Letters, Return,
//! Backspace and Space are on the same keys in the US and Norwegian layouts.
//! The dead keys are Norwegian, the layout the dead-key scenario selects.

use std::ffi::{CStr, c_char, c_void};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub code: u16,
    pub flags: u64,
}

pub const SHIFT: u64 = 1 << 17;
pub const OPTION: u64 = 1 << 19;

pub const RETURN: Key = Key { code: 36, flags: 0 };
pub const BACKSPACE: Key = Key { code: 51, flags: 0 };
pub const SPACE: Key = Key { code: 49, flags: 0 };
pub const RIGHT: Key = Key { code: 124, flags: 0 };

/// The key that types a lowercase letter, or Space for `' '`.
pub fn letter(c: char) -> Option<Key> {
    const CODES: [(char, u16); 27] = [
        ('a', 0), ('s', 1), ('d', 2), ('f', 3), ('h', 4), ('g', 5), ('z', 6), ('x', 7), ('c', 8),
        ('v', 9), ('b', 11), ('q', 12), ('w', 13), ('e', 14), ('r', 15), ('y', 16), ('t', 17),
        ('o', 31), ('u', 32), ('i', 34), ('p', 35), ('l', 37), ('j', 38), ('k', 40), ('n', 45),
        ('m', 46), (' ', 49),
    ];
    CODES.iter().find(|(ch, _)| *ch == c).map(|&(_, code)| Key { code, flags: 0 })
}

/// The Norwegian layout's dead keys.
pub mod norwegian {
    use super::{Key, OPTION, SHIFT};

    pub const LAYOUT: &str = "com.apple.keylayout.Norwegian";
    /// ´, left of Backspace.
    pub const ACUTE: Key = Key { code: 24, flags: 0 };
    /// ⇧´ gives `.
    pub const GRAVE: Key = Key { code: 24, flags: SHIFT };
    /// ¨, left of Return.
    pub const DIAERESIS: Key = Key { code: 30, flags: 0 };
    /// ⇧¨ gives ^.
    pub const CIRCUMFLEX: Key = Key { code: 30, flags: SHIFT };
    /// ⌥¨ gives ~.
    pub const TILDE: Key = Key { code: 30, flags: OPTION };
}

// Text Input Sources (Carbon) and the CoreFoundation it speaks.
#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentKeyboardLayoutInputSource() -> *const c_void;
    fn TISGetInputSourceProperty(source: *const c_void, key: *const c_void) -> *const c_void;
    fn TISCreateInputSourceList(properties: *const c_void, include_all_installed: bool) -> *const c_void;
    fn TISEnableInputSource(source: *const c_void) -> i32;
    fn TISSelectInputSource(source: *const c_void) -> i32;
    static kTISPropertyInputSourceID: *const c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(object: *const c_void);
    fn CFStringGetCString(string: *const c_void, buffer: *mut c_char, size: isize, encoding: u32) -> bool;
    fn CFStringCreateWithCString(allocator: *const c_void, text: *const c_char, encoding: u32) -> *const c_void;
    fn CFDictionaryCreate(
        allocator: *const c_void,
        keys: *const *const c_void,
        values: *const *const c_void,
        count: isize,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> *const c_void;
    fn CFArrayGetCount(array: *const c_void) -> isize;
    fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
}

const UTF8: u32 = 0x0800_0100;

/// The current keyboard layout's input source id, such as
/// `com.apple.keylayout.Norwegian`.
pub fn current_layout() -> Option<String> {
    // SAFETY: TIS and CF calls with valid arguments; the copied source is
    // released, the property is borrowed from it.
    unsafe {
        let source = TISCopyCurrentKeyboardLayoutInputSource();
        if source.is_null() {
            return None;
        }
        let id = TISGetInputSourceProperty(source, kTISPropertyInputSourceID);
        let mut buffer = [0 as c_char; 256];
        let ok = !id.is_null() && CFStringGetCString(id, buffer.as_mut_ptr(), buffer.len() as isize, UTF8);
        CFRelease(source);
        ok.then(|| CStr::from_ptr(buffer.as_ptr()).to_string_lossy().into_owned())
    }
}

/// Enables and selects an installed keyboard layout by input source id.
pub fn select_layout(id: &str) -> Result<(), String> {
    let text = std::ffi::CString::new(id).map_err(|e| e.to_string())?;
    // SAFETY: CF and TIS calls with valid arguments; everything created is
    // released, and the array's elements are borrowed from it.
    unsafe {
        let value = CFStringCreateWithCString(std::ptr::null(), text.as_ptr(), UTF8);
        let keys = [kTISPropertyInputSourceID];
        let values = [value];
        let filter = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            &raw const kCFTypeDictionaryKeyCallBacks,
            &raw const kCFTypeDictionaryValueCallBacks,
        );
        let list = TISCreateInputSourceList(filter, true);
        CFRelease(filter);
        CFRelease(value);
        if list.is_null() || CFArrayGetCount(list) == 0 {
            if !list.is_null() {
                CFRelease(list);
            }
            return Err(format!("the keyboard layout {id} isn't installed"));
        }
        let source = CFArrayGetValueAtIndex(list, 0);
        TISEnableInputSource(source);
        let status = TISSelectInputSource(source);
        CFRelease(list);
        if status == 0 { Ok(()) } else { Err(format!("couldn't select {id} (status {status})")) }
    }
}
