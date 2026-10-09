//! PROTOTYPE: throwaway Slint validation spike for "Does Slint meet Genea's
//! budgets?" (prudentmildew/genea#15). Not Genea code; never merge.
//!
//!   slint-spike [FILE]                      open FILE to judge typing/scrolling feel
//!   slint-spike bench <NAME> [--out FILE]   run one in-process benchmark
//!   slint-spike suite --label LABEL         run every benchmark in child processes

mod bench;
mod editor;
mod metrics;
mod prewarm;
mod suite;
mod sys;
mod synth;
mod syntax;

use std::{path::PathBuf, time::Duration};

use slint::{ComponentHandle, RenderingState, Timer, TimerMode, winit_030::WinitWindowAccessor};

slint::include_modules!();

pub fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures").join(name)
}

fn trace(what: &str) {
    if std::env::var("SPIKE_TRACE").is_ok() {
        let fp = sys::usage(std::process::id() as i32).map_or(0.0, |u| u.footprint_mb);
        eprintln!("{:>8.2} ms  {fp:>6.1} MB  {what}", sys::since_process_start(std::time::Instant::now()).as_secs_f64() * 1000.0);
    }
}

/// The one window and its editor, owned by the main thread.
pub mod app {
    use std::cell::{OnceCell, RefCell};

    use crate::{EditorWindow, editor::Editor};

    thread_local! {
        static APP: OnceCell<(EditorWindow, RefCell<Editor>)> = const { OnceCell::new() };
    }

    pub fn init(window: EditorWindow, editor: Editor) {
        APP.with(|app| app.set((window, RefCell::new(editor))).ok().unwrap());
    }

    pub fn with<R>(f: impl FnOnce(&mut Editor, &EditorWindow) -> R) -> R {
        APP.with(|app| {
            let (window, editor) = app.get().unwrap();
            f(&mut editor.borrow_mut(), window)
        })
    }
}

fn main() {
    trace("main");
    prewarm::start();
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
    // First, so the journal sees startup too. Not for the idle bench or
    // interactive use: its observers would add work to every wake-up.
    if bench.as_ref().is_some_and(|(name, _)| name != "idle") {
        metrics::start();
    }
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("skia".into())
        .select()
        .unwrap();
    trace("backend selected");

    let window = EditorWindow::new().unwrap();
    trace("window component built");
    // PROTOTYPE (#18): SPIKE_WINDOW=WxH in points, to see what scales with window size.
    if let Some((w, h)) = std::env::var("SPIKE_WINDOW").ok().and_then(|v| {
        let (w, h) = v.split_once('x')?;
        Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?))
    }) {
        window.window().set_size(slint::LogicalSize::new(w, h));
    }
    let editor = editor::Editor::new(&window);
    app::init(window.clone_strong(), editor);

    window.on_key_pressed(|event| {
        if app::with(|editor, window| editor.key(&event, window)) {
            slint::private_unstable_api::re_exports::EventResult::Accept
        } else {
            slint::private_unstable_api::re_exports::EventResult::Reject
        }
    });
    window.on_scrolled(|delta_y| app::with(|editor, window| editor.scroll_by(-delta_y, window)));
    window.on_viewport_changed(|| app::with(|editor, window| editor.sync(window)));

    window.window().on_winit_window_event(|_, event| {
        if let slint::winit_030::winit::event::WindowEvent::Occluded(false) = event {
            metrics::mark_visible();
            trace("visible");
        }
        if let slint::winit_030::winit::event::WindowEvent::KeyboardInput { event, .. } = event
            && event.state.is_pressed()
        {
            metrics::mark_key();
        }
        slint::winit_030::EventResult::Propagate
    });
    if metrics::recording() {
        window
            .window()
            .set_rendering_notifier(|state, _| match state {
                RenderingState::BeforeRendering => {
                    metrics::mark_before_rendering();
                    trace("before rendering");
                }
                RenderingState::AfterRendering => {
                    metrics::mark_after_rendering();
                    trace("after rendering");
                    // Benchmark auto-scroll: one step per rendered frame.
                    Timer::single_shot(Duration::ZERO, || {
                        app::with(|editor, window| editor.auto_scroll_step(window))
                    });
                }
                _ => {}
            })
            .unwrap();
    }

    // PROTOTYPE (#18): SPIKE_FILE overrides the benchmark's file.
    let file = std::env::var_os("SPIKE_FILE").map(PathBuf::from).unwrap_or(file);
    let large = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0)
        > editor::HIGHLIGHT_LIMIT as u64;
    app::with(|editor, window| {
        if large {
            editor.open_in_background(&file, window);
        } else {
            editor.open(&file, window);
        }
    });
    trace("file opened");

    let blink = std::rc::Rc::new(Timer::default());
    if std::env::var_os("SPIKE_NO_BLINK").is_none() {
        blink.start(TimerMode::Repeated, Duration::from_millis(530), || {
            app::with(|editor, window| editor.blink(window))
        });
    }
    // PROTOTYPE (#18): SPIKE_BLINK_STOP_AFTER=S stops blinking (caret left on) after S seconds,
    // standing in for "stop blinking after S seconds without input".
    let blink_stop = Timer::default();
    if let Some(secs) = std::env::var("SPIKE_BLINK_STOP_AFTER").ok().and_then(|v| v.parse::<f64>().ok()) {
        let blink = blink.clone();
        blink_stop.start(TimerMode::SingleShot, Duration::from_secs_f64(secs), move || {
            blink.stop();
            app::with(|editor, window| editor.show_caret(window));
            trace("blink stopped");
        });
    }

    if let Some((name, out)) = bench {
        slint::spawn_local(async move {
            let result = bench::run(&name).await;
            let text = serde_json::to_string_pretty(&result).unwrap();
            match out {
                Some(path) => std::fs::write(path, text).unwrap(),
                None => println!("{text}"),
            }
            slint::quit_event_loop().unwrap();
        })
        .unwrap();
    }

    window.show().unwrap();
    trace("shown");
    window.window().with_winit_window(|w| w.focus_window());
    if std::env::var_os("SPIKE_TWO_DRAWABLES").is_some() {
        // Experiment: wgpu asks for 3 window-sized drawables; try 2.
        let w = window.as_weak();
        Timer::single_shot(Duration::from_millis(50), move || {
            if let Some(w) = w.upgrade() {
                w.window().with_winit_window(synth::two_drawables);
            }
        });
    }
    slint::run_event_loop().unwrap();
}
