//! Opening web links in the user's browser (NSWorkspace).

use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSString, NSURL};

/// Opens a script's local URL (ticket #40), `http` or `https` on
/// `localhost`, `127.0.0.1` or `[::1]`, in the default browser.
pub fn open_local_url(url: &str) {
    let host = url.strip_prefix("http://").or_else(|| url.strip_prefix("https://")).unwrap_or_default();
    if !["localhost", "127.0.0.1", "[::1]"].iter().any(|local| host.starts_with(local)) {
        eprintln!("genea: not opening a link that isn't local: {url}");
        return;
    }
    open(url);
}

/// Opens `url` in the default browser. Only `https` links are opened.
pub fn open_url(url: &str) {
    if !url.starts_with("https://") {
        eprintln!("genea: not opening a non-https link: {url}");
        return;
    }
    open(url);
}

fn open(url: &str) {
    let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) else {
        eprintln!("genea: couldn't open {url}: not a valid URL");
        return;
    };
    if !NSWorkspace::sharedWorkspace().openURL(&url) {
        eprintln!("genea: couldn't open {}", url.absoluteString().map(|s| s.to_string()).unwrap_or_default());
    }
}
