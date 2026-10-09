//! Tabs and one split (ticket #31).

use genea_core::{CaretMove::*, CloseChoice, Command, ProjectId, ProjectView, Workbench};
use genea_testkit::{FixtureProject, TestHost};

fn project(files: &[(&str, &str)]) -> (FixtureProject, Workbench, ProjectId) {
    let fixture = files.iter().fold(FixtureProject::new(), |f, (path, text)| f.file(path, text)).build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    (fixture, workbench, project)
}

fn run(workbench: &mut Workbench, project: ProjectId, commands: impl IntoIterator<Item = Command>) {
    for command in commands {
        workbench.dispatch(project, command);
    }
    workbench.settle().unwrap();
}

fn open(path: &str) -> Command {
    Command::OpenFile(path.into())
}

fn view(workbench: &Workbench, project: ProjectId) -> ProjectView {
    workbench.project(project).unwrap()
}

/// Each side's tab titles, with the active one in brackets.
fn tabs(workbench: &Workbench, project: ProjectId) -> Vec<Vec<String>> {
    view(workbench, project)
        .panes
        .iter()
        .map(|pane| {
            pane.tabs
                .iter()
                .enumerate()
                .map(|(i, tab)| if pane.active == Some(i) { format!("[{}]", tab.title) } else { tab.title.clone() })
                .collect()
        })
        .collect()
}

#[test]
fn opened_files_show_as_tabs_with_the_last_one_active() {
    let (_fixture, mut workbench, project) = project(&[("a.ts", "a\n"), ("b.ts", "b\n")]);

    run(&mut workbench, project, [open("a.ts")]);
    run(&mut workbench, project, [open("b.ts")]);

    assert_eq!(tabs(&workbench, project), [["a.ts", "[b.ts]"]]);
    assert_eq!(view(&workbench, project).editor.unwrap().lines[0].text, "b");
}

#[test]
fn opening_a_file_that_is_already_open_focuses_its_tab() {
    let (_fixture, mut workbench, project) = project(&[("a.ts", "a\n"), ("b.ts", "b\n")]);
    run(&mut workbench, project, [open("a.ts")]);
    run(&mut workbench, project, [open("b.ts")]);

    run(&mut workbench, project, [open("a.ts")]);

    assert_eq!(tabs(&workbench, project), [["[a.ts]", "b.ts"]]);
    assert_eq!(view(&workbench, project).editor.unwrap().lines[0].text, "a");
}

fn caret(workbench: &Workbench, project: ProjectId) -> String {
    view(workbench, project).status.caret.unwrap()
}

#[test]
fn each_tab_keeps_its_caret_and_edits_when_switching() {
    let (_fixture, mut workbench, project) = project(&[("a.ts", "aaa\n"), ("b.ts", "bbb\n")]);
    run(&mut workbench, project, [open("a.ts")]);
    run(&mut workbench, project, [Command::MoveCaret(LineEnd), Command::InsertText("!".into())]);
    run(&mut workbench, project, [open("b.ts")]);
    assert_eq!(caret(&workbench, project), "1:1");

    run(&mut workbench, project, [Command::SelectTab { pane: 0, tab: 0 }]);

    let view = view(&workbench, project);
    assert_eq!(view.editor.as_ref().unwrap().lines[0].text, "aaa!");
    assert_eq!(view.status.caret.as_deref(), Some("1:5"));
    let modified: Vec<bool> = view.panes[0].tabs.iter().map(|t| t.modified).collect();
    assert_eq!(modified, [true, false]);
}

#[test]
fn closing_a_tab_shows_its_neighbour() {
    let (_fixture, mut workbench, project) = project(&[("a.ts", "a\n"), ("b.ts", "b\n"), ("c.ts", "c\n")]);
    run(&mut workbench, project, [open("a.ts")]);
    run(&mut workbench, project, [open("b.ts")]);
    run(&mut workbench, project, [open("c.ts")]);
    run(&mut workbench, project, [Command::SelectTab { pane: 0, tab: 1 }]);

    run(&mut workbench, project, [Command::CloseTab { pane: 0, tab: 1 }]);
    assert_eq!(tabs(&workbench, project), [["a.ts", "[c.ts]"]]);

    run(&mut workbench, project, [Command::CloseTab { pane: 0, tab: 1 }]);
    assert_eq!(tabs(&workbench, project), [["[a.ts]"]]);

    run(&mut workbench, project, [Command::CloseTab { pane: 0, tab: 0 }]);
    let view = view(&workbench, project);
    assert_eq!(tabs(&workbench, project), [Vec::<String>::new()]);
    assert_eq!(view.editor, None);
    assert_eq!(view.status.caret, None);
}

#[test]
fn closing_a_tab_with_unsaved_edits_asks_first() {
    let (fixture, mut workbench, project) = project(&[("a.ts", "a\n")]);
    run(&mut workbench, project, [open("a.ts")]);
    run(&mut workbench, project, [Command::InsertText("x".into())]);

    run(&mut workbench, project, [Command::CloseTab { pane: 0, tab: 0 }]);

    let prompt = view(&workbench, project).close_prompt.expect("asks to save, discard or cancel");
    assert_eq!(prompt.title, "a.ts");
    assert_eq!(tabs(&workbench, project), [["[a.ts]"]], "nothing closes before the answer");
    assert_eq!(fixture.read("a.ts"), "a\n");
}

#[test]
fn cancel_keeps_the_tab_and_its_edits() {
    let (_fixture, mut workbench, project) = project(&[("a.ts", "a\n")]);
    run(&mut workbench, project, [open("a.ts")]);
    run(&mut workbench, project, [Command::InsertText("x".into()), Command::CloseTab { pane: 0, tab: 0 }]);

    run(&mut workbench, project, [Command::ResolveClose(CloseChoice::Cancel)]);

    let view = view(&workbench, project);
    assert_eq!(view.close_prompt, None);
    assert_eq!(view.editor.unwrap().lines[0].text, "xa");
}

#[test]
fn discard_closes_the_tab_and_leaves_the_file_as_it_was() {
    let (fixture, mut workbench, project) = project(&[("a.ts", "a\n")]);
    run(&mut workbench, project, [open("a.ts")]);
    run(&mut workbench, project, [Command::InsertText("x".into()), Command::CloseTab { pane: 0, tab: 0 }]);

    run(&mut workbench, project, [Command::ResolveClose(CloseChoice::Discard)]);

    assert_eq!(view(&workbench, project).close_prompt, None);
    assert_eq!(tabs(&workbench, project), [Vec::<String>::new()]);
    assert_eq!(fixture.read("a.ts"), "a\n");

    run(&mut workbench, project, [open("a.ts")]);
    assert_eq!(view(&workbench, project).editor.unwrap().lines[0].text, "a", "reopening reads the file again");
}

#[test]
fn save_writes_the_file_then_closes_the_tab() {
    let (fixture, mut workbench, project) = project(&[("a.ts", "a\n"), ("b.ts", "b\n")]);
    run(&mut workbench, project, [open("a.ts")]);
    run(&mut workbench, project, [open("b.ts")]);
    run(&mut workbench, project, [Command::SelectTab { pane: 0, tab: 0 }, Command::InsertText("x".into())]);
    run(&mut workbench, project, [Command::CloseTab { pane: 0, tab: 0 }]);

    run(&mut workbench, project, [Command::ResolveClose(CloseChoice::Save)]);

    assert_eq!(fixture.read("a.ts"), "xa\n");
    assert_eq!(tabs(&workbench, project), [["[b.ts]"]]);
}

#[test]
fn a_failed_save_keeps_the_closing_tab_open() {
    let (fixture, mut workbench, project) = project(&[("src/a.ts", "a\n")]);
    run(&mut workbench, project, [open("src/a.ts")]);
    std::fs::remove_dir_all(fixture.path("src")).unwrap();
    run(&mut workbench, project, [Command::InsertText("x".into()), Command::CloseTab { pane: 0, tab: 0 }]);

    run(&mut workbench, project, [Command::ResolveClose(CloseChoice::Save)]);

    let view = view(&workbench, project);
    assert_eq!(tabs(&workbench, project), [["[a.ts]"]]);
    assert!(view.editor.unwrap().modified);
    assert!(view.notices.last().unwrap().message.starts_with("Couldn't save"));
}
