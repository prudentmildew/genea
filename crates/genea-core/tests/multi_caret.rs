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
