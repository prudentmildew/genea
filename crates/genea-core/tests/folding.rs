//! Code folding (ticket #25): regions from the syntax tree, collapsed and
//! expanded from the gutter (`ToggleFold`) or by keyboard at the caret.

use genea_core::{CaretMove::*, Command, Fold, ProjectId, Workbench};
use genea_testkit::{FixtureProject, TestHost};

fn open_file(name: &str, text: &str, rows: f64) -> (FixtureProject, Workbench, ProjectId) {
    let fixture = FixtureProject::new().file(name, text).build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::SetViewport { rows });
    workbench.dispatch(project, Command::OpenFile(name.into()));
    workbench.settle().unwrap();
    (fixture, workbench, project)
}

fn run(workbench: &mut Workbench, project: ProjectId, commands: impl IntoIterator<Item = Command>) {
    for command in commands {
        workbench.dispatch(project, command);
    }
}

/// The visible lines' indices in the file.
fn shown(workbench: &Workbench, project: ProjectId) -> Vec<usize> {
    workbench.project(project).unwrap().editor.unwrap().lines.iter().map(|l| l.index).collect()
}

/// The visible lines that start a fold region, with its state.
fn markers(workbench: &Workbench, project: ProjectId) -> Vec<(usize, Fold)> {
    let editor = workbench.project(project).unwrap().editor.unwrap();
    editor.lines.iter().filter_map(|l| Some((l.index, l.fold?))).collect()
}

fn caret(workbench: &Workbench, project: ProjectId) -> String {
    workbench.project(project).unwrap().status.caret.unwrap()
}

const FUNCTION: &str = "function f() {\n  if (a) {\n    b();\n  }\n  c();\n}\n";

#[test]
fn blocks_spanning_lines_have_fold_markers() {
    let (_fixture, workbench, project) = open_file("main.ts", FUNCTION, 20.0);

    assert_eq!(markers(&workbench, project), [(0, Fold::Expanded), (1, Fold::Expanded)]);
}

#[test]
fn toggling_a_marker_hides_the_block_s_lines_and_shows_them_again() {
    let (_fixture, mut workbench, project) = open_file("main.ts", FUNCTION, 20.0);

    run(&mut workbench, project, [Command::ToggleFold { line: 1 }]);
    assert_eq!(shown(&workbench, project), [0, 1, 3, 4, 5, 6]);
    assert_eq!(markers(&workbench, project), [(0, Fold::Expanded), (1, Fold::Collapsed)]);

    run(&mut workbench, project, [Command::ToggleFold { line: 1 }]);
    assert_eq!(shown(&workbench, project), [0, 1, 2, 3, 4, 5, 6]);
}

#[test]
fn visible_lines_are_numbered_by_row() {
    let (_fixture, mut workbench, project) = open_file("main.ts", FUNCTION, 20.0);

    run(&mut workbench, project, [Command::ToggleFold { line: 1 }]);

    let editor = workbench.project(project).unwrap().editor.unwrap();
    let rows: Vec<(usize, usize)> = editor.lines.iter().map(|l| (l.index, l.row)).collect();
    assert_eq!(rows, [(0, 0), (1, 1), (3, 2), (4, 3), (5, 4), (6, 5)]);
}

#[test]
fn collapsing_at_the_caret_folds_the_innermost_block_and_then_the_next_one_out() {
    let (_fixture, mut workbench, project) = open_file("main.ts", FUNCTION, 20.0);
    run(&mut workbench, project, [Command::PlaceCaret { line: 2, column: 4 }]);

    run(&mut workbench, project, [Command::CollapseFold]);
    assert_eq!(shown(&workbench, project), [0, 1, 3, 4, 5, 6]);
    // The caret leaves the hidden lines for the block's opening brace.
    assert_eq!(caret(&workbench, project), "2:10");

    run(&mut workbench, project, [Command::CollapseFold]);
    assert_eq!(shown(&workbench, project), [0, 5, 6]);
    assert_eq!(caret(&workbench, project), "1:14");
}

#[test]
fn collapsing_on_a_block_s_first_line_folds_that_block() {
    let (_fixture, mut workbench, project) = open_file("main.ts", FUNCTION, 20.0);

    run(&mut workbench, project, [Command::PlaceCaret { line: 0, column: 0 }, Command::CollapseFold]);

    assert_eq!(shown(&workbench, project), [0, 5, 6]);
}

#[test]
fn expanding_at_the_caret_unfolds_the_block_on_its_line() {
    let (_fixture, mut workbench, project) = open_file("main.ts", FUNCTION, 20.0);
    run(&mut workbench, project, [Command::ToggleFold { line: 1 }, Command::ToggleFold { line: 0 }]);
    assert_eq!(shown(&workbench, project), [0, 5, 6]);

    run(&mut workbench, project, [Command::PlaceCaret { line: 0, column: 0 }, Command::ExpandFold]);

    // The inner block stays folded.
    assert_eq!(shown(&workbench, project), [0, 1, 3, 4, 5, 6]);
    assert_eq!(markers(&workbench, project), [(0, Fold::Expanded), (1, Fold::Collapsed)]);
}

#[test]
fn collapsing_with_a_selection_inside_moves_the_caret_to_the_block_start() {
    let (_fixture, mut workbench, project) = open_file("main.ts", FUNCTION, 20.0);
    run(&mut workbench, project, [Command::PlaceCaret { line: 2, column: 5 }, Command::ExpandSelection]);

    run(&mut workbench, project, [Command::CollapseFold]);

    assert_eq!(caret(&workbench, project), "2:10");
    assert_eq!(shown(&workbench, project), [0, 1, 3, 4, 5, 6]);
}

#[test]
fn up_and_down_skip_folded_lines() {
    let (_fixture, mut workbench, project) = open_file("main.ts", FUNCTION, 20.0);
    run(&mut workbench, project, [Command::ToggleFold { line: 1 }, Command::PlaceCaret { line: 1, column: 2 }]);

    run(&mut workbench, project, [Command::MoveCaret(Down)]);
    assert_eq!(caret(&workbench, project), "4:3");

    run(&mut workbench, project, [Command::MoveCaret(Up)]);
    assert_eq!(caret(&workbench, project), "2:3");
    assert_eq!(shown(&workbench, project), [0, 1, 3, 4, 5, 6]);
}

#[test]
fn right_at_the_end_of_a_folded_line_jumps_past_the_fold() {
    let (_fixture, mut workbench, project) = open_file("main.ts", FUNCTION, 20.0);
    run(&mut workbench, project, [Command::ToggleFold { line: 1 }, Command::PlaceCaret { line: 1, column: 10 }]);

    run(&mut workbench, project, [Command::MoveCaret(Right)]);
    assert_eq!(caret(&workbench, project), "4:1");

    run(&mut workbench, project, [Command::MoveCaret(Left)]);
    assert_eq!(caret(&workbench, project), "2:11");
    assert_eq!(shown(&workbench, project), [0, 1, 3, 4, 5, 6]);
}

#[test]
fn a_caret_placed_in_folded_lines_unfolds_them() {
    let (_fixture, mut workbench, project) = open_file("main.ts", FUNCTION, 20.0);
    run(&mut workbench, project, [Command::ToggleFold { line: 1 }]);

    run(&mut workbench, project, [Command::PlaceCaret { line: 2, column: 0 }]);

    assert_eq!(shown(&workbench, project), [0, 1, 2, 3, 4, 5, 6]);
}

#[test]
fn a_fold_moves_with_edits_above_it() {
    let (_fixture, mut workbench, project) = open_file("main.ts", FUNCTION, 20.0);
    run(&mut workbench, project, [Command::ToggleFold { line: 1 }]);

    run(&mut workbench, project, [Command::PlaceCaret { line: 0, column: 0 }, Command::InsertText("// f\n".into())]);

    assert_eq!(shown(&workbench, project), [0, 1, 2, 4, 5, 6, 7]);
    assert_eq!(markers(&workbench, project), [(1, Fold::Expanded), (2, Fold::Collapsed)]);
}

#[test]
fn collapse_all_and_expand_all() {
    let (_fixture, mut workbench, project) =
        open_file("main.ts", "/**\n * Docs.\n */\nfunction f() {\n  a();\n}\nconst o = {\n  b: 1,\n};\n", 20.0);

    run(&mut workbench, project, [Command::CollapseAllFolds]);
    assert_eq!(shown(&workbench, project), [0, 3, 5, 6, 8, 9]);

    run(&mut workbench, project, [Command::ExpandAllFolds]);
    assert_eq!(shown(&workbench, project), (0..10).collect::<Vec<_>>());
}

#[test]
fn scrolling_counts_rows_not_hidden_lines() {
    let body: String = (0..30).map(|i| format!("  a{i}();\n")).collect();
    let text = format!("function f() {{\n{body}}}\nlast();\n");
    let (_fixture, mut workbench, project) = open_file("main.ts", &text, 5.0);
    run(&mut workbench, project, [Command::ToggleFold { line: 0 }]);

    // Rows: the header, `}`, `last();` and the empty last line.
    run(&mut workbench, project, [Command::ScrollBy { rows: 100.0 }]);

    let editor = workbench.project(project).unwrap().editor.unwrap();
    assert_eq!(editor.scroll_top, 0.0);
    assert_eq!(editor.lines.iter().map(|l| l.index).collect::<Vec<_>>(), [0, 31, 32, 33]);
}

#[test]
fn json_markdown_and_comments_fold_too() {
    let (_fixture, workbench, project) = open_file("data.json", "{\n  \"a\": [\n    1\n  ]\n}\n", 20.0);
    assert_eq!(markers(&workbench, project), [(0, Fold::Expanded), (1, Fold::Expanded)]);

    let (_fixture, mut workbench, project) = open_file("README.md", "# Title\n\nText.\n\n## More\n\nMore text.\n", 20.0);
    run(&mut workbench, project, [Command::ToggleFold { line: 4 }]);
    assert_eq!(shown(&workbench, project), [0, 1, 2, 3, 4, 7]);

    let (_fixture, mut workbench, project) = open_file("main.ts", "/*\n a\n b\n*/\nx();\n", 20.0);
    run(&mut workbench, project, [Command::ToggleFold { line: 0 }]);
    assert_eq!(shown(&workbench, project), [0, 4, 5]);
}
