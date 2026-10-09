//! Opening web links in the user's browser (NSWorkspace).

use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSString, NSURL};

/// Opens `url` in the default browser. Only `https` links are opened.
pub fn open_url(url: &str) {
    if !url.starts_with("https://") {
        eprintln!("genea: not opening a non-https link: {url}");
        return;
    }
    let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) else {
        eprintln!("genea: couldn't open {url}: not a valid URL");
        return;
    };
    if !NSWorkspace::sharedWorkspace().openURL(&url) {
        eprintln!("genea: couldn't open {}", url.absoluteString().map(|s| s.to_string()).unwrap_or_default());
    }
}
