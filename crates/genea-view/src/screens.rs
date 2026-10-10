//! The screens a restored window may go on (ticket #59): a saved position
//! is used only if the window would still show on a screen (displays get
//! unplugged and rearranged between runs).

use genea_core::WindowFrame;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSScreen, NSWindow, NSWindowStyleMask};
use objc2_foundation::{NSPoint, NSRect, NSSize};

/// How much of a window's top must be on a screen for its saved position to
/// count: enough to grab its title bar.
const GRAB: (f64, f64) = (120.0, 24.0);

/// Each screen's visible frame (without the menu bar and the Dock), in the
/// coordinates window positions use: points from the top-left of the main
/// screen, y growing downwards.
pub fn visible_frames() -> Vec<WindowFrame> {
    let Some(mtm) = MainThreadMarker::new() else { return Vec::new() };
    let screens = NSScreen::screens(mtm);
    // AppKit's origin is the main screen's bottom-left, y growing upwards.
    let Some(main_height) = screens.firstObject().map(|main| main.frame().size.height) else { return Vec::new() };
    screens
        .iter()
        .map(|screen| {
            let visible = screen.visibleFrame();
            WindowFrame {
                x: visible.origin.x,
                y: main_height - (visible.origin.y + visible.size.height),
                width: visible.size.width,
                height: visible.size.height,
            }
        })
        .collect()
}

/// The height of a titled window's title bar, in points.
///
/// A position set before the native window exists (as a restored one is)
/// is taken by winit as the content's top-left, not the window's, while a
/// window reports its outer position; the caller adds this so the window
/// lands where it was.
pub fn title_bar_height() -> f64 {
    let Some(mtm) = MainThreadMarker::new() else { return 0.0 };
    let content = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(400.0, 400.0));
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable;
    let frame = NSWindow::frameRectForContentRect_styleMask(content, style, mtm);
    (frame.size.height - content.size.height).max(0.0)
}

/// Whether a window at `frame` would have its title bar on one of `screens`.
pub fn shows(screens: &[WindowFrame], frame: WindowFrame) -> bool {
    let top = WindowFrame { height: GRAB.1, ..frame };
    screens.iter().any(|screen| {
        let width = (top.x + top.width).min(screen.x + screen.width) - top.x.max(screen.x);
        let height = (top.y + top.height).min(screen.y + screen.height) - top.y.max(screen.y);
        width >= GRAB.0.min(frame.width) && height >= GRAB.1
    })
}
