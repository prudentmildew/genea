//! The Problems view, the left column's view switcher, "Open config", and
//! how problems show in the editor and the status bar (ticket #29).

use genea_core::{Command, InlineProblem, LeftColumnView, ProjectId, Severity, TextPosition, Workbench};
use genea_testkit::{FixtureBuilder, FixtureProject, TestHost};

fn open(fixture: FixtureBuilder) -> (FixtureProject, Workbench, ProjectId) {
    let fixture = fixture.build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    (fixture, workbench, project)
}

#[test]
fn the_problems_shortcut_shows_problems_and_collapses_the_column_when_pressed_again() {
    let (_fixture, mut workbench, project) = open(FixtureProject::new());
    let left_column = |workbench: &Workbench| workbench.project(project).unwrap().left_column;

    workbench.dispatch(project, Command::ToggleLeftColumn(LeftColumnView::Problems));
    assert_eq!(left_column(&workbench), Some(LeftColumnView::Problems));

    workbench.dispatch(project, Command::ToggleLeftColumn(LeftColumnView::Problems));
    assert_eq!(left_column(&workbench), None);

    workbench.dispatch(project, Command::ToggleLeftColumn(LeftColumnView::Problems));
    assert_eq!(left_column(&workbench), Some(LeftColumnView::Problems));
}

#[test]
fn open_config_creates_an_empty_config_when_there_is_none() {
    let (fixture, mut workbench, project) = open(FixtureProject::new());

    workbench.dispatch(project, Command::OpenConfig);
    workbench.settle().unwrap();

    assert_eq!(fixture.read("genea.jsonc"), "{}\n");
    let editor = workbench.project(project).unwrap().editor.unwrap();
    assert_eq!(editor.title, "genea.jsonc");
    assert_eq!(editor.lines[0].text, "{}");
}

#[test]
fn open_config_opens_an_existing_config_as_it_is() {
    let (fixture, mut workbench, project) = open(FixtureProject::new().file("genea.jsonc", "{ \"theme\": \"dark\" }"));

    workbench.dispatch(project, Command::OpenConfig);
    workbench.settle().unwrap();

    assert_eq!(fixture.read("genea.jsonc"), "{ \"theme\": \"dark\" }");
    assert_eq!(workbench.project(project).unwrap().editor.unwrap().lines[0].text, "{ \"theme\": \"dark\" }");
}

const BAD_CONFIG: &str = "{\n  // Two mistakes.\n  \"theme\": \"blue\",\n  \"fontSize\": 14\n}\n";

#[test]
fn clicking_a_problem_opens_its_file_at_the_problem() {
    let (_fixture, mut workbench, project) = open(FixtureProject::new().file("genea.jsonc", BAD_CONFIG));
    let item = workbench.project(project).unwrap().problems[1].clone();
    assert_eq!(item.location, "4:3");

    workbench.dispatch(project, Command::OpenFileAt { path: item.path, at: item.position });
    workbench.settle().unwrap();

    let view = workbench.project(project).unwrap();
    assert_eq!(view.editor.unwrap().title, "genea.jsonc");
    assert_eq!(view.status.caret.as_deref(), Some("4:3"));
}

#[test]
fn open_file_at_moves_the_caret_in_an_open_file_and_keeps_its_edits() {
    let (_fixture, mut workbench, project) = open(FixtureProject::new().file("main.ts", "let a = 1;\nlet b = 2;\n"));
    workbench.dispatch(project, Command::OpenFile("main.ts".into()));
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::InsertText("x".into()));

    workbench.dispatch(
        project,
        Command::OpenFileAt { path: "main.ts".into(), at: TextPosition { line: 1, column: 4 } },
    );
    workbench.settle().unwrap();

    let view = workbench.project(project).unwrap();
    assert_eq!(view.status.caret.as_deref(), Some("2:5"));
    let editor = view.editor.unwrap();
    assert_eq!(editor.lines[0].text, "xlet a = 1;");
    assert!(editor.modified);
}

#[test]
fn a_bad_config_shows_inline_in_its_file() {
    let (_fixture, mut workbench, project) = open(FixtureProject::new().file("genea.jsonc", BAD_CONFIG));

    workbench.dispatch(project, Command::OpenConfig);
    workbench.settle().unwrap();

    let editor = workbench.project(project).unwrap().editor.unwrap();
    assert_eq!(
        editor.problems,
        [
            InlineProblem {
                line: 2,
                columns: 11..17,
                severity: Severity::Error,
                message: r#""theme" must be "system", "light" or "dark". Using the default."#.into(),
            },
            InlineProblem {
                line: 3,
                columns: 2..12,
                severity: Severity::Warning,
                message: r#"Unknown key "fontSize". It is ignored."#.into(),
            },
        ]
    );
}

#[test]
fn a_bad_config_shows_in_the_status_bar_until_it_is_fixed() {
    let (fixture, mut workbench, project) = open(FixtureProject::new().file("genea.jsonc", BAD_CONFIG));

    let status = workbench.project(project).unwrap().status;
    assert_eq!((status.errors, status.warnings), (1, 1));
    assert_eq!(status.config_notice.as_deref(), Some("genea.jsonc has 1 error and 1 warning"));

    fixture.write("genea.jsonc", "{}");
    workbench.settle().unwrap();

    let status = workbench.project(project).unwrap().status;
    assert_eq!((status.errors, status.warnings), (0, 0));
    assert_eq!(status.config_notice, None);
}
