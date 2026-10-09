//! Undo and redo: typing groups into steps, each step restores selections
//! (ticket #22). Grouping is driven by the test host's manual clock.

use std::{ops::Range, time::Duration};

use genea_core::{CaretMove::*, Command, EditorView, ProjectId, Workbench};
use genea_testkit::{FixtureProject, TestHost};

/// Past the ~1 s pause that ends a typing step.
const PAUSE: Duration = Duration::from_millis(1500);
/// Well inside a typing step.
const KEYSTROKE: Duration = Duration::from_millis(100);

struct Editing {
    fixture: FixtureProject,
    host: TestHost,
    workbench: Workbench,
    project: ProjectId,
}

fn open(text: &str) -> Editing {
    let fixture = FixtureProject::new().file("file.ts", text).build();
    let host = TestHost::new();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::SetViewport { rows: 10.0 });
    workbench.dispatch(project, Command::OpenFile("file.ts".into()));
    workbench.settle().unwrap();
    Editing { fixture, host, workbench, project }
}

impl Editing {
    fn run(&mut self, commands: impl IntoIterator<Item = Command>) {
        for command in commands {
            self.workbench.dispatch(self.project, command);
        }
    }

    /// Types `text` one character at a time, a keystroke apart.
    fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.run([Command::InsertText(c.to_string())]);
            self.host.clock().advance(KEYSTROKE);
        }
    }

    fn pause(&self) {
        self.host.clock().advance(PAUSE);
    }

    fn editor(&self) -> EditorView {
        self.workbench.project(self.project).unwrap().editor.unwrap()
    }

    /// The visible lines' grid text.
    fn lines(&self) -> Vec<String> {
        self.editor().lines.into_iter().map(|l| l.text).collect()
    }

    /// The caret as (line, column) and the selected columns of each visible
    /// line.
    fn selection(&self) -> ((usize, usize), Vec<Vec<Range<usize>>>) {
        let editor = self.editor();
        ((editor.caret.line, editor.caret.column), editor.lines.into_iter().map(|l| l.selections).collect())
    }

    fn modified(&self) -> bool {
        self.editor().modified
    }
}

#[test]
fn a_pause_in_typing_ends_an_undo_step() {
    let mut editing = open("\n");

    editing.type_text("hello");
    editing.pause();
    editing.type_text(" world");

    editing.run([Command::Undo]);
    assert_eq!(editing.lines(), ["hello", ""]);
    editing.run([Command::Undo]);
    assert_eq!(editing.lines(), ["", ""]);
}

#[test]
fn typing_without_a_pause_is_one_undo_step() {
    let mut editing = open("\n");

    editing.type_text("hello world");

    editing.run([Command::Undo]);
    assert_eq!(editing.lines(), ["", ""]);
}

#[test]
fn a_caret_jump_ends_an_undo_step() {
    let mut editing = open("ab\n");

    editing.run([Command::MoveCaret(LineEnd)]);
    editing.type_text("1");
    editing.run([Command::MoveCaret(LineStart)]);
    editing.type_text("2");
    assert_eq!(editing.lines(), ["2ab1", ""]);

    editing.run([Command::Undo]);
    assert_eq!(editing.lines(), ["ab1", ""]);
}

#[test]
fn switching_from_typing_to_deleting_ends_an_undo_step() {
    let mut editing = open("\n");

    editing.type_text("abc");
    editing.run([Command::Delete(Left)]);
    editing.type_text("d");
    assert_eq!(editing.lines(), ["abd", ""]);

    editing.run([Command::Undo]);
    assert_eq!(editing.lines(), ["ab", ""], "undoes the typing after the Backspace");
    editing.run([Command::Undo]);
    assert_eq!(editing.lines(), ["abc", ""], "undoes the Backspace");
    editing.run([Command::Undo]);
    assert_eq!(editing.lines(), ["", ""], "undoes the first typing");
}

#[test]
fn backspacing_without_a_pause_is_one_undo_step() {
    let mut editing = open("hello\n");

    editing.run([Command::MoveCaret(LineEnd)]);
    for _ in 0..3 {
        editing.run([Command::Delete(Left)]);
        editing.host.clock().advance(KEYSTROKE);
    }
    assert_eq!(editing.lines(), ["he", ""]);

    editing.run([Command::Undo]);
    assert_eq!(editing.lines(), ["hello", ""]);
}

#[test]
fn undo_and_redo_restore_the_selection_from_before_and_after_a_step() {
    let mut editing = open("hello world\n");

    editing.run([Command::SelectWord { line: 0, column: 8 }]);
    editing.type_text("there");

    editing.run([Command::Undo]);
    assert_eq!(editing.lines(), ["hello world", ""]);
    assert_eq!(editing.selection(), ((0, 11), vec![vec![6..11], vec![]]), "\"world\" is selected again");

    editing.run([Command::Redo]);
    assert_eq!(editing.lines(), ["hello there", ""]);
    assert_eq!(editing.selection(), ((0, 11), vec![vec![], vec![]]), "the caret is after \"there\"");
}

#[test]
fn undo_puts_the_caret_back_where_a_backspace_started() {
    let mut editing = open("abc\n");

    editing.run([Command::MoveCaret(LineEnd), Command::Delete(Left), Command::Delete(Left)]);
    editing.run([Command::MoveCaret(LineStart)]);

    editing.run([Command::Undo]);
    assert_eq!(editing.lines(), ["abc", ""]);
    assert_eq!(editing.selection(), ((0, 3), vec![vec![], vec![]]));
}

#[test]
fn redo_steps_forward_through_undone_steps_in_order() {
    let mut editing = open("\n");
    editing.type_text("one");
    editing.pause();
    editing.type_text(" two");

    editing.run([Command::Undo, Command::Undo]);
    assert_eq!(editing.lines(), ["", ""]);

    editing.run([Command::Redo]);
    assert_eq!(editing.lines(), ["one", ""]);
    editing.run([Command::Redo]);
    assert_eq!(editing.lines(), ["one two", ""]);
    editing.run([Command::Redo]);
    assert_eq!(editing.lines(), ["one two", ""], "nothing more to redo");
}

#[test]
fn an_edit_after_undo_drops_what_could_be_redone() {
    let mut editing = open("\n");
    editing.type_text("one");
    editing.pause();
    editing.type_text(" two");

    editing.run([Command::Undo]);
    editing.type_text("!");
    editing.run([Command::Redo]);

    assert_eq!(editing.lines(), ["one!", ""]);
}

#[test]
fn typing_right_after_an_undo_is_a_step_of_its_own() {
    let mut editing = open("\n");
    editing.type_text("ab");
    editing.run([Command::Delete(Left)]);

    editing.run([Command::Undo]);
    editing.type_text("c");
    editing.run([Command::Undo]);

    assert_eq!(editing.lines(), ["ab", ""], "the typing before the undo is a separate step");
}

#[test]
fn undo_with_no_history_does_nothing() {
    let mut editing = open("abc\n");

    editing.run([Command::Undo, Command::Redo]);

    assert_eq!(editing.lines(), ["abc", ""]);
    assert!(!editing.modified());
}

#[test]
fn paste_is_an_undo_step_of_its_own() {
    let mut editing = open("\n");
    editing.host.clipboard().set_text("PASTED");

    editing.type_text("a");
    editing.run([Command::Paste]);
    editing.type_text("b");
    assert_eq!(editing.lines(), ["aPASTEDb", ""]);

    editing.run([Command::Undo]);
    assert_eq!(editing.lines(), ["aPASTED", ""]);
    editing.run([Command::Undo]);
    assert_eq!(editing.lines(), ["a", ""]);
}

#[test]
fn a_multi_line_paste_undoes_in_one_step() {
    let mut editing = open("x\n");
    editing.host.clipboard().set_text("one\ntwo\n");

    editing.run([Command::Paste, Command::Undo]);

    assert_eq!(editing.lines(), ["x", ""]);
}

#[test]
fn undo_history_survives_saving() {
    let mut editing = open("\n");
    editing.type_text("hello");

    editing.run([Command::Save]);
    editing.workbench.settle().unwrap();
    assert_eq!(editing.fixture.read("file.ts"), "hello\n");

    editing.run([Command::Undo]);
    assert_eq!(editing.lines(), ["", ""]);
}

#[test]
fn undoing_back_to_the_saved_text_shows_the_file_as_saved() {
    let mut editing = open("\n");
    editing.type_text("one");
    editing.run([Command::Save]);
    editing.workbench.settle().unwrap();
    editing.pause();
    editing.type_text(" two");
    assert!(editing.modified());

    editing.run([Command::Undo]);
    assert!(!editing.modified(), "the text is what is on disk");

    editing.run([Command::Undo]);
    assert!(editing.modified(), "undone past the save");

    editing.run([Command::Redo]);
    assert!(!editing.modified(), "redone back to the save");
}

#[test]
fn retyping_the_saved_text_after_an_undo_still_shows_the_file_as_unsaved() {
    let mut editing = open("\n");
    editing.type_text("a");
    editing.run([Command::Save]);
    editing.workbench.settle().unwrap();

    editing.run([Command::Undo]);
    editing.type_text("b");

    assert!(editing.modified());
}
