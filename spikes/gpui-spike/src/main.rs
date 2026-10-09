//! PROTOTYPE: throwaway GPUI validation spike for "Does GPUI meet Genea's
//! budgets?" (prudentmildew/genea#14). Not Genea code; never merge.
//!
//!   gpui-spike [FILE]                      open FILE to judge typing/scrolling feel
//!   gpui-spike bench <NAME> [--out FILE]   run one in-process benchmark
//!   gpui-spike suite --label LABEL         run every benchmark in child processes

mod bench;
mod editor;
mod metrics;
mod suite;
mod sys;
mod synth;
mod syntax;

use std::path::PathBuf;

use gpui::{
    App, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, prelude::*, px, size,
};
use gpui_platform::application;

use editor::{Backspace, Down, Editor, Left, Newline, PageDown, PageUp, Quit, Right, Up};

pub fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures").join(name)
}

fn trace(what: &str) {
    if std::env::var("SPIKE_TRACE").is_ok() {
        eprintln!("{:>8.2} ms  {what}", sys::since_process_start(std::time::Instant::now()).as_secs_f64() * 1000.0);
    }
}

fn main() {
    trace("main");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1).cloned())
    };
    match args.first().map(String::as_str) {
        Some("suite") => suite::run(&flag("--label").unwrap_or_else(|| "raw".into())),
        Some("bench") => {
            let name = args.get(1).expect("bench name").clone();
            let out = flag("--out").map(PathBuf::from);
            run_app(fixture(bench::file_for(&name)), Some((name, out)));
        }
        file => run_app(
            file.map(PathBuf::from)
                .unwrap_or_else(|| fixture("checker.ts")),
            None,
        ),
    }
}

fn run_app(file: PathBuf, bench: Option<(String, Option<PathBuf>)>) {
    let app = application();
    trace("application() built");
    app.run(move |cx: &mut App| {
        trace("run callback");
        // First, so the journal sees startup too. Not for the idle bench or
        // interactive use: its polling thread would be a wake-up source itself.
        let recorder = bench
            .as_ref()
            .filter(|(name, _)| name != "idle")
            .map(|_| metrics::Recorder::start(cx));
        cx.bind_keys([
            KeyBinding::new("backspace", Backspace, None),
            KeyBinding::new("enter", Newline, None),
            KeyBinding::new("left", Left, None),
            KeyBinding::new("right", Right, None),
            KeyBinding::new("up", Up, None),
            KeyBinding::new("down", Down, None),
            KeyBinding::new("pageup", PageUp, None),
            KeyBinding::new("pagedown", PageDown, None),
            KeyBinding::new("cmd-q", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());

        trace("keys bound");
        let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
        let large = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0)
            > editor::HIGHLIGHT_LIMIT as u64;
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("GPUI spike (PROTOTYPE)".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |_, cx| {
                    trace("window built, creating editor");
                    cx.new(|cx| {
                        let mut editor = Editor::new(cx);
                        if large {
                            editor.open_in_background(&file, cx);
                        } else {
                            editor.open(&file, cx);
                        }
                        editor
                    })
                },
            )
            .unwrap();
        trace("open_window returned");
        let editor = window
            .update(cx, |editor, window, cx| {
                window.focus(&editor.focus_handle, cx);
                cx.entity()
            })
            .unwrap();
        cx.activate(true);
        trace("activated");

        if let Some((name, out)) = bench {
            cx.spawn(async move |cx| {
                let result = bench::run(&name, recorder.as_ref(), editor, cx).await;
                let text = serde_json::to_string_pretty(&result).unwrap();
                match out {
                    Some(path) => std::fs::write(path, text).unwrap(),
                    None => println!("{text}"),
                }
                drop(recorder.map(metrics::Recorder::finish));
                cx.update(|cx| cx.quit());
            })
            .detach();
        }
    });
}
