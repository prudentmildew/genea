//! PROTOTYPE. In-process benchmarks. Each drives the editor, then reads
//! GPUI's foreground journal for the window it measured.

use std::time::{Duration, Instant};

use gpui::{AsyncApp, Entity, profiler::journal::ForegroundEvent};
use serde_json::{Value, json};

use crate::{
    editor::Editor,
    fixture,
    metrics::{Recorder, ms, stats},
    synth, sys,
};

pub fn file_for(bench: &str) -> &'static str {
    match bench {
        "start" | "idle" | "open" => "typical.ts",
        _ => "checker.ts",
    }
}

pub async fn run(
    name: &str,
    recorder: Option<&Recorder>,
    editor: Entity<Editor>,
    cx: &mut AsyncApp,
) -> Value {
    let mut result = bench(name, recorder, editor, cx).await;
    if let Value::Object(map) = &mut result {
        map.insert("window_visible_at_end".into(), synth::window_visible().into());
    }
    result
}

async fn bench(
    name: &str,
    recorder: Option<&Recorder>,
    editor: Entity<Editor>,
    cx: &mut AsyncApp,
) -> Value {
    match name {
        "start" => start(recorder.unwrap(), cx).await,
        "typing" => typing(recorder.unwrap(), &editor, cx).await,
        "scroll" => scroll(recorder.unwrap(), &editor, cx).await,
        "open" => open(recorder.unwrap(), &editor, cx).await,
        "huge" => huge(recorder.unwrap(), &editor, cx).await,
        "idle" => {
            let seconds = std::env::var("SPIKE_IDLE_SECONDS").map_or(45, |s| s.parse().unwrap());
            sleep(cx, Duration::from_secs(seconds)).await;
            json!({})
        }
        _ => panic!("unknown bench {name}"),
    }
}

async fn sleep(cx: &mut AsyncApp, d: Duration) {
    cx.background_executor().timer(d).await;
}

async fn wait_for(cx: &mut AsyncApp, editor: &Entity<Editor>, f: impl Fn(&Editor) -> bool) {
    while !editor.read_with(cx, |e, _| f(e)) {
        sleep(cx, Duration::from_millis(5)).await;
    }
}

/// Process start (kernel's record) to the first frame presented, which
/// already shows the restored file.
async fn start(recorder: &Recorder, cx: &mut AsyncApp) -> Value {
    // On the efficiency cores the first frame can take longer than 500 ms.
    let deadline = Instant::now() + Duration::from_secs(10);
    let journal = loop {
        sleep(cx, Duration::from_millis(100)).await;
        let journal = recorder.journal();
        if journal.first_present().is_some() || Instant::now() > deadline {
            break journal;
        }
    };
    let first = journal.first_present().expect("a presented frame");
    let at = first.presentation.present_end;
    json!({
        "start_to_first_frame_ms": ms(sys::since_process_start(at)),
        "main_to_first_frame_ms": ms(at.duration_since(recorder.base)),
        "before_first_frame": journal.longest_stall((recorder.base, at)),
        "first_draw_start_ms": ms(first.frame.draw_start.duration_since(recorder.base)),
        "first_dirty_ms": first.frame.dirty_at.map(|d| ms(d.duration_since(recorder.base))),
    })
}

/// Types into the middle of checker.ts at ~25 keys/s through the real AppKit
/// input path, with the syntax tree reparsed after every edit.
async fn typing(recorder: &Recorder, editor: &Entity<Editor>, cx: &mut AsyncApp) -> Value {
    wait_for(cx, editor, |e| e.syntax.is_some()).await;
    editor.update(cx, |e, cx| e.move_cursor_to_line(27_000, cx));
    sleep(cx, Duration::from_millis(500)).await;
    editor.update(cx, |e, _| e.edit_log.clear());

    let text: Vec<char> = "let total be the sum of parts and keep going ".chars().collect();
    let begin = Instant::now();
    for i in 0..400 {
        let key = if i % 23 == 22 {
            synth::RETURN
        } else if i % 11 == 10 {
            synth::BACKSPACE
        } else {
            synth::letter(text[i % text.len()])
        };
        synth::press(key);
        sleep(cx, Duration::from_millis(40)).await;
    }
    sleep(cx, Duration::from_millis(300)).await;
    let end = Instant::now();

    let journal = recorder.journal();
    let edits = editor.read_with(cx, |e, _| e.edit_log.clone());
    let presents = journal.presents();
    let [mut total, mut work, mut work_async, mut frame, mut wait, mut edit, mut reparse] =
        std::array::from_fn::<Vec<Duration>, 7, _>(|_| Vec::new());
    let (mut draw, mut present) = (Vec::new(), Vec::new());
    let keys = journal.events((begin, end)).into_iter().filter_map(|e| match e {
        ForegroundEvent::Input(i) if i.kind == "key_down" => Some(i),
        _ => None,
    });
    for key in keys {
        let Some(&(edit_start, edited, reparsed)) =
            edits.iter().find(|(s, _, _)| *s >= key.start)
        else {
            continue;
        };
        let Some(p) = presents.iter().find(|p| p.frame.draw_start >= reparsed) else {
            continue;
        };
        let gpui_frame = p.presentation.present_end - p.frame.draw_start;
        total.push(p.presentation.present_end - key.start);
        work.push((reparsed - key.start) + gpui_frame);
        work_async.push((edited - key.start) + gpui_frame);
        frame.push(gpui_frame);
        draw.push(p.frame.draw_duration());
        present.push(p.presentation.present_end - p.presentation.present_start);
        wait.push(p.frame.draw_start - reparsed);
        edit.push(edited - edit_start);
        reparse.push(reparsed - edited);
    }
    json!({
        "keys": total.len(),
        "input_to_present_ms": stats(&mut total),
        "genea_work_ms": stats(&mut work),
        "genea_work_if_reparse_async_ms": stats(&mut work_async),
        "gpui_draw_plus_present_ms": stats(&mut frame),
        "gpui_draw_ms": stats(&mut draw),
        "gpui_present_ms": stats(&mut present),
        "wait_for_frame_ms": stats(&mut wait),
        "rope_edit_ms": stats(&mut edit),
        "treesitter_reparse_ms": stats(&mut reparse),
        "longest_stall": journal.longest_stall((begin, end)),
        "lost_journal_entries": journal.lost,
    })
}

async fn scroll_phase(
    recorder: &Recorder,
    editor: &Entity<Editor>,
    step: f32,
    seconds: u64,
    cx: &mut AsyncApp,
) -> Value {
    let begin = Instant::now();
    let until = begin + Duration::from_secs(seconds);
    editor.update(cx, |e, cx| {
        e.auto_scroll = Some((step, until));
        cx.notify();
    });
    sleep(cx, Duration::from_secs(seconds)).await;
    let end = Instant::now();
    sleep(cx, Duration::from_millis(200)).await;
    let journal = recorder.journal();
    json!({
        "px_per_frame": step,
        "frames": journal.frames((begin, end)),
        "longest_stall": journal.longest_stall((begin, end)),
    })
}

async fn scroll(
    recorder: &Recorder,
    editor: &Entity<Editor>,
    cx: &mut AsyncApp,
) -> Value {
    wait_for(cx, editor, |e| e.syntax.is_some()).await;
    sleep(cx, Duration::from_millis(500)).await;
    // ~3,600 px/s: a brisk continuous scroll.
    let steady = scroll_phase(recorder, editor, 30., 8, cx).await;
    // ~24,000 px/s: every frame is all-new lines, so nothing is cached.
    let fling = scroll_phase(recorder, editor, 200., 4, cx).await;
    json!({ "steady": steady, "fling": fling })
}

/// Opens a ~1 MB file 20 times; time from the open to the first frame
/// showing it.
async fn open(recorder: &Recorder, editor: &Entity<Editor>, cx: &mut AsyncApp) -> Value {
    sleep(cx, Duration::from_millis(500)).await;
    let files = [fixture("onemb.ts"), fixture("onemb-b.ts")];
    let begin = Instant::now();
    let mut starts = Vec::new();
    for i in 0..20 {
        let file = files[i % 2].clone();
        starts.push(Instant::now());
        editor.update(cx, |e, cx| e.open(&file, cx));
        sleep(cx, Duration::from_millis(400)).await;
    }
    let end = Instant::now();
    let journal = recorder.journal();
    let presents = journal.presents();
    let mut latencies: Vec<Duration> = starts
        .iter()
        .filter_map(|t| {
            presents
                .iter()
                .find(|p| p.frame.dirty_at.is_some_and(|d| d >= *t))
                .map(|p| p.presentation.present_end.duration_since(*t))
        })
        .collect();
    json!({
        "open_to_first_frame_ms": stats(&mut latencies),
        "longest_stall": journal.longest_stall((begin, end)),
    })
}

/// Opens the ~100 MB file: rope built off the main thread, no highlighting.
async fn huge(
    recorder: &Recorder,
    editor: &Entity<Editor>,
    cx: &mut AsyncApp,
) -> Value {
    // Started with checker.ts; switch to the huge file.
    wait_for(cx, editor, |e| !e.loading).await;
    sleep(cx, Duration::from_millis(500)).await;
    let begin = Instant::now();
    let generation = editor.read_with(cx, |e, _| e.generation);
    let path = fixture("huge.ts");
    editor.update(cx, |e, cx| e.open_in_background(&path, cx));
    wait_for(cx, editor, |e| e.generation > generation).await;
    let loaded = Instant::now();
    sleep(cx, Duration::from_millis(300)).await;
    let journal = recorder.journal();
    let first_screen = journal
        .presents()
        .into_iter()
        .find(|p| p.presentation.present_end > loaded)
        .map(|p| ms(p.presentation.present_end.duration_since(begin)));
    let open_end = Instant::now();
    let open_stall = journal.longest_stall((begin, open_end));

    // Scroll through it, then jump to the end and back.
    let scrolling = scroll_phase(recorder, editor, 30., 4, cx).await;
    let jump_begin = Instant::now();
    editor.update(cx, |e, cx| {
        let max = e.max_scroll();
        e.scroll_by(max, cx)
    });
    sleep(cx, Duration::from_millis(300)).await;
    editor.update(cx, |e, cx| {
        let top = e.scroll_top;
        e.scroll_by(-top, cx)
    });
    sleep(cx, Duration::from_millis(300)).await;
    let jump_stall = recorder.journal().longest_stall((jump_begin, Instant::now()));

    let footprint = sys::usage(std::process::id() as i32).map(|u| u.footprint_mb);
    json!({
        "bytes": std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
        "open_to_first_screen_ms": first_screen,
        "rope_built_ms": ms(loaded.duration_since(begin)),
        "longest_stall_while_opening": open_stall,
        "scrolling": scrolling,
        "longest_stall_jump_to_end": jump_stall,
        "footprint_mb_with_file": footprint,
    })
}
