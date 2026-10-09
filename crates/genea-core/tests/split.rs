//! One split: two editors side by side (ticket #31).

use genea_core::{CaretMove::*, CloseChoice, Command, ProjectId, ProjectView, Workbench};
use genea_testkit::{FixtureProject, TestHost};

fn project(files: &[(&str, &str)]) -> (FixtureProject, Workbench, ProjectId) {
    let fixture = files.iter().fold(FixtureProject::new(), |f, (path, text)| f.file(path, text)).build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    (fixture, workbench, project)
}

fn run(workbench: &mut Workbench, project: ProjectId, commands: impl IntoIterator<Item = Command>) {
    // Settles after each command: a file opens in the background.
    for command in commands {
        workbench.dispatch(project, command);
        workbench.settle().unwrap();
    }
}

fn open(path: &str) -> Command {
    Command::OpenFile(path.into())
}

fn view(workbench: &Workbench, project: ProjectId) -> ProjectView {
    workbench.project(project).unwrap()
}

/// Each side's tab titles, with the active one in brackets and the focused
/// side marked with a `*`.
fn tabs(workbench: &Workbench, project: ProjectId) -> Vec<String> {
    let view = view(workbench, project);
    view.panes
        .iter()
        .enumerate()
        .map(|(p, pane)| {
            let titles: Vec<String> = pane
                .tabs
                .iter()
                .enumerate()
                .map(|(i, tab)| if pane.active == Some(i) { format!("[{}]", tab.title) } else { tab.title.clone() })
                .collect();
            format!("{}{}", if p == view.focused_pane { "*" } else { "" }, titles.join(" "))
        })
        .collect()
}

/// The first visible line of each side's editor.
fn first_lines(workbench: &Workbench, project: ProjectId) -> Vec<String> {
    view(workbench, project)
        .panes
        .iter()
        .map(|p| p.editor.as_ref().map_or("-".into(), |e| e.lines[0].text.clone()))
        .collect()
}

#[test]
fn split_right_opens_the_file_again_on_the_right_and_focuses_it() {
    let (_fixture, mut workbench, project) = project(&[("a.ts", "a\n"), ("b.ts", "b\n")]);
    run(&mut workbench, project, [open("a.ts")]);
    run(&mut workbench, project, [open("b.ts")]);
    assert!(view(&workbench, project).can_split);

    run(&mut workbench, project, [Command::SplitRight]);

    assert_eq!(tabs(&workbench, project), ["a.ts [b.ts]", "*[b.ts]"]);
    assert_eq!(first_lines(&workbench, project), ["b", "b"]);
}

#[test]
fn there_is_at_most_one_split() {
    let (_fixture, mut workbench, project) = project(&[("a.ts", "a\n")]);
    assert!(!view(&workbench, project).can_split, "nothing to split without a file");
    run(&mut workbench, project, [open("a.ts"), Command::SplitRight]);

    assert!(!view(&workbench, project).can_split);
    run(&mut workbench, project, [Command::SplitRight]);
    run(&mut workbench, project, [Command::SelectTab { pane: 0, tab: 0 }, Command::SplitRight]);

    assert_eq!(tabs(&workbench, project), ["*[a.ts]", "[a.ts]"]);
}

#[test]
fn edits_to_a_file_open_on_both_sides_show_on_both() {
    let (fixture, mut workbench, project) = project(&[("a.ts", "one\ntwo\n")]);
    run(&mut workbench, project, [open("a.ts"), Command::SplitRight]);

    run(&mut workbench, project, [Command::InsertText("// ".into())]);
    assert_eq!(first_lines(&workbench, project), ["// one", "// one"]);
    assert!(view(&workbench, project).panes.iter().all(|p| p.tabs[0].modified));

    run(&mut workbench, project, [Command::FocusPane(0), Command::MoveCaret(Down), Command::Delete(Right)]);
    let view = view(&workbench, project);
    let second: Vec<&str> = view.panes.iter().map(|p| p.editor.as_ref().unwrap().lines[1].text.as_str()).collect();
    assert_eq!(second, ["wo", "wo"]);

    run(&mut workbench, project, [Command::Save]);
    assert_eq!(fixture.read("a.ts"), "// one\nwo\n");
    assert!(workbench.project(project).unwrap().panes.iter().all(|p| !p.tabs[0].modified));
}

#[test]
fn each_side_keeps_its_own_caret_and_scroll() {
    let text: String = (1..=100).map(|i| format!("line {i}\n")).collect();
    let (_fixture, mut workbench, project) = project(&[("a.ts", &text)]);
    run(&mut workbench, project, [Command::SetViewport { rows: 10.0 }, open("a.ts"), Command::SplitRight]);

    run(&mut workbench, project, [Command::MoveCaret(DocumentEnd)]);
    run(&mut workbench, project, [Command::FocusPane(0)]);

    let view = view(&workbench, project);
    assert_eq!(view.status.caret.as_deref(), Some("1:1"));
    let carets: Vec<usize> = view.panes.iter().map(|p| p.editor.as_ref().unwrap().caret.line).collect();
    assert_eq!(carets, [0, 100]);
    assert_eq!(first_lines(&workbench, project), ["line 1", "line 92"]);
}

#[test]
fn a_side_without_the_focus_scrolls_on_its_own() {
    let text: String = (1..=100).map(|i| format!("line {i}\n")).collect();
    let (_fixture, mut workbench, project) = project(&[("a.ts", &text), ("b.ts", "b\n")]);
    run(&mut workbench, project, [Command::SetViewport { rows: 10.0 }, open("a.ts"), Command::SplitRight]);
    run(&mut workbench, project, [open("b.ts")]);

    run(&mut workbench, project, [Command::ScrollPane { pane: 0, rows: 5.0 }]);

    assert_eq!(first_lines(&workbench, project), ["line 6", "b"]);
    assert_eq!(view(&workbench, project).focused_pane, 1);
}

#[test]
fn opening_a_file_open_on_the_other_side_focuses_it_there() {
    let (_fixture, mut workbench, project) = project(&[("a.ts", "a\n"), ("b.ts", "b\n")]);
    run(&mut workbench, project, [open("a.ts"), Command::SplitRight]);
    run(&mut workbench, project, [open("b.ts")]);
    run(&mut workbench, project, [Command::FocusPane(0)]);

    run(&mut workbench, project, [open("b.ts")]);

    assert_eq!(tabs(&workbench, project), ["[a.ts]", "*a.ts [b.ts]"]);
}

#[test]
fn a_tab_moves_to_the_other_side() {
    let (_fixture, mut workbench, project) = project(&[("a.ts", "a\n"), ("b.ts", "b\n")]);
    run(&mut workbench, project, [open("a.ts")]);
    run(&mut workbench, project, [open("b.ts")]);

    run(&mut workbench, project, [Command::MoveTabToOtherSide { pane: 0, tab: 1 }]);
    assert_eq!(tabs(&workbench, project), ["[a.ts]", "*[b.ts]"], "moving splits");

    run(&mut workbench, project, [Command::MoveTabToOtherSide { pane: 0, tab: 0 }]);
    assert_eq!(tabs(&workbench, project), ["*b.ts [a.ts]"], "the emptied side closes");
}

#[test]
fn moving_a_tab_to_a_side_where_its_file_is_open_focuses_that_tab() {
    let (_fixture, mut workbench, project) = project(&[("a.ts", "a\n"), ("b.ts", "b\n")]);
    run(&mut workbench, project, [open("a.ts")]);
    run(&mut workbench, project, [open("b.ts"), Command::SplitRight]);
    run(&mut workbench, project, [open("a.ts")]);

    run(&mut workbench, project, [Command::MoveTabToOtherSide { pane: 1, tab: 1 }]);

    assert_eq!(tabs(&workbench, project), ["*[a.ts] b.ts", "[b.ts]"]);
}

#[test]
fn closing_the_split_brings_its_tabs_to_one_side() {
    let (_fixture, mut workbench, project) = project(&[("a.ts", "a\n"), ("b.ts", "b\n"), ("c.ts", "c\n")]);
    run(&mut workbench, project, [open("a.ts")]);
    run(&mut workbench, project, [open("b.ts"), Command::SplitRight]);
    run(&mut workbench, project, [open("c.ts")]);

    run(&mut workbench, project, [Command::CloseSplit]);

    assert_eq!(tabs(&workbench, project), ["*a.ts b.ts [c.ts]"]);
    assert!(view(&workbench, project).can_split);
}

#[test]
fn closing_the_last_tab_on_a_side_closes_the_split() {
    let (_fixture, mut workbench, project) = project(&[("a.ts", "a\n"), ("b.ts", "b\n")]);
    run(&mut workbench, project, [open("a.ts"), Command::SplitRight]);
    run(&mut workbench, project, [open("b.ts")]);

    run(&mut workbench, project, [Command::CloseTab { pane: 0, tab: 0 }]);

    assert_eq!(tabs(&workbench, project), ["*a.ts [b.ts]"]);
}

#[test]
fn closing_one_of_two_tabs_on_an_edited_file_keeps_the_edits_without_asking() {
    let (_fixture, mut workbench, project) = project(&[("a.ts", "a\n")]);
    run(&mut workbench, project, [open("a.ts"), Command::SplitRight, Command::InsertText("x".into())]);

    run(&mut workbench, project, [Command::CloseTab { pane: 1, tab: 0 }]);

    let view = view(&workbench, project);
    assert_eq!(view.close_prompt, None);
    assert_eq!(tabs(&workbench, project), ["*[a.ts]"]);
    let editor = view.editor.unwrap();
    assert_eq!(editor.lines[0].text, "xa");
    assert!(editor.modified);

    run(&mut workbench, project, [Command::CloseTab { pane: 0, tab: 0 }]);
    assert!(workbench.project(project).unwrap().close_prompt.is_some(), "the last tab asks");
    run(&mut workbench, project, [Command::ResolveClose(CloseChoice::Discard)]);
    assert_eq!(tabs(&workbench, project), ["*"]);
}
