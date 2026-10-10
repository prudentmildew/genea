//! "Show Diff Against HEAD" (ticket #57): the focused file as an inline diff
//! against its text at HEAD, the review's inline diff without Keep and
//! Revert.
//!
//! Fixture repositories are made with the `git` binary, as in
//! `tests/repository.rs`; Genea itself reads them in-process.

use std::{path::Path, process};

use genea_core::{Command, DiffAgainst, EditorView, ProjectId, Workbench};
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

/// A fixture whose files are committed.
fn repository(fixture: FixtureBuilder) -> FixtureProject {
    let fixture = fixture.build();
    git(fixture.root(), &["init", "--quiet", "--initial-branch", "main"]);
    git(fixture.root(), &["add", "--all"]);
    git(fixture.root(), &["commit", "--quiet", "--message", "Initial commit"]);
    fixture
}

/// Opens the project with `path` in the editor.
fn open(fixture: &FixtureProject, path: &str) -> (Workbench, ProjectId) {
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::OpenFile(path.into()));
    workbench.settle().unwrap();
    (workbench, project)
}

fn editor(workbench: &Workbench, project: ProjectId) -> EditorView {
    workbench.project(project).unwrap().editor.expect("an open editor")
}

/// The editor's rows as the user reads them, top to bottom: `-` for a
/// removed line, `+` for an added one, a space for an unchanged one.
fn rows(view: &EditorView) -> Vec<String> {
    let diff = view.inline_diff.as_ref().expect("an inline diff");
    let mut rows: Vec<(usize, String)> = view
        .lines
        .iter()
        .map(|line| {
            let mark = if diff.added.contains(&line.index) { '+' } else { ' ' };
            (line.row, format!("{mark}{}", line.text))
        })
        .chain(diff.removed.iter().map(|removed| (removed.row, format!("-{}", removed.text))))
        .collect();
    rows.sort();
    rows.into_iter().map(|(_, text)| text).collect()
}

fn type_at(workbench: &mut Workbench, project: ProjectId, line: usize, column: usize, text: &str) {
    workbench.dispatch(project, Command::PlaceCaret { line, column });
    workbench.dispatch(project, Command::InsertText(text.into()));
}

#[test]
fn the_diff_against_head_shows_the_files_changes_since_the_last_commit() {
    let fixture = repository(FixtureProject::new().file("main.ts", "one\ntwo\nthree\n"));
    fixture.write("main.ts", "one\n2\nthree\n");
    let (mut workbench, project) = open(&fixture, "main.ts");
    type_at(&mut workbench, project, 2, 5, "!");

    workbench.dispatch(project, Command::ShowDiffAgainstHead);
    workbench.settle().unwrap();

    let view = editor(&workbench, project);
    assert_eq!(rows(&view), [" one", "-two", "-three", "+2", "+three!", " "]);
    let diff = view.inline_diff.unwrap();
    assert_eq!(diff.against, DiffAgainst::Head);
    // No Keep and Revert: HEAD isn't a review.
    assert_eq!(diff.change, None);
}
