//! The native Open panel for project folders (NSOpenPanel), and the sheet
//! that asks to save a closing tab's edits (NSAlert).
//!
//! The panel is modeless (`beginWithCompletionHandler:`), not
//! `runModal`: a nested modal run loop inside winit's event handling is
//! asking for re-entrancy trouble. The completion handler comes back through
//! Slint's event loop, so callers can touch the app normally.

use std::{cell::RefCell, path::PathBuf};

use block2::RcBlock;
use genea_core::CloseChoice;
use objc2::{MainThreadMarker, rc::Retained};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertSecondButtonReturn, NSModalResponse, NSModalResponseOK, NSOpenPanel,
    NSView, NSWindow,
};
use objc2_foundation::NSString;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::ComponentHandle;

use crate::ProjectWindow;

/// Asks for a project folder. `done` gets the folder, or nothing is called
/// on cancel. (Files open from the Files view, ticket #30.)
pub fn pick_folder(done: impl FnOnce(PathBuf) + Send + 'static) {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("genea: Open panels must be shown from the main thread");
        return;
    };
    let panel: Retained<NSOpenPanel> = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseDirectories(true);
    panel.setCanChooseFiles(false);
    panel.setAllowsMultipleSelection(false);
    panel.setCanCreateDirectories(true);
    panel.setPrompt(Some(&NSString::from_str("Open")));

    // The block type is `Fn`; the callback runs once.
    let done = RefCell::new(Some(done));
    let chosen_from = panel.clone();
    let handler = RcBlock::new(move |response: NSModalResponse| {
        if response != NSModalResponseOK {
            return;
        }
        let Some(path) = chosen_from.URL().and_then(|url| url.to_file_path()) else { return };
        if let Some(done) = done.borrow_mut().take() {
            // Back through Slint's loop (which this also wakes), outside
            // AppKit's callout.
            let _ = slint::invoke_from_event_loop(move || done(path));
        }
    });
    panel.beginWithCompletionHandler(&handler);
}

/// Asks whether to save a closing tab's edits, as a sheet on `window`:
/// Save, Don't Save or Cancel. `done` gets the answer through Slint's event
/// loop. Like the Open panels, the sheet is not a nested modal run loop.
pub fn ask_to_save(window: &ProjectWindow, title: &str, done: impl FnOnce(CloseChoice) + Send + 'static) {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("genea: alerts must be shown from the main thread");
        return;
    };
    let Some(ns_window) = ns_window(window) else {
        // No native window to attach to: keep the tab and its edits.
        let _ = slint::invoke_from_event_loop(move || done(CloseChoice::Cancel));
        return;
    };
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(&format!("Do you want to save the changes you made to “{title}”?")));
    alert.setInformativeText(&NSString::from_str("Your changes will be lost if you don’t save them."));
    // In this order: the response codes are the first, second and third button.
    alert.addButtonWithTitle(&NSString::from_str("Save"));
    alert.addButtonWithTitle(&NSString::from_str("Don’t Save"));
    alert.addButtonWithTitle(&NSString::from_str("Cancel"));

    let done = RefCell::new(Some(done));
    let handler = RcBlock::new(move |response: NSModalResponse| {
        let choice = match response {
            r if r == NSAlertFirstButtonReturn => CloseChoice::Save,
            r if r == NSAlertSecondButtonReturn => CloseChoice::Discard,
            _ => CloseChoice::Cancel,
        };
        if let Some(done) = done.borrow_mut().take() {
            let _ = slint::invoke_from_event_loop(move || done(choice));
        }
    });
    alert.beginSheetModalForWindow_completionHandler(&ns_window, Some(&handler));
}

/// The NSWindow behind a Slint window, once it exists.
fn ns_window(window: &ProjectWindow) -> Option<Retained<NSWindow>> {
    let handle = window.window().window_handle();
    let handle = handle.window_handle().ok()?;
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else { return None };
    // SAFETY: an AppKit window handle's `ns_view` is a live NSView for as
    // long as the window exists, and we are on the main thread.
    let view: &NSView = unsafe { appkit.ns_view.cast().as_ref() };
    view.window()
}
