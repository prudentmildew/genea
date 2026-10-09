//! Undo and redo: typing groups into steps, each step restores selections
//! (ticket #22). Grouping is driven by the test host's manual clock.

use std::time::Duration;

use genea_core::{CaretMove::*, Command, ProjectId, Workbench};
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

    /// The visible lines' grid text.
    fn lines(&self) -> Vec<String> {
        self.workbench.project(self.project).unwrap().editor.unwrap().lines.into_iter().map(|l| l.text).collect()
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
