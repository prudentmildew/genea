//! Read-only git (ticket #56): the branch in the status bar, gutter markers
//! comparing the live buffer with HEAD, and rolling back a hunk.
//!
//! Fixture repositories are made with the `git` binary, as a user would in
//! the terminal; Genea itself reads them in-process.

use std::{path::Path, process};

use genea_core::{CaretMove, Command, HunkView, LineChange, ProjectId, Workbench};
use genea_testkit::{FixtureBuilder, FixtureProject, TestHost};

/// Runs `git` in `root`, isolated from the user's and the system's config.
fn git(root: &Path, args: &[&str]) {
    let output = process::Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .expect("run git");
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
}

/// A fixture whose files are committed on `branch`.
fn repository(fixture: FixtureBuilder, branch: &str) -> FixtureProject {
    let fixture = fixture.build();
    git(fixture.root(), &["init", "--quiet", "--initial-branch", branch]);
    git(fixture.root(), &["add", "--all"]);
    git(fixture.root(), &["commit", "--quiet", "--message", "Initial commit"]);
    fixture
}

fn open(fixture: &FixtureProject) -> (Workbench, ProjectId) {
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    (workbench, project)
}

fn branch(workbench: &Workbench, project: ProjectId) -> Option<String> {
    workbench.project(project).unwrap().status.branch
}

#[test]
fn the_status_bar_shows_the_repositorys_branch() {
    let fixture = repository(FixtureProject::new().file("src/main.ts", "let a = 1;\n"), "feature/login");
    let (workbench, project) = open(&fixture);
    assert_eq!(branch(&workbench, project).as_deref(), Some("feature/login"));
}

#[test]
fn a_checkout_in_the_terminal_updates_the_branch() {
    let fixture = repository(FixtureProject::new().file("src/main.ts", "let a = 1;\n"), "main");
    let (mut workbench, project) = open(&fixture);

    git(fixture.root(), &["checkout", "--quiet", "-b", "feature/search"]);
    workbench.settle().unwrap();
    assert_eq!(branch(&workbench, project).as_deref(), Some("feature/search"));

    git(fixture.root(), &["checkout", "--quiet", "main"]);
    workbench.settle().unwrap();
    assert_eq!(branch(&workbench, project).as_deref(), Some("main"));
}

const MAIN: &str = "one\ntwo\nthree\nfour\n";

/// A repository with `src/main.ts` committed as [`MAIN`], open in the editor.
fn editing_main() -> (FixtureProject, Workbench, ProjectId) {
    let fixture = repository(FixtureProject::new().file("src/main.ts", MAIN), "main");
    let (mut workbench, project) = open(&fixture);
    workbench.dispatch(project, Command::OpenFile("src/main.ts".into()));
    workbench.settle().unwrap();
    (fixture, workbench, project)
}

/// The gutter markers on the visible lines, as `(line, change)`.
fn markers(workbench: &Workbench, project: ProjectId) -> Vec<(usize, LineChange)> {
    let editor = workbench.project(project).unwrap().editor.unwrap();
    editor.gutter.iter().map(|marker| (marker.line, marker.change)).collect()
}

/// The buffer's text, from its visible lines.
fn text(workbench: &Workbench, project: ProjectId) -> String {
    let editor = workbench.project(project).unwrap().editor.unwrap();
    editor.lines.iter().map(|line| line.text.as_str()).collect::<Vec<_>>().join("\n")
}

fn type_at(workbench: &mut Workbench, project: ProjectId, line: usize, column: usize, text: &str) {
    workbench.dispatch(project, Command::PlaceCaret { line, column });
    workbench.dispatch(project, Command::InsertText(text.into()));
}

#[test]
fn a_file_as_it_is_at_head_has_no_markers() {
    let (_fixture, workbench, project) = editing_main();
    assert_eq!(markers(&workbench, project), []);
}

#[test]
fn typing_in_a_line_marks_it_modified() {
    let (_fixture, mut workbench, project) = editing_main();
    type_at(&mut workbench, project, 1, 3, "!");
    workbench.settle().unwrap();
    assert_eq!(markers(&workbench, project), [(1, LineChange::Modified)]);
}

#[test]
fn new_lines_are_marked_added() {
    let (_fixture, mut workbench, project) = editing_main();
    type_at(&mut workbench, project, 2, 0, "two and a half\nnearly three\n");
    workbench.settle().unwrap();
    assert_eq!(markers(&workbench, project), [(2, LineChange::Added), (3, LineChange::Added)]);
}

#[test]
fn deleted_lines_are_marked_on_the_line_below_them() {
    let (_fixture, mut workbench, project) = editing_main();
    workbench.dispatch(project, Command::SelectLine { line: 1 });
    workbench.dispatch(project, Command::Select(CaretMove::Down));
    workbench.dispatch(project, Command::Delete(CaretMove::Left));
    workbench.settle().unwrap();
    assert_eq!(text(&workbench, project), "one\nfour\n");
    assert_eq!(markers(&workbench, project), [(1, LineChange::Deleted)]);

    // At the end of the file, the empty last line is below them.
    workbench.dispatch(project, Command::SelectLine { line: 1 });
    workbench.dispatch(project, Command::Delete(CaretMove::Left));
    workbench.settle().unwrap();
    assert_eq!(text(&workbench, project), "one\n");
    assert_eq!(markers(&workbench, project), [(1, LineChange::Deleted)]);
}

#[test]
fn committing_in_the_terminal_clears_the_markers_of_what_was_committed() {
    let (fixture, mut workbench, project) = editing_main();
    type_at(&mut workbench, project, 1, 3, "!");
    workbench.dispatch(project, Command::Save);
    workbench.settle().unwrap();
    assert_eq!(markers(&workbench, project), [(1, LineChange::Modified)]);

    git(fixture.root(), &["commit", "--quiet", "--all", "--message", "Exclaim"]);
    workbench.settle().unwrap();
    assert_eq!(markers(&workbench, project), []);
}

#[test]
fn a_project_without_a_repository_has_no_branch_no_markers_and_no_errors() {
    let fixture = FixtureProject::new().file("src/main.ts", MAIN).build();
    let (mut workbench, project) = open(&fixture);
    workbench.dispatch(project, Command::OpenFile("src/main.ts".into()));
    workbench.settle().unwrap();
    type_at(&mut workbench, project, 1, 3, "!");
    workbench.settle().unwrap();

    let view = workbench.project(project).unwrap();
    assert_eq!(view.status.branch, None);
    assert_eq!(markers(&workbench, project), []);
    assert_eq!(view.notices, []);
    assert_eq!(view.problems, []);
}

#[test]
fn a_file_that_isnt_in_head_has_no_markers() {
    let (fixture, mut workbench, project) = editing_main();
    fixture.write("src/new.ts", "let b = 2;\n");
    workbench.dispatch(project, Command::OpenFile("src/new.ts".into()));
    workbench.settle().unwrap();
    type_at(&mut workbench, project, 0, 0, "// New\n");
    workbench.settle().unwrap();
    assert_eq!(markers(&workbench, project), []);
}

#[test]
fn git_init_in_the_terminal_shows_the_branch() {
    let fixture = FixtureProject::new().file("src/main.ts", MAIN).build();
    let (mut workbench, project) = open(&fixture);
    git(fixture.root(), &["init", "--quiet", "--initial-branch", "trunk"]);
    workbench.settle().unwrap();
    assert_eq!(branch(&workbench, project).as_deref(), Some("trunk"));
}

fn shown_hunk(workbench: &Workbench, project: ProjectId) -> Option<HunkView> {
    workbench.project(project).unwrap().editor.unwrap().hunk
}

#[test]
fn clicking_a_marker_shows_the_lines_at_head() {
    let (_fixture, mut workbench, project) = editing_main();
    workbench.dispatch(project, Command::SelectLine { line: 1 });
    workbench.dispatch(project, Command::Select(CaretMove::Down));
    workbench.dispatch(project, Command::InsertText("2\n3\n".into()));
    workbench.settle().unwrap();
    assert_eq!(text(&workbench, project), "one\n2\n3\nfour\n");
    assert_eq!(shown_hunk(&workbench, project), None);

    workbench.dispatch(project, Command::ShowHunk { line: 2 });
    assert_eq!(
        shown_hunk(&workbench, project),
        Some(HunkView { lines: 1..3, change: LineChange::Modified, head: "two\nthree".into() })
    );

    workbench.dispatch(project, Command::HideHunk);
    assert_eq!(shown_hunk(&workbench, project), None);
}

#[test]
fn the_shown_hunk_closes_on_a_click_or_typing_but_not_on_scrolling() {
    let (_fixture, mut workbench, project) = editing_main();
    type_at(&mut workbench, project, 0, 0, "zero\n");
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::ShowHunk { line: 0 });
    workbench.dispatch(project, Command::ScrollBy { rows: 1.0 });
    assert_eq!(shown_hunk(&workbench, project), Some(HunkView { lines: 0..1, change: LineChange::Added, head: "".into() }));

    workbench.dispatch(project, Command::PlaceCaret { line: 2, column: 0 });
    assert_eq!(shown_hunk(&workbench, project), None);

    workbench.dispatch(project, Command::ShowHunk { line: 0 });
    workbench.dispatch(project, Command::InsertText("x".into()));
    workbench.settle().unwrap();
    assert_eq!(shown_hunk(&workbench, project), None);
}

#[test]
fn a_line_without_a_marker_shows_nothing() {
    let (_fixture, mut workbench, project) = editing_main();
    type_at(&mut workbench, project, 0, 0, "zero\n");
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::ShowHunk { line: 2 });
    assert_eq!(shown_hunk(&workbench, project), None);
}

#[test]
fn every_change_in_the_file_has_its_markers() {
    let (_fixture, mut workbench, project) = editing_main();
    type_at(&mut workbench, project, 0, 0, "zero\n");
    type_at(&mut workbench, project, 4, 4, "!");
    workbench.settle().unwrap();
    assert_eq!(text(&workbench, project), "zero\none\ntwo\nthree\nfour!\n");
    assert_eq!(markers(&workbench, project), [(0, LineChange::Added), (4, LineChange::Modified)]);
}
