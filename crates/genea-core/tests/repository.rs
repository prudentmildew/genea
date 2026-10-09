//! Read-only git (ticket #56): the branch in the status bar, gutter markers
//! comparing the live buffer with HEAD, and rolling back a hunk.
//!
//! Fixture repositories are made with the `git` binary, as a user would in
//! the terminal; Genea itself reads them in-process.

use std::{path::Path, process};

use genea_core::{Command, LineChange, ProjectId, Workbench};
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
