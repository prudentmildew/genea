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
//! - `about`: the About window's third-party licences;
//! - `window`: one project window's view-state → Slint sync;
//! - `welcome`: the welcome window's sync;
//! - `new_project`: the New Project dialog's sync (its own window);
//! - `surface`: the editor surface's ring of line slots;
//! - `terminal`: the terminal pane's rows, keys and mouse;
//! - `navigation`: the Usages view and the rename prompt (ticket #44);
//! - `fonts`: registers the emoji font the first time an emoji is shown;
//! - `blink`: keeps the hidden TextInput from repainting on a timer;
//! - `keys`: the keymap; `dialogs`: native Open panels; `pasteboard`: the
//!   system clipboard behind the host's `Clipboard`; `links`: opening web
//!   links in the browser.
//! - `journal` and `remote`: the benchmark harness's instrumentation journal
//!   and control channel, both off unless `GENEA_JOURNAL=1`.

mod about;
mod app;
mod assist;
mod blink;
mod dialogs;
mod fonts;
mod journal;
mod keys;
mod links;
mod new_project;
mod navigation;
mod pasteboard;
mod remote;
mod surface;
mod terminal;
mod welcome;
mod window;

use std::path::PathBuf;

slint::include_modules!();

fn main() {
    let mut args = std::env::args_os().skip(1).map(PathBuf::from);
    let folder = args.next();
    let file = args.next();

    // The benchmark journal (`GENEA_JOURNAL=1`); off, it installs nothing.
    journal::start();

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
    // The harness's commands need the event loop, which exists from here.
    if journal::on() {
        remote::listen();
    }
    // Not `run_event_loop`: closing the last project window brings back the
    // welcome instead of quitting. The app quits from the welcome or ⌘Q.
    if let Err(error) = slint::run_event_loop_until_quit() {
        eprintln!("genea: {error}");
        std::process::exit(1);
    }
}
