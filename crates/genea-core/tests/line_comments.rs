//! ⌘/ (ticket #25): toggles line comments on the caret's lines, using the
//! language's comment syntax.

use genea_core::{CaretMove::*, Command, ProjectId, Workbench};
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

fn select_lines(from: usize, to: usize) -> [Command; 2] {
    [Command::PlaceCaret { line: from, column: 0 }, Command::ExtendSelection { line: to, column: 0 }]
}

#[test]
fn toggling_comments_out_the_caret_s_line_and_back_in() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "let a = 1;\nlet b = 2;\n");

    run(&mut workbench, project, [Command::PlaceCaret { line: 0, column: 4 }, Command::ToggleLineComment]);
    assert_eq!(text(&workbench, project), "// let a = 1;\nlet b = 2;\n");
    assert_eq!(workbench.project(project).unwrap().status.caret.unwrap(), "1:8");

    run(&mut workbench, project, [Command::ToggleLineComment]);
    assert_eq!(text(&workbench, project), "let a = 1;\nlet b = 2;\n");
}

#[test]
fn selected_lines_are_commented_at_their_shallowest_indentation_skipping_blank_ones() {
    let (_fixture, mut workbench, project) =
        open_file("main.ts", "function f() {\n  if (a) {\n    b();\n\n  }\n}\n");

    run(&mut workbench, project, select_lines(1, 5));
    run(&mut workbench, project, [Command::ToggleLineComment]);

    assert_eq!(text(&workbench, project), "function f() {\n  // if (a) {\n  //   b();\n\n  // }\n}\n");

    // The selection still covers the lines, so ⌘/ again uncomments them.
    run(&mut workbench, project, [Command::ToggleLineComment]);
    assert_eq!(text(&workbench, project), "function f() {\n  if (a) {\n    b();\n\n  }\n}\n");
}

#[test]
fn lines_that_are_not_all_commented_get_commented() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "// a();\nb();\n");

    run(&mut workbench, project, select_lines(0, 2));
    run(&mut workbench, project, [Command::ToggleLineComment]);

    assert_eq!(text(&workbench, project), "// // a();\n// b();\n");
}

#[test]
fn a_comment_without_a_space_is_uncommented_too() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "  //a();\n");

    run(&mut workbench, project, [Command::ToggleLineComment]);

    assert_eq!(text(&workbench, project), "  a();\n");
}

#[test]
fn a_selection_ending_at_the_start_of_a_line_leaves_that_line_alone() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "a();\nb();\nc();\n");

    run(&mut workbench, project, [Command::Select(Down)]);
    run(&mut workbench, project, [Command::ToggleLineComment]);

    assert_eq!(text(&workbench, project), "// a();\nb();\nc();\n");
}

#[test]
fn every_caret_s_line_is_toggled_in_one_undo_step() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "a();\nb();\nc();\n");

    run(&mut workbench, project, [
        Command::PlaceCaret { line: 0, column: 1 },
        Command::AddCaret { line: 2, column: 1 },
        Command::ToggleLineComment,
    ]);
    assert_eq!(text(&workbench, project), "// a();\nb();\n// c();\n");

    run(&mut workbench, project, [Command::Undo]);
    assert_eq!(text(&workbench, project), "a();\nb();\nc();\n");
}

#[test]
fn yaml_and_env_files_comment_with_a_hash() {
    let (_fixture, mut workbench, project) = open_file("config.yml", "a: 1\n");
    run(&mut workbench, project, [Command::ToggleLineComment]);
    assert_eq!(text(&workbench, project), "# a: 1\n");

    let (_fixture, mut workbench, project) = open_file(".env", "KEY=value\n");
    run(&mut workbench, project, [Command::ToggleLineComment]);
    assert_eq!(text(&workbench, project), "# KEY=value\n");
}

#[test]
fn css_wraps_each_line_in_a_block_comment_and_unwraps_it() {
    let (_fixture, mut workbench, project) = open_file("style.css", "a {\n  color: red;\n}\n");

    run(&mut workbench, project, [Command::PlaceCaret { line: 1, column: 0 }, Command::ToggleLineComment]);
    assert_eq!(text(&workbench, project), "a {\n  /* color: red; */\n}\n");

    run(&mut workbench, project, [Command::ToggleLineComment]);
    assert_eq!(text(&workbench, project), "a {\n  color: red;\n}\n");
}

#[test]
fn html_and_markdown_use_html_comments() {
    let (_fixture, mut workbench, project) = open_file("index.html", "<p>hi</p>\n");
    run(&mut workbench, project, [Command::ToggleLineComment]);
    assert_eq!(text(&workbench, project), "<!-- <p>hi</p> -->\n");

    let (_fixture, mut workbench, project) = open_file("README.md", "Some text\n");
    run(&mut workbench, project, [Command::ToggleLineComment]);
    assert_eq!(text(&workbench, project), "<!-- Some text -->\n");
}

#[test]
fn a_plain_text_file_has_no_comments() {
    let (_fixture, mut workbench, project) = open_file("notes.txt", "hello\n");

    run(&mut workbench, project, [Command::ToggleLineComment]);

    assert_eq!(text(&workbench, project), "hello\n");
    assert!(!workbench.project(project).unwrap().editor.unwrap().modified);
}
