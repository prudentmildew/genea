//! Auto-indent on Return (ticket #25): the new line is indented according
//! to the syntax around the caret, one level of the file's indentation
//! deeper (two spaces here; `indentation.rs` covers the rest, ticket #26).

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

/// The buffer as the visible lines show it.
fn text(workbench: &Workbench, project: ProjectId) -> String {
    let editor = workbench.project(project).unwrap().editor.unwrap();
    editor.lines.into_iter().map(|l| l.text).collect::<Vec<_>>().join("\n")
}

fn caret(workbench: &Workbench, project: ProjectId) -> String {
    workbench.project(project).unwrap().status.caret.unwrap()
}

#[test]
fn return_after_an_opening_brace_indents_the_new_line() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "function f() {\n}\n");

    run(&mut workbench, project, [Command::PlaceCaret { line: 0, column: 14 }, Command::NewLine]);

    assert_eq!(text(&workbench, project), "function f() {\n  \n}\n");
    assert_eq!(caret(&workbench, project), "2:3");
}

#[test]
fn return_keeps_the_indentation_of_a_line_inside_a_block() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "function f() {\n    let a = 1;\n}\n");

    run(&mut workbench, project, [Command::PlaceCaret { line: 1, column: 14 }, Command::NewLine]);

    assert_eq!(text(&workbench, project), "function f() {\n    let a = 1;\n    \n}\n");
    assert_eq!(caret(&workbench, project), "3:5");
}

#[test]
fn return_between_a_pair_of_brackets_puts_the_closing_one_on_its_own_line() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "  const a = {};\n");

    run(&mut workbench, project, [Command::PlaceCaret { line: 0, column: 13 }, Command::NewLine]);

    assert_eq!(text(&workbench, project), "  const a = {\n    \n  };\n");
    assert_eq!(caret(&workbench, project), "2:5");
}

#[test]
fn return_inside_call_arguments_and_arrays_indents_too() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "f(a, [1, 2]);\n");

    run(&mut workbench, project, [Command::PlaceCaret { line: 0, column: 2 }, Command::NewLine]);
    assert_eq!(text(&workbench, project), "f(\n  a, [1, 2]);\n");

    workbench.settle().unwrap();
    run(&mut workbench, project, [Command::PlaceCaret { line: 1, column: 6 }, Command::NewLine]);
    assert_eq!(text(&workbench, project), "f(\n  a, [\n    1, 2]);\n");
}

#[test]
fn a_brace_typed_just_before_return_indents_before_the_reparse_lands() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "if (a) \n");

    // No settle between them: the syntax tree doesn't have the brace yet.
    run(&mut workbench, project, [
        Command::PlaceCaret { line: 0, column: 7 },
        Command::InsertText("{".into()),
        Command::NewLine,
    ]);

    assert_eq!(text(&workbench, project), "if (a) {\n  \n");
}

#[test]
fn a_brace_inside_a_string_does_not_indent() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "  let s = \"{\";\n");

    run(&mut workbench, project, [Command::PlaceCaret { line: 0, column: 12 }, Command::NewLine]);

    assert_eq!(text(&workbench, project), "  let s = \"{\n  \";\n");
}

#[test]
fn return_inside_a_jsx_element_indents_its_children() {
    let (_fixture, mut workbench, project) = open_file("view.tsx", "const v = <div></div>;\n");

    run(&mut workbench, project, [Command::PlaceCaret { line: 0, column: 15 }, Command::NewLine]);

    assert_eq!(text(&workbench, project), "const v = <div>\n  \n</div>;\n");
}

#[test]
fn return_inside_an_html_element_indents_its_children() {
    let (_fixture, mut workbench, project) = open_file("index.html", "<ul>\n</ul>\n");

    run(&mut workbench, project, [Command::PlaceCaret { line: 0, column: 4 }, Command::NewLine]);

    assert_eq!(text(&workbench, project), "<ul>\n  \n</ul>\n");
}

#[test]
fn return_after_a_yaml_key_without_a_value_indents_the_value() {
    let (_fixture, mut workbench, project) = open_file("config.yaml", "jobs:\n");

    run(&mut workbench, project, [Command::PlaceCaret { line: 0, column: 5 }, Command::NewLine]);

    assert_eq!(text(&workbench, project), "jobs:\n  \n");
}

#[test]
fn return_in_a_plain_text_file_keeps_the_line_s_indentation() {
    let (_fixture, mut workbench, project) = open_file("notes.txt", "  item {\n");

    run(&mut workbench, project, [Command::PlaceCaret { line: 0, column: 8 }, Command::NewLine]);

    assert_eq!(text(&workbench, project), "  item {\n  \n");
}

#[test]
fn return_indents_at_every_caret_and_undoes_in_one_step() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "a({});\nb({});\n");

    run(&mut workbench, project, [
        Command::PlaceCaret { line: 0, column: 3 },
        Command::AddCaret { line: 1, column: 3 },
        Command::NewLine,
    ]);
    assert_eq!(text(&workbench, project), "a({\n  \n});\nb({\n  \n});\n");

    run(&mut workbench, project, [Command::Undo]);
    assert_eq!(text(&workbench, project), "a({});\nb({});\n");
}
