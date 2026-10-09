//! Syntax highlighting (ticket #24): highlight spans in view state, for the
//! first-class languages and the basic file types, kept in step with edits.

use genea_core::{Command, Highlight, ProjectId, Workbench};
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
