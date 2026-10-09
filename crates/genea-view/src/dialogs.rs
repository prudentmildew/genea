//! Native Open panels (NSOpenPanel).
//!
//! The panels are modeless (`beginWithCompletionHandler:`), not
//! `runModal`: a nested modal run loop inside winit's event handling is
//! asking for re-entrancy trouble. The completion handler comes back through
//! Slint's event loop, so callers can touch the app normally.

use std::{cell::RefCell, path::{Path, PathBuf}};

use block2::RcBlock;
use objc2::{MainThreadMarker, rc::Retained};
use objc2_app_kit::{NSModalResponse, NSModalResponseOK, NSOpenPanel};
use objc2_foundation::{NSString, NSURL};

/// Asks for a project folder. `done` gets the folder, or nothing is called
/// on cancel.
pub fn pick_folder(done: impl FnOnce(PathBuf) + Send + 'static) {
    show(Kind::Folder, None, done);
}

/// Asks for a file, starting in `directory`.
pub fn pick_file(directory: &Path, done: impl FnOnce(PathBuf) + Send + 'static) {
    show(Kind::File, Some(directory), done);
}

enum Kind {
    Folder,
    File,
}

fn show(kind: Kind, directory: Option<&Path>, done: impl FnOnce(PathBuf) + Send + 'static) {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("genea: Open panels must be shown from the main thread");
        return;
    };
    let panel: Retained<NSOpenPanel> = NSOpenPanel::openPanel(mtm);
    let folder = matches!(kind, Kind::Folder);
    panel.setCanChooseDirectories(folder);
    panel.setCanChooseFiles(!folder);
    panel.setAllowsMultipleSelection(false);
    panel.setCanCreateDirectories(folder);
    panel.setPrompt(Some(&NSString::from_str("Open")));
    if let Some(url) = directory.and_then(NSURL::from_directory_path) {
        panel.setDirectoryURL(Some(&url));
    }

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
