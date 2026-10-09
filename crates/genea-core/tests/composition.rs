//! Dead-key composition (ticket #21, ADR 0004). The view's hidden
//! `TextInput` reports the IME's preedit and commits; the core draws the
//! preedit inline and takes commits as typing. The keystrokes themselves are
//! covered by the benchmark harness's synthesized events.

use genea_core::{Command, Preedit, ProjectId, Workbench};
use genea_testkit::{FixtureProject, TestHost};

fn open(text: &str) -> (FixtureProject, Workbench, ProjectId) {
    let fixture = FixtureProject::new().file("file.ts", text).build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::OpenFile("file.ts".into()));
    workbench.settle().unwrap();
    (fixture, workbench, project)
}

fn run(workbench: &mut Workbench, project: ProjectId, commands: impl IntoIterator<Item = Command>) {
    for command in commands {
        workbench.dispatch(project, command);
    }
}

fn lines(workbench: &Workbench, project: ProjectId) -> Vec<String> {
    workbench.project(project).unwrap().editor.unwrap().lines.into_iter().map(|l| l.text).collect()
}

#[test]
fn the_preedit_is_drawn_inline_at_the_caret_but_is_not_in_the_file() {
    let (_fixture, mut workbench, project) = open("let s = ;\n");

    run(&mut workbench, project, [Command::PlaceCaret { line: 0, column: 8 }]);
    run(&mut workbench, project, [Command::SetPreedit("`".into())]);

    let editor = workbench.project(project).unwrap().editor.unwrap();
    assert_eq!(editor.lines[0].text, "let s = `;");
    assert_eq!(editor.preedit, Some(Preedit { line: 0, column: 8, width: 1 }));
    assert!(!editor.modified, "composing doesn't edit the file");
}

#[test]
fn a_commit_replaces_the_preedit_with_the_composed_text() {
    let (_fixture, mut workbench, project) = open("caf\n");

    run(&mut workbench, project, [Command::MoveCaret(genea_core::CaretMove::LineEnd)]);
    run(&mut workbench, project, [Command::SetPreedit("´".into()), Command::InsertText("é".into())]);

    let editor = workbench.project(project).unwrap().editor.unwrap();
    assert_eq!(editor.lines[0].text, "café");
    assert_eq!(editor.preedit, None);
    assert_eq!(workbench.project(project).unwrap().status.caret.unwrap(), "1:5");
}

#[test]
fn norwegian_dead_keys_compose_backtick_caret_and_tilde() {
    let (_fixture, mut workbench, project) = open("\n");

    // ⇧´ space, ⇧¨ space, ⌥¨ space: each a preedit, then a commit.
    for (preedit, commit) in [("`", "`"), ("^", "^"), ("~", "~")] {
        run(&mut workbench, project, [Command::SetPreedit(preedit.into()), Command::InsertText(commit.into())]);
    }

    assert_eq!(lines(&workbench, project), ["`^~", ""]);
}

#[test]
fn a_cancelled_composition_leaves_the_text_as_it_was() {
    let (_fixture, mut workbench, project) = open("ab\n");

    run(&mut workbench, project, [Command::SetPreedit("¨".into()), Command::SetPreedit(String::new())]);

    let editor = workbench.project(project).unwrap().editor.unwrap();
    assert_eq!(editor.lines[0].text, "ab");
    assert_eq!(editor.preedit, None);
}

#[test]
fn saving_while_composing_writes_only_the_file_text() {
    let (fixture, mut workbench, project) = open("ab\n");

    run(&mut workbench, project, [Command::InsertText("x".into()), Command::SetPreedit("´".into()), Command::Save]);
    workbench.settle().unwrap();

    assert_eq!(fixture.read("file.ts"), "xab\n");
}
