//! Genea's thin Slint view layer (ADR 0004) and the `genea` binary.
//!
//!   genea                    the welcome window: Open…, recent projects
//!   genea FOLDER [FILE]      opens FOLDER as a project, and FILE in it
//!
//! The view renders the core's view state and turns input into commands. It
//! holds no behaviour of its own: anything a test should check belongs in
//! genea-core. Layout of this crate:
//!
//! - `ui/*.slint`: the markup (one component per file; `app.slint` exports);
//! - `app`: the workbench, the windows, callback wiring, the notifier;
//! - `window`: one project window's view-state → Slint sync;
//! - `welcome`: the welcome window's sync;
//! - `surface`: the editor surface's ring of line slots;
//! - `fonts`: registers the emoji font the first time an emoji is shown;
//! - `blink`: keeps the hidden TextInput from repainting on a timer;
//! - `keys`: the keymap; `dialogs`: native Open panels; `pasteboard`: the
//!   system clipboard behind the host's `Clipboard`.

mod app;
mod blink;
mod dialogs;
mod fonts;
mod keys;
mod pasteboard;
mod surface;
mod welcome;
mod window;

use std::path::PathBuf;

slint::include_modules!();

fn main() {
    let mut args = std::env::args_os().skip(1).map(PathBuf::from);
    let folder = args.next();
    let file = args.next();

    blink::turn_off_text_input_blink();

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
    // Not `run_event_loop`: closing the last project window brings back the
    // welcome instead of quitting. The app quits from the welcome or ⌘Q.
    if let Err(error) = slint::run_event_loop_until_quit() {
        eprintln!("genea: {error}");
        std::process::exit(1);
    }
}
