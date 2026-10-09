//! Multiple carets (ticket #52): the WebStorm macOS commands, edits at
//! every caret, and multi-caret edits as one undo step.

use std::{ops::Range, time::Duration};

use genea_core::{CaretMove::*, Command, EditorView, ProjectId, Workbench};
use genea_testkit::{FixtureProject, TestHost};

/// Well inside a typing step.
const KEYSTROKE: Duration = Duration::from_millis(100);

struct Editing {
    _fixture: FixtureProject,
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
    Editing { _fixture: fixture, host, workbench, project }
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

    fn editor(&self) -> EditorView {
        self.workbench.project(self.project).unwrap().editor.unwrap()
    }

    /// The visible lines' grid text.
    fn lines(&self) -> Vec<String> {
        self.editor().lines.into_iter().map(|l| l.text).collect()
    }

    /// Every caret as (line, column), top to bottom.
    fn carets(&self) -> Vec<(usize, usize)> {
        self.editor().carets.into_iter().map(|c| (c.line, c.column)).collect()
    }

    /// The primary caret as (line, column).
    fn primary(&self) -> (usize, usize) {
        let caret = self.editor().caret;
        (caret.line, caret.column)
    }

    /// The selected columns of each visible line.
    fn selections(&self) -> Vec<Vec<Range<usize>>> {
        self.editor().lines.into_iter().map(|l| l.selections).collect()
    }
}

#[test]
fn option_click_adds_a_caret_and_typing_goes_to_both() {
    let mut editing = open("one\ntwo\n");

    editing.run([Command::PlaceCaret { line: 0, column: 1 }, Command::AddCaret { line: 1, column: 2 }]);
    assert_eq!(editing.carets(), [(0, 1), (1, 2)]);
    assert_eq!(editing.primary(), (1, 2));

    editing.type_text("X");

    assert_eq!(editing.lines(), ["oXne", "twXo", ""]);
    assert_eq!(editing.carets(), [(0, 2), (1, 3)]);
}

#[test]
fn option_click_on_a_caret_removes_it_but_never_the_last_one() {
    let mut editing = open("one\ntwo\n");

    editing.run([Command::AddCaret { line: 1, column: 1 }]);
    assert_eq!(editing.carets(), [(0, 0), (1, 1)]);

    editing.run([Command::AddCaret { line: 1, column: 1 }]);
    assert_eq!(editing.carets(), [(0, 0)]);

    editing.run([Command::AddCaret { line: 0, column: 0 }]);
    assert_eq!(editing.carets(), [(0, 0)]);
}

#[test]
fn typing_at_three_carets_is_one_undo_step_that_restores_all_three() {
    let mut editing = open("a\nb\nc\n");
    editing.run([
        Command::PlaceCaret { line: 0, column: 1 },
        Command::AddCaret { line: 1, column: 1 },
        Command::AddCaret { line: 2, column: 1 },
    ]);

    editing.type_text("xy");
    assert_eq!(editing.lines(), ["axy", "bxy", "cxy", ""]);

    editing.run([Command::Undo]);
    assert_eq!(editing.lines(), ["a", "b", "c", ""]);
    assert_eq!(editing.carets(), [(0, 1), (1, 1), (2, 1)]);
    assert_eq!(editing.primary(), (2, 1));
    assert!(!editing.editor().modified);

    editing.run([Command::Redo]);
    assert_eq!(editing.lines(), ["axy", "bxy", "cxy", ""]);
    assert_eq!(editing.carets(), [(0, 3), (1, 3), (2, 3)]);
}

#[test]
fn backspace_deletes_at_every_caret_and_carets_that_meet_merge() {
    let mut editing = open("abc\nxyz\n");
    editing.run([Command::PlaceCaret { line: 0, column: 2 }, Command::AddCaret { line: 1, column: 2 }]);

    editing.run([Command::Delete(Left)]);
    assert_eq!(editing.lines(), ["ac", "xz", ""]);
    assert_eq!(editing.carets(), [(0, 1), (1, 1)]);

    // Two carets side by side: deleting a word back from both meets.
    editing.run([Command::PlaceCaret { line: 1, column: 1 }, Command::AddCaret { line: 1, column: 2 }]);
    editing.run([Command::Delete(WordLeft)]);
    assert_eq!(editing.lines(), ["ac", "", ""]);
    assert_eq!(editing.carets(), [(1, 0)]);
}

#[test]
fn moving_moves_every_caret_and_carets_that_meet_merge() {
    let mut editing = open("abc\nxyz\n");
    editing.run([Command::PlaceCaret { line: 0, column: 1 }, Command::AddCaret { line: 1, column: 1 }]);

    editing.run([Command::MoveCaret(Right)]);
    assert_eq!(editing.carets(), [(0, 2), (1, 2)]);

    editing.run([Command::Select(LineEnd)]);
    assert_eq!(editing.selections(), [vec![2..3], vec![2..3], vec![]]);

    editing.run([Command::MoveCaret(DocumentStart)]);
    assert_eq!(editing.carets(), [(0, 0)]);
}

/// Carets at the ends of the first three lines.
fn three_carets(text: &str) -> Editing {
    let mut editing = open(text);
    editing.run([
        Command::PlaceCaret { line: 0, column: 99 },
        Command::AddCaret { line: 1, column: 99 },
        Command::AddCaret { line: 2, column: 99 },
    ]);
    editing
}

#[test]
fn pasting_as_many_lines_as_carets_puts_one_line_at_each_caret() {
    let mut editing = three_carets("a\nb\nc\n");
    editing.host.clipboard().set_text("1\n2\n3\n");

    editing.run([Command::Paste]);

    assert_eq!(editing.lines(), ["a1", "b2", "c3", ""]);
    assert_eq!(editing.carets(), [(0, 2), (1, 2), (2, 2)]);

    editing.run([Command::Undo]);
    assert_eq!(editing.lines(), ["a", "b", "c", ""]);
}

#[test]
fn pasting_a_different_number_of_lines_pastes_all_of_it_at_each_caret() {
    let mut editing = three_carets("a\nb\nc\n");
    editing.host.clipboard().set_text("1\n2");

    editing.run([Command::Paste]);

    assert_eq!(editing.lines(), ["a1", "2", "b1", "2", "c1", "2", ""]);
}

#[test]
fn copying_at_several_carets_copies_each_selection_on_its_own_line() {
    let mut editing = three_carets("ab\ncd\nef\n");

    editing.run([Command::Select(Left), Command::Copy]);
    assert_eq!(editing.host.clipboard().text().as_deref(), Some("b\nd\nf"));

    editing.run([Command::Cut]);
    assert_eq!(editing.lines(), ["a", "c", "e", ""]);

    editing.run([Command::MoveCaret(LineStart), Command::Paste]);
    assert_eq!(editing.lines(), ["ba", "dc", "fe", ""]);
}
