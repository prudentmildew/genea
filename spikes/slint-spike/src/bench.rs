//! PROTOTYPE. In-process benchmarks, the same as the GPUI spike's: each
//! drives the editor, then reads the spike's journal (`metrics`) for the
//! window it measured.

use std::{
    cell::RefCell,
    rc::Rc,
    task::{Poll, Waker},
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::{
    app,
    editor::Editor,
    fixture,
    metrics::{self, ms, stats},
    synth, sys,
};

pub fn file_for(bench: &str) -> &'static str {
    match bench {
        "start" | "idle" | "open" => "typical.ts",
        _ => "checker.ts",
    }
}

pub async fn run(name: &str) -> Value {
    let mut result = bench(name).await;
    if let Value::Object(map) = &mut result {
        map.insert("window_visible_at_end".into(), synth::window_visible().into());
    }
    result
}

async fn bench(name: &str) -> Value {
    match name {
        "start" => start().await,
        "typing" => typing().await,
        "scroll" => scroll().await,
        "open" => open().await,
        "huge" => huge().await,
        "idle" => {
            let seconds = std::env::var("SPIKE_IDLE_SECONDS").map_or(45, |s| s.parse().unwrap());
            sleep(Duration::from_secs(seconds)).await;
            json!({})
        }
        _ => panic!("unknown bench {name}"),
    }
}

/// A Slint timer as a future.
pub async fn sleep(d: Duration) {
    let state: Rc<RefCell<(bool, Option<Waker>)>> = Rc::default();
    slint::Timer::single_shot(d, {
        let state = state.clone();
        move || {
            let mut s = state.borrow_mut();
            s.0 = true;
            if let Some(w) = s.1.take() {
                w.wake();
            }
        }
    });
    std::future::poll_fn(|cx| {
        let mut s = state.borrow_mut();
        if s.0 {
            Poll::Ready(())
        } else {
            s.1 = Some(cx.waker().clone());
            Poll::Pending
        }
    })
    .await
}

fn read<R>(f: impl FnOnce(&Editor) -> R) -> R {
    app::with(|e, _| f(e))
}

async fn wait_for(f: impl Fn(&Editor) -> bool) {
    while !read(&f) {
        sleep(Duration::from_millis(5)).await;
    }
}

/// Process start (kernel's record) to the end of the first frame, which
/// already shows the restored file.
async fn start() -> Value {
    // On the efficiency cores the first frame can take longer than 500 ms.
    let deadline = Instant::now() + Duration::from_secs(10);
    let journal = loop {
        sleep(Duration::from_millis(100)).await;
        let journal = metrics::journal();
        if journal.first_frame().is_some() || Instant::now() > deadline {
            break journal;
        }
    };
    let first = journal.first_frame().expect("a rendered frame");
    let base = metrics::base();
    // Content is on screen once both the frame is presented and the window is visible.
    while metrics::visible().is_none() && Instant::now() < deadline {
        sleep(Duration::from_millis(10)).await;
    }
    let visible = metrics::visible().unwrap_or(first.end);
    json!({
        "start_to_visible_ms": ms(sys::since_process_start(visible)),
        "start_to_content_visible_ms": ms(sys::since_process_start(visible.max(first.end))),
        "start_to_first_frame_ms": ms(sys::since_process_start(first.end)),
        "main_to_first_frame_ms": ms(first.end - base),
        "before_first_frame": journal.longest_stall((base, first.end)),
        "first_frame_start_ms": ms(first.start - base),
        "first_before_rendering_ms": ms(first.before - base),
        "first_draw_ms": ms(first.draw()),
    })
}

/// Types into the middle of checker.ts at ~25 keys/s through the real AppKit
/// input path, with the syntax tree reparsed after every edit.
async fn typing() -> Value {
    wait_for(|e| e.syntax.is_some()).await;
    app::with(|e, w| e.move_cursor_to_line(27_000, w));
    sleep(Duration::from_millis(500)).await;
    app::with(|e, _| e.edit_log.clear());

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
        sleep(Duration::from_millis(40)).await;
    }
    sleep(Duration::from_millis(300)).await;
    let end = Instant::now();

    let journal = metrics::journal();
    let edits = read(|e| e.edit_log.clone());
    let [mut total, mut work, mut work_async, mut frame, mut wait, mut edit, mut reparse, mut sync] =
        std::array::from_fn::<Vec<Duration>, 8, _>(|_| Vec::new());
    let (mut draw, mut present, mut acquire) = (Vec::new(), Vec::new(), Vec::new());
    for &key in journal.keys.iter().filter(|k| **k >= begin && **k <= end) {
        let Some(&(edit_start, edited, reparsed, synced)) = edits.iter().find(|(s, ..)| *s >= key) else {
            continue;
        };
        let Some(f) = journal.frames.iter().find(|f| f.before >= synced) else {
            continue;
        };
        // The frame's own segment; if it began before the edit finished (same
        // run-loop pass), count it from the end of the edit.
        let start = f.start.max(synced);
        let slint_frame = f.end - start;
        total.push(f.end - key);
        work.push((synced - key) + slint_frame);
        work_async.push((edited - key) + (synced - reparsed) + slint_frame);
        frame.push(slint_frame);
        acquire.push(f.before - start);
        draw.push(f.draw());
        present.push(f.end - f.after);
        wait.push(start - synced);
        edit.push(edited - edit_start);
        reparse.push(reparsed - edited);
        sync.push(synced - reparsed);
    }
    json!({
        "keys": total.len(),
        "input_to_present_ms": stats(&mut total),
        "genea_work_ms": stats(&mut work),
        "genea_work_if_reparse_async_ms": stats(&mut work_async),
        "slint_frame_ms": stats(&mut frame),
        "slint_tick_to_before_rendering_ms": stats(&mut acquire),
        "slint_draw_ms": stats(&mut draw),
        "slint_flush_present_ms": stats(&mut present),
        "wait_for_frame_ms": stats(&mut wait),
        "rope_edit_ms": stats(&mut edit),
        "treesitter_reparse_ms": stats(&mut reparse),
        "view_sync_ms": stats(&mut sync),
        "longest_stall": journal.longest_stall((begin, end)),
    })
}

async fn scroll_phase(step: f32, seconds: u64) -> Value {
    let begin = Instant::now();
    let until = begin + Duration::from_secs(seconds);
    app::with(|e, w| {
        e.scroll_sync_log.clear();
        e.auto_scroll = Some((step, until));
        e.auto_scroll_step(w);
    });
    sleep(Duration::from_secs(seconds)).await;
    let end = Instant::now();
    sleep(Duration::from_millis(200)).await;
    let journal = metrics::journal();
    // The view sync for the next frame runs in its own run-loop pass, right
    // after the previous frame; GPUI did the same work inside its draw.
    let mut sync = read(|e| e.scroll_sync_log.clone());
    json!({
        "px_per_frame": step,
        "frames": journal.frame_stats((begin, end)),
        "view_sync_ms": stats(&mut sync),
        "longest_stall": journal.longest_stall((begin, end)),
    })
}

async fn scroll() -> Value {
    wait_for(|e| e.syntax.is_some()).await;
    sleep(Duration::from_millis(500)).await;
    // ~3,600 px/s: a brisk continuous scroll.
    let steady = scroll_phase(30., 8).await;
    // ~24,000 px/s: every frame is all-new lines, so nothing is cached.
    let fling = scroll_phase(200., 4).await;
    json!({ "steady": steady, "fling": fling })
}

/// Opens a ~1 MB file 20 times; time from the open to the end of the first
/// frame showing it.
async fn open() -> Value {
    sleep(Duration::from_millis(500)).await;
    let files = [fixture("onemb.ts"), fixture("onemb-b.ts")];
    let begin = Instant::now();
    let mut opens = Vec::new();
    for i in 0..20 {
        let file = files[i % 2].clone();
        let t = Instant::now();
        app::with(|e, w| e.open(&file, w));
        opens.push((t, read(|e| e.text_set_at.unwrap())));
        sleep(Duration::from_millis(400)).await;
    }
    let end = Instant::now();
    let journal = metrics::journal();
    let mut latencies: Vec<Duration> = opens
        .iter()
        .filter_map(|(t, set)| {
            journal
                .frames
                .iter()
                .find(|f| f.before >= *set)
                .map(|f| f.end - *t)
        })
        .collect();
    json!({
        "open_to_first_frame_ms": stats(&mut latencies),
        "longest_stall": journal.longest_stall((begin, end)),
    })
}

/// Opens the ~100 MB file: rope built off the main thread, no highlighting.
async fn huge() -> Value {
    // Started with checker.ts; switch to the huge file.
    wait_for(|e| !e.loading).await;
    sleep(Duration::from_millis(500)).await;
    let begin = Instant::now();
    let generation = read(|e| e.generation);
    let path = fixture("huge.ts");
    app::with(|e, w| e.open_in_background(&path, w));
    wait_for(|e| e.generation > generation).await;
    let loaded = read(|e| e.text_set_at.unwrap());
    sleep(Duration::from_millis(300)).await;
    let journal = metrics::journal();
    let first_screen = journal
        .frames
        .iter()
        .find(|f| f.before >= loaded)
        .map(|f| ms(f.end - begin));
    let open_end = Instant::now();
    let open_stall = journal.longest_stall((begin, open_end));

    // Scroll through it, then jump to the end and back.
    let scrolling = scroll_phase(30., 4).await;
    let jump_begin = Instant::now();
    app::with(|e, w| {
        let max = e.max_scroll();
        e.scroll_by(max, w)
    });
    sleep(Duration::from_millis(300)).await;
    app::with(|e, w| {
        let top = e.scroll_top;
        e.scroll_by(-top, w)
    });
    sleep(Duration::from_millis(300)).await;
    let jump_stall = metrics::journal().longest_stall((jump_begin, Instant::now()));

    let footprint = sys::usage(std::process::id() as i32).map(|u| u.footprint_mb);
    json!({
        "bytes": std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
        "open_to_first_screen_ms": first_screen,
        "rope_built_ms": ms(loaded - begin),
        "longest_stall_while_opening": open_stall,
        "scrolling": scrolling,
        "longest_stall_jump_to_end": jump_stall,
        "footprint_mb_with_file": footprint,
    })
}
