//! Expand and shrink selection (⌥↑ / ⌥↓, ticket #25): the selection grows
//! to the enclosing syntax node, and shrinks back the way it grew.

use genea_core::{Command, ProjectId, Workbench};
use genea_testkit::{FixtureProject, TestHost};

fn open_file(name: &str, text: &str) -> (FixtureProject, Workbench, ProjectId) {
    let fixture = FixtureProject::new().file(name, text).build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::SetViewport { rows: 20.0 });
    workbench.dispatch(project, Command::OpenFile(name.into()));
    workbench.settle().unwrap();
    (fixture, workbench, project)
}

fn run(workbench: &mut Workbench, project: ProjectId, commands: impl IntoIterator<Item = Command>) {
    for command in commands {
        workbench.dispatch(project, command);
    }
}

/// The selected text of each selection, top to bottom; a selection going
/// on past a line's end takes its line break.
fn selected(workbench: &Workbench, project: ProjectId) -> Vec<String> {
    let editor = workbench.project(project).unwrap().editor.unwrap();
    let mut selections: Vec<String> = Vec::new();
    let mut open = false;
    for line in &editor.lines {
        let chars: Vec<char> = line.text.chars().collect();
        for (i, columns) in line.selections.iter().enumerate() {
            let text: String = chars[columns.start.min(chars.len())..columns.end.min(chars.len())].iter().collect();
            let past_end = columns.end > chars.len();
            if i == 0 && open && columns.start == 0 {
                selections.last_mut().unwrap().push_str(&text);
            } else {
                selections.push(text);
            }
            if past_end {
                selections.last_mut().unwrap().push('\n');
            }
            open = past_end;
        }
        if line.selections.is_empty() {
            open = false;
        }
    }
    selections
}

/// Expands once and returns the one selection.
fn expand(workbench: &mut Workbench, project: ProjectId) -> String {
    run(workbench, project, [Command::ExpandSelection]);
    let selected = selected(workbench, project);
    assert_eq!(selected.len(), 1, "one selection: {selected:?}");
    selected[0].clone()
}

fn shrink(workbench: &mut Workbench, project: ProjectId) -> Vec<String> {
    run(workbench, project, [Command::ShrinkSelection]);
    selected(workbench, project)
}

#[test]
fn expanding_walks_out_through_the_enclosing_nodes() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "const total = price * count;\n");
    run(&mut workbench, project, [Command::PlaceCaret { line: 0, column: 16 }]);

    assert_eq!(expand(&mut workbench, project), "price");
    assert_eq!(expand(&mut workbench, project), "price * count");
    assert_eq!(expand(&mut workbench, project), "total = price * count");
    assert_eq!(expand(&mut workbench, project), "const total = price * count;");
}

#[test]
fn shrinking_goes_back_the_way_the_selection_grew() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "const total = price * count;\n");
    run(&mut workbench, project, [Command::PlaceCaret { line: 0, column: 16 }]);
    for _ in 0..3 {
        expand(&mut workbench, project);
    }

    assert_eq!(shrink(&mut workbench, project), ["price * count"]);
    assert_eq!(shrink(&mut workbench, project), ["price"]);
    assert_eq!(shrink(&mut workbench, project), Vec::<String>::new());
    assert_eq!(workbench.project(project).unwrap().status.caret.unwrap(), "1:17");
}

#[test]
fn a_block_s_inside_comes_before_the_block() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "function f() {\n  a();\n  b();\n}\n");
    run(&mut workbench, project, [Command::PlaceCaret { line: 1, column: 2 }]);

    assert_eq!(expand(&mut workbench, project), "a");
    assert_eq!(expand(&mut workbench, project), "a()");
    assert_eq!(expand(&mut workbench, project), "a();");
    assert_eq!(expand(&mut workbench, project), "a();\n  b();");
    assert_eq!(expand(&mut workbench, project), "{\n  a();\n  b();\n}");
    assert_eq!(expand(&mut workbench, project), "function f() {\n  a();\n  b();\n}");
}

#[test]
fn json_values_expand_to_their_array_and_pair() {
    let (_fixture, mut workbench, project) = open_file("data.json", "{\"a\": [10, 20]}\n");
    run(&mut workbench, project, [Command::PlaceCaret { line: 0, column: 8 }]);

    assert_eq!(expand(&mut workbench, project), "10");
    assert_eq!(expand(&mut workbench, project), "10, 20");
    assert_eq!(expand(&mut workbench, project), "[10, 20]");
    assert_eq!(expand(&mut workbench, project), "\"a\": [10, 20]");
}

#[test]
fn every_caret_expands() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "f(alpha);\ng(beta);\n");
    run(&mut workbench, project, [
        Command::PlaceCaret { line: 0, column: 3 },
        Command::AddCaret { line: 1, column: 3 },
        Command::ExpandSelection,
    ]);

    assert_eq!(selected(&workbench, project), ["alpha", "beta"]);
}

#[test]
fn shrinking_after_the_selection_changed_otherwise_does_nothing() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "const total = price * count;\n");
    run(&mut workbench, project, [Command::PlaceCaret { line: 0, column: 16 }]);
    expand(&mut workbench, project);
    expand(&mut workbench, project);
    run(&mut workbench, project, [Command::SelectWord { line: 0, column: 7 }]);

    assert_eq!(shrink(&mut workbench, project), ["total"]);
}

#[test]
fn a_file_without_a_syntax_tree_selects_nothing_new() {
    let (_fixture, mut workbench, project) = open_file("notes.txt", "some words\n");
    run(&mut workbench, project, [Command::PlaceCaret { line: 0, column: 2 }]);

    run(&mut workbench, project, [Command::ExpandSelection]);

    assert!(selected(&workbench, project).is_empty());
}
