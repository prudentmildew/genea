//! Genea's thin Slint view layer (ADR 0004) and the `genea` binary.
//!
//!   genea                    an empty window; File › Open… picks a project
//!   genea FOLDER [FILE]      opens FOLDER as a project, and FILE in it
//!
//! The view renders the core's view state and turns input into commands. It
//! holds no behaviour of its own: anything a test should check belongs in
//! genea-core. Layout of this crate:
//!
//! - `ui/*.slint`: the markup (one component per file; `app.slint` exports);
//! - `app`: the workbench, the windows, callback wiring, the notifier;
//! - `about`: the About window's third-party licences;
//! - `window`: one project window's view-state → Slint sync;
//! - `surface`: the editor surface's ring of line slots;
//! - `keys`: the keymap; `dialogs`: native Open panels; `links`: opening
//!   web links in the browser.

mod about;
mod app;
mod dialogs;
mod keys;
mod links;
mod surface;
mod window;

use std::path::PathBuf;

slint::include_modules!();

fn main() {
    let mut args = std::env::args_os().skip(1).map(PathBuf::from);
    let folder = args.next();
    let file = args.next();

    // In code, not env vars: env vars would leak into every child process.
    if let Err(error) = slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("skia".into())
        .select()
    {
        eprintln!("genea: couldn't start the winit + Skia backend: {error}");
        std::process::exit(1);
    }

    if let Err(error) = app::start(folder, file) {
        eprintln!("genea: {error}");
        std::process::exit(1);
    }
    if let Err(error) = slint::run_event_loop() {
        eprintln!("genea: {error}");
        std::process::exit(1);
    }
}
