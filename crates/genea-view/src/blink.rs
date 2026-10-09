//! Turns off the blink timer of the hidden `TextInput`.
//!
//! Keys reach Genea through a focused, transparent `TextInput` (ADR 0004).
//! While it has focus, Slint toggles its cursor every half blink period on a
//! repeating timer. The cursor is invisible, but each toggle still dirties
//! the window and repaints it, which breaks "an idle caret must not
//! repaint" (ADR 0004). Slint's winit backend reads the period from the
//! `NSTextInsertionPointBlinkPeriod` user default, and a negative period
//! means no blink and no timer.
//!
//! The value goes into the registration domain, which is not persisted. A
//! period the user set in their own defaults still wins.

use objc2::runtime::AnyObject;
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSUserDefaults};

/// Call before any window is created: Slint reads the period when the
/// `TextInput` gets focus.
pub fn turn_off_text_input_blink() {
    let key = NSString::from_str("NSTextInsertionPointBlinkPeriod");
    let period = NSNumber::new_i32(-1);
    let value: &AnyObject = &period;
    let defaults = NSDictionary::from_slices(&[&*key], &[value]);
    // SAFETY: the dictionary maps NSString keys to property-list objects, as
    // registerDefaults: requires.
    unsafe { NSUserDefaults::standardUserDefaults().registerDefaults(&defaults) };
}
