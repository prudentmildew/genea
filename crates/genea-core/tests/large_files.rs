//! Large files (ticket #27): a file over 5 MB opens with no syntax
//! highlighting and no language intelligence, and the status bar says why.

use std::{
    fs::{File, OpenOptions},
    io::Write,
    sync::mpsc::{Receiver, channel},
    time::{Duration, Instant},
};

use genea_core::{Caret, Command, EditorView, ProjectId, Workbench};
use genea_testkit::{FixtureProject, TestHost};

const MB: usize = 1024 * 1024;

/// A highlightable line of TypeScript, 41 bytes with its line break.
const LINE: &str = "const greeting: string = \"hi\"; // hello\n";

/// TypeScript of exactly `bytes` bytes: whole copies of [`LINE`], padded
/// with a comment line at the end.
fn typescript(bytes: usize) -> String {
    let mut text = LINE.repeat(bytes / LINE.len());
    let rest = bytes - text.len();
    if rest > 0 {
        text.push_str(&"/".repeat(rest - 1));
        text.push('\n');
    }
    assert_eq!(text.len(), bytes);
    text
}

fn open_file(name: &str, text: &str) -> (FixtureProject, Workbench, ProjectId) {
    let fixture = FixtureProject::new().file(name, text).build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::SetViewport { rows: 20.0 });
    workbench.dispatch(project, Command::OpenFile(name.into()));
    workbench.settle().unwrap();
    (fixture, workbench, project)
}

#[test]
fn a_file_over_5_mb_opens_without_highlighting_and_the_status_bar_says_why() {
    let text = typescript(5 * MB + 1);
    let (_fixture, workbench, project) = open_file("big.ts", &text);

    let view = workbench.project(project).unwrap();
    let editor = view.editor.unwrap();
    assert_eq!(editor.lines[0].text, LINE.trim_end());
    assert_eq!(editor.lines.len(), 20);
    assert_eq!(editor.line_count, text.lines().count() + 1);
    assert!(editor.lines.iter().all(|line| line.highlights.is_empty()), "no highlighting");
    assert!(!editor.read_only);
    let reason = view.status.large_file.expect("a status-bar item explains the missing highlighting");
    assert!(reason.contains("5 MB") && reason.contains("highlighting"), "{reason:?}");
}

#[test]
fn a_file_of_exactly_5_mb_is_highlighted_as_usual() {
    let (_fixture, workbench, project) = open_file("big.ts", &typescript(5 * MB));

    let view = workbench.project(project).unwrap();
    assert!(!view.editor.unwrap().lines[0].highlights.is_empty(), "highlighted");
    assert_eq!(view.status.large_file, None);
}

#[test]
fn a_large_file_can_be_edited_and_saved_and_stays_unhighlighted() {
    let text = typescript(5 * MB + 1);
    let (fixture, mut workbench, project) = open_file("big.ts", &text);

    workbench.dispatch(project, Command::SelectAll);
    workbench.dispatch(project, Command::InsertText("let small = 1;".into()));
    workbench.dispatch(project, Command::Save);
    workbench.settle().unwrap();

    assert_eq!(fixture.read("big.ts"), "let small = 1;");
    let view = workbench.project(project).unwrap();
    let editor = view.editor.unwrap();
    assert_eq!(editor.lines[0].text, "let small = 1;");
    assert!(editor.lines[0].highlights.is_empty(), "still no highlighting");
    assert!(view.status.large_file.is_some(), "still a large file until reopened");
}

#[test]
fn the_status_bar_item_follows_the_focused_tab() {
    let fixture = FixtureProject::new().file("big.ts", typescript(5 * MB + 1)).file("small.ts", LINE).build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::OpenFile("big.ts".into()));
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::OpenFile("small.ts".into()));
    workbench.settle().unwrap();

    let view = workbench.project(project).unwrap();
    assert_eq!(view.status.large_file, None);
    assert!(!view.editor.unwrap().lines[0].highlights.is_empty());

    workbench.dispatch(project, Command::OpenFile("big.ts".into()));
    assert!(workbench.project(project).unwrap().status.large_file.is_some());
}

/// `count` numbered lines from `from`, each with its line break.
fn numbered(from: usize, count: usize) -> String {
    (from..from + count).map(|i| format!("line {i}\n")).collect()
}

/// A file the test streams into Genea: a pipe, so the test decides when
/// the rest of the file arrives. Its size isn't known up front, so Genea
/// treats it like a file that may well be large.
struct Stream {
    _fixture: FixtureProject,
    workbench: Workbench,
    project: ProjectId,
    writer: Option<File>,
    notified: Receiver<()>,
}

impl Stream {
    /// Opens `stream.ts` in a 20-row viewport and writes `first` into it.
    fn open(first: &[u8]) -> Stream {
        let fixture = FixtureProject::new().build();
        let pipe = fixture.path("stream.ts");
        let made = std::process::Command::new("mkfifo").arg(&pipe).status().unwrap();
        assert!(made.success());
        let mut workbench = Workbench::new(TestHost::new().shared());
        let (notify, notified) = channel();
        workbench.set_notifier(move || {
            let _ = notify.send(());
        });
        let project = workbench.open_project(fixture.root()).unwrap();
        workbench.dispatch(project, Command::SetViewport { rows: 20.0 });
        workbench.dispatch(project, Command::OpenFile("stream.ts".into()));
        // Blocks until Genea opens the pipe for reading.
        let mut writer = OpenOptions::new().write(true).open(&pipe).unwrap();
        writer.write_all(first).unwrap();
        Stream { _fixture: fixture, workbench, project, writer: Some(writer), notified }
    }

    /// Applies background results as they arrive until the project shows
    /// an editor (the stream can't settle until it is finished).
    fn wait_for_editor(&mut self) -> EditorView {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.workbench.pump();
            if let Some(editor) = self.editor() {
                return editor;
            }
            let left = deadline.checked_duration_since(Instant::now()).expect("no editor showed within 10 s");
            let _ = self.notified.recv_timeout(left);
        }
    }

    /// Writes the rest of the file, ends it and settles.
    fn finish(&mut self, rest: &[u8]) {
        let mut writer = self.writer.take().unwrap();
        writer.write_all(rest).unwrap();
        drop(writer);
        self.workbench.settle().unwrap();
    }

    fn dispatch(&mut self, command: Command) {
        self.workbench.dispatch(self.project, command);
    }

    fn editor(&self) -> Option<EditorView> {
        self.workbench.project(self.project).unwrap().editor
    }
}

#[test]
fn the_first_screen_shows_while_the_rest_of_the_file_is_still_being_read() {
    let mut stream = Stream::open(numbered(0, 1000).as_bytes());

    let editor = stream.wait_for_editor();

    let texts: Vec<&str> = editor.lines.iter().map(|line| line.text.as_str()).collect();
    assert_eq!(texts, (0..20).map(|i| format!("line {i}")).collect::<Vec<_>>());
    assert!(editor.loading, "the rest is still being read");
    assert!(editor.read_only, "no edits until the whole file is in");
    stream.dispatch(Command::InsertText("x".into()));
    assert_eq!(stream.editor().unwrap().lines[0].text, "line 0");

    stream.finish(numbered(1000, 1000).as_bytes());

    let editor = stream.editor().unwrap();
    assert_eq!(editor.line_count, 2001);
    assert!(!editor.loading);
    assert!(!editor.read_only);
    assert_eq!(editor.lines[0].text, "line 0");
    stream.dispatch(Command::InsertText("x".into()));
    assert_eq!(stream.editor().unwrap().lines[0].text, "xline 0");
}

#[test]
fn the_caret_placed_on_the_first_screen_stays_once_the_file_is_in() {
    let mut stream = Stream::open(numbered(0, 1000).as_bytes());
    stream.wait_for_editor();

    stream.dispatch(Command::PlaceCaret { line: 5, column: 3 });
    stream.dispatch(Command::ScrollBy { rows: 2.0 });
    stream.finish(numbered(1000, 1000).as_bytes());

    let editor = stream.editor().unwrap();
    assert_eq!(editor.caret, Caret { line: 5, column: 3 });
    assert_eq!(editor.scroll_top, 2.0);
}

#[test]
fn a_file_whose_rest_isnt_utf8_stays_read_only_with_a_notice() {
    let mut stream = Stream::open(numbered(0, 1000).as_bytes());
    stream.wait_for_editor();

    stream.finish(b"caf\xe9\n");

    let view = stream.workbench.project(stream.project).unwrap();
    let editor = view.editor.unwrap();
    assert!(!editor.loading);
    assert!(editor.read_only);
    assert_eq!(editor.line_count, 1002);
    assert!(view.notices.iter().any(|n| n.message.contains("isn't valid UTF-8")), "{:?}", view.notices);
}
