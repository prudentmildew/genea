//! Syntax highlighting (ticket #24): highlight spans in view state, for the
//! first-class languages and the basic file types, kept in step with edits.

use genea_core::{CaretMove::*, Command, Highlight, ProjectId, Workbench};
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

/// A visible line's highlight spans as (the grid text they cover, highlight).
fn spans(workbench: &Workbench, project: ProjectId, line: usize) -> Vec<(String, Highlight)> {
    let editor = workbench.project(project).unwrap().editor.unwrap();
    let line = editor.lines.iter().find(|l| l.index == line).expect("the line is visible");
    let cells = grid_cells(&line.text);
    line.highlights
        .iter()
        .map(|span| {
            let text: String = cells.iter().filter(|(column, _)| span.columns.contains(column)).map(|(_, c)| *c).collect();
            (text, span.highlight)
        })
        .collect()
}

/// Each char of grid text with the display column it starts at.
fn grid_cells(text: &str) -> Vec<(usize, char)> {
    let mut column = 0;
    text.chars()
        .map(|c| {
            let cell = (column, c);
            column += if ('\u{3000}'..='\u{9fff}').contains(&c) { 2 } else { 1 };
            cell
        })
        .collect()
}

fn has(spans: &[(String, Highlight)], text: &str, highlight: Highlight) -> bool {
    spans.iter().any(|(t, h)| t == text && *h == highlight)
}

#[track_caller]
fn assert_highlighted(spans: &[(String, Highlight)], text: &str, highlight: Highlight) {
    assert!(has(spans, text, highlight), "expected {text:?} as {highlight:?} in {spans:?}");
}

#[test]
fn a_typescript_file_is_highlighted_once_parsed() {
    let (_fixture, workbench, project) = open_file("main.ts", "const greeting: string = \"hi\"; // hello\n");

    let line = spans(&workbench, project, 0);

    assert_highlighted(&line, "const", Highlight::Keyword);
    assert_highlighted(&line, "string", Highlight::TypeBuiltin);
    assert_highlighted(&line, "\"hi\"", Highlight::String);
    assert_highlighted(&line, "// hello", Highlight::Comment);
}

fn columns(workbench: &Workbench, project: ProjectId, line: usize) -> Vec<(std::ops::Range<usize>, Highlight)> {
    let editor = workbench.project(project).unwrap().editor.unwrap();
    let line = editor.lines.iter().find(|l| l.index == line).expect("the line is visible");
    line.highlights.iter().map(|s| (s.columns.clone(), s.highlight)).collect()
}

#[test]
fn until_the_reparse_lands_the_old_highlights_move_with_the_edit() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "let a = \"x\";\n");

    // No settle: the background reparse hasn't been applied yet.
    run(&mut workbench, project, [Command::InsertText("  ".into())]);
    let line = spans(&workbench, project, 0);
    assert_highlighted(&line, "let", Highlight::Keyword);
    assert_highlighted(&line, "\"x\"", Highlight::String);
    assert!(columns(&workbench, project, 0).contains(&(2..5, Highlight::Keyword)));

    // Typing inside the string grows it.
    run(&mut workbench, project, [Command::PlaceCaret { line: 0, column: 12 }, Command::InsertText("yz".into())]);
    assert_highlighted(&spans(&workbench, project, 0), "\"xyz\"", Highlight::String);
}

#[test]
fn highlighting_catches_up_with_edits_once_the_reparse_lands() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "let a = 1;\nlet b = 2;\n");

    run(&mut workbench, project, [Command::InsertText("// ".into())]);
    workbench.settle().unwrap();

    assert_highlighted(&spans(&workbench, project, 0), "// let a = 1;", Highlight::Comment);
    assert_highlighted(&spans(&workbench, project, 1), "let", Highlight::Keyword);
}

#[test]
fn edits_made_while_a_reparse_runs_are_caught_up_with_too() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "\n");

    for c in "const s = \"a\"; /* done */".chars() {
        run(&mut workbench, project, [Command::InsertText(c.to_string())]);
    }
    workbench.settle().unwrap();

    let line = spans(&workbench, project, 0);
    assert_highlighted(&line, "const", Highlight::Keyword);
    assert_highlighted(&line, "\"a\"", Highlight::String);
    assert_highlighted(&line, "/* done */", Highlight::Comment);
}

#[test]
fn deleting_text_is_highlighted_after_the_reparse() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "// let a = 1;\n");

    run(&mut workbench, project, [Command::Delete(Right)]);
    run(&mut workbench, project, [Command::Delete(Right)]);
    run(&mut workbench, project, [Command::Delete(Right)]);
    workbench.settle().unwrap();

    assert_highlighted(&spans(&workbench, project, 0), "let", Highlight::Keyword);
}

#[test]
fn undo_and_redo_are_highlighted_after_the_reparse() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "let a = 1;\n");

    // Undone before the reparse of the deletion lands: the text is back to
    // what was parsed, but the shifted highlights lost `let`.
    run(&mut workbench, project, [Command::Select(WordRight), Command::Delete(Left), Command::Undo]);
    workbench.settle().unwrap();
    assert_highlighted(&spans(&workbench, project, 0), "let", Highlight::Keyword);

    run(&mut workbench, project, [Command::Redo, Command::InsertText("// ".into())]);
    workbench.settle().unwrap();
    assert_highlighted(&spans(&workbench, project, 0), "//  a = 1;", Highlight::Comment);
}

#[test]
fn typing_at_several_carets_moves_and_then_reparses_every_line() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "let a = 1;\nlet b = 2;\n");

    run(&mut workbench, project, [Command::AddCaret { line: 1, column: 0 }, Command::InsertText("// ".into())]);
    // Before the reparse lands, both keywords moved right with their text.
    assert!(columns(&workbench, project, 0).contains(&(3..6, Highlight::Keyword)));
    assert!(columns(&workbench, project, 1).contains(&(3..6, Highlight::Keyword)));

    workbench.settle().unwrap();
    assert_highlighted(&spans(&workbench, project, 0), "// let a = 1;", Highlight::Comment);
    assert_highlighted(&spans(&workbench, project, 1), "// let b = 2;", Highlight::Comment);
}

#[test]
fn spans_are_in_display_columns_past_tabs_and_wide_characters() {
    let (_fixture, workbench, project) = open_file("main.ts", "\tlet s = \"日本\"; // x\n");

    let line = columns(&workbench, project, 0);

    assert!(line.contains(&(4..7, Highlight::Keyword)), "a tab takes 4 columns: {line:?}");
    assert!(line.contains(&(12..18, Highlight::String)), "each CJK char takes 2: {line:?}");
    assert!(line.contains(&(20..24, Highlight::Comment)), "{line:?}");
}

#[test]
fn the_preedit_is_plain_and_pushes_the_highlights_after_it() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "let s = ;\n");

    run(&mut workbench, project, [Command::PlaceCaret { line: 0, column: 4 }, Command::SetPreedit("´".into())]);

    let line = columns(&workbench, project, 0);
    assert!(line.contains(&(0..3, Highlight::Keyword)), "{line:?}");
    assert!(line.iter().all(|(c, _)| !c.contains(&4)), "the preedit is plain: {line:?}");
    assert!(line.iter().any(|(c, _)| c.contains(&9)), "`=` moved one column right: {line:?}");
}

#[test]
fn opening_another_file_while_a_parse_runs_highlights_the_new_one() {
    let fixture = FixtureProject::new().file("a.ts", "let a = 1;\n").file("b.json", "{ \"k\": 1 }\n").build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::OpenFile("a.ts".into()));
    workbench.settle().unwrap();

    run(&mut workbench, project, [Command::InsertText("x".into()), Command::OpenFile("b.json".into())]);
    workbench.settle().unwrap();

    assert_highlighted(&spans(&workbench, project, 0), "\"k\"", Highlight::Property);
}

#[test]
fn a_file_in_a_background_tab_catches_up_with_its_edits() {
    let fixture = FixtureProject::new().file("a.ts", "let a = 1;\n").file("b.ts", "let b = 2;\n").build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::OpenFile("a.ts".into()));
    workbench.settle().unwrap();

    // The parse of the edit is still running when a.ts goes to the background.
    run(&mut workbench, project, [Command::InsertText("// ".into()), Command::OpenFile("b.ts".into())]);
    workbench.settle().unwrap();
    run(&mut workbench, project, [Command::SelectTab { pane: 0, tab: 0 }]);

    assert_highlighted(&spans(&workbench, project, 0), "// let a = 1;", Highlight::Comment);
}

#[test]
fn files_over_5_mb_are_not_highlighted() {
    let line = "let a = 1;\n";
    let (_fixture, workbench, project) = open_file("big.ts", &line.repeat(5 * 1024 * 1024 / line.len() + 1));

    assert!(spans(&workbench, project, 0).is_empty());
}

#[test]
fn tsx_highlights_jsx_tags_and_attributes_and_types() {
    let (_fixture, workbench, project) =
        open_file("app.tsx", "const view: JSX.Element = <div className=\"x\">{count}</div>;\n");

    let line = spans(&workbench, project, 0);

    assert_highlighted(&line, "div", Highlight::Tag);
    assert_highlighted(&line, "className", Highlight::Attribute);
    assert_highlighted(&line, "Element", Highlight::Type);
}

#[test]
fn javascript_and_jsx_are_highlighted() {
    let (_fixture, workbench, project) = open_file("main.js", "function add(a, b) { return a + 1; }\n");
    let line = spans(&workbench, project, 0);
    assert_highlighted(&line, "function", Highlight::Keyword);
    assert_highlighted(&line, "add", Highlight::Function);
    assert_highlighted(&line, "1", Highlight::Number);

    let (_fixture, workbench, project) = open_file("view.jsx", "export const v = <span id=\"a\" />;\n");
    let line = spans(&workbench, project, 0);
    assert_highlighted(&line, "span", Highlight::Tag);
    assert_highlighted(&line, "id", Highlight::Attribute);
}

#[test]
fn json_highlights_keys_and_values() {
    let (_fixture, workbench, project) = open_file("package.json", "{ \"name\": \"genea\", \"private\": true, \"n\": 1 }\n");

    let line = spans(&workbench, project, 0);

    assert_highlighted(&line, "\"name\"", Highlight::Property);
    assert_highlighted(&line, "\"genea\"", Highlight::String);
    assert_highlighted(&line, "true", Highlight::Constant);
    assert_highlighted(&line, "1", Highlight::Number);
}

#[test]
fn css_highlights_selectors_properties_and_values() {
    let (_fixture, workbench, project) = open_file("style.css", "/* c */\nbody .card { color: red; width: 10px; }\n");

    assert_highlighted(&spans(&workbench, project, 0), "/* c */", Highlight::Comment);
    let line = spans(&workbench, project, 1);
    assert_highlighted(&line, "body", Highlight::Tag);
    assert_highlighted(&line, "color", Highlight::Property);
    assert_highlighted(&line, "10", Highlight::Number);
}

#[test]
fn html_highlights_tags_and_the_script_inside() {
    let (_fixture, workbench, project) =
        open_file("index.html", "<p class=\"x\">hi</p>\n<script>\nconst a = 1;\n</script>\n");

    let line = spans(&workbench, project, 0);
    assert_highlighted(&line, "p", Highlight::Tag);
    assert_highlighted(&line, "class", Highlight::Attribute);
    assert_highlighted(&spans(&workbench, project, 2), "const", Highlight::Keyword);
}

#[test]
fn markdown_highlights_headings_inline_code_and_fenced_code() {
    let (_fixture, workbench, project) =
        open_file("README.md", "# Title\n\nSome `code` and **bold**.\n\n```ts\nlet x = 1;\n```\n");

    assert_highlighted(&spans(&workbench, project, 0), "Title", Highlight::Heading);
    let line = spans(&workbench, project, 2);
    assert!(line.iter().any(|(t, h)| t.contains("code") && *h == Highlight::Literal), "{line:?}");
    assert!(line.iter().any(|(t, h)| t.contains("bold") && *h == Highlight::Strong), "{line:?}");
    assert_highlighted(&spans(&workbench, project, 5), "let", Highlight::Keyword);
}

#[test]
fn yaml_highlights_keys_and_scalars() {
    let (_fixture, workbench, project) = open_file("pnpm-workspace.yaml", "packages:\n  - \"apps/*\" # all\n");

    assert_highlighted(&spans(&workbench, project, 0), "packages", Highlight::Property);
    let line = spans(&workbench, project, 1);
    assert_highlighted(&line, "\"apps/*\"", Highlight::String);
    assert_highlighted(&line, "# all", Highlight::Comment);
}

#[test]
fn env_files_highlight_comments_keys_and_values() {
    let (_fixture, workbench, project) = open_file(".env.local", "# secrets\nexport API_URL=http://localhost # dev\n");

    assert_highlighted(&spans(&workbench, project, 0), "# secrets", Highlight::Comment);
    let line = spans(&workbench, project, 1);
    assert_highlighted(&line, "export", Highlight::Keyword);
    assert_highlighted(&line, "API_URL", Highlight::Property);
    assert_highlighted(&line, "http://localhost", Highlight::String);
    assert_highlighted(&line, "# dev", Highlight::Comment);
}

#[test]
fn other_files_are_plain_text() {
    let (_fixture, workbench, project) = open_file("run.sh", "# not highlighted\necho \"hi\"\n");

    assert!(spans(&workbench, project, 0).is_empty());
    assert!(spans(&workbench, project, 1).is_empty());
}
