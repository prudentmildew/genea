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

#[test]
fn an_untracked_file_shows_as_all_added() {
    let fixture = repository(FixtureProject::new().file("main.ts", "one\n"));
    fixture.write("src/new.ts", "alpha\nbeta\n");
    let (mut workbench, project) = open(&fixture, "src/new.ts");

    workbench.dispatch(project, Command::ShowDiffAgainstHead);
    workbench.settle().unwrap();

    assert_eq!(rows(&editor(&workbench, project)), ["+alpha", "+beta", " "]);
}

#[test]
fn in_a_repository_without_a_commit_every_file_is_all_added() {
    let fixture = FixtureProject::new().file("main.ts", "one\n").build();
    git(fixture.root(), &["init", "--quiet"]);
    let (mut workbench, project) = open(&fixture, "main.ts");

    workbench.dispatch(project, Command::ShowDiffAgainstHead);
    workbench.settle().unwrap();

    assert_eq!(rows(&editor(&workbench, project)), ["+one", " "]);
}

#[test]
fn the_diff_follows_edits_in_the_buffer() {
    let fixture = repository(FixtureProject::new().file("main.ts", "one\ntwo\n"));
    let (mut workbench, project) = open(&fixture, "main.ts");
    workbench.dispatch(project, Command::ShowDiffAgainstHead);
    workbench.settle().unwrap();
    assert_eq!(rows(&editor(&workbench, project)), [" one", " two", " "]);

    type_at(&mut workbench, project, 1, 0, "2\n");
    workbench.settle().unwrap();

    assert_eq!(rows(&editor(&workbench, project)), [" one", "+2", " two", " "]);
}

#[test]
fn a_commit_in_the_terminal_moves_the_diff_to_the_new_head() {
    let fixture = repository(FixtureProject::new().file("main.ts", "one\ntwo\n"));
    fixture.write("main.ts", "one\n2\n");
    let (mut workbench, project) = open(&fixture, "main.ts");
    workbench.dispatch(project, Command::ShowDiffAgainstHead);
    workbench.settle().unwrap();
    assert_eq!(rows(&editor(&workbench, project)), [" one", "-two", "+2", " "]);

    git(fixture.root(), &["commit", "--quiet", "--all", "--message", "Two"]);
    workbench.settle().unwrap();

    let view = editor(&workbench, project);
    assert_eq!(rows(&view), [" one", " 2", " "]);
    assert_eq!(view.inline_diff.unwrap().against, DiffAgainst::Head);
}

#[test]
fn close_inline_diff_shows_the_file_plainly() {
    let fixture = repository(FixtureProject::new().file("main.ts", "one\n"));
    fixture.write("main.ts", "1\n");
    let (mut workbench, project) = open(&fixture, "main.ts");
    workbench.dispatch(project, Command::ShowDiffAgainstHead);
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::CloseInlineDiff);
    workbench.settle().unwrap();

    let view = editor(&workbench, project);
    assert_eq!(view.inline_diff, None);
    assert_eq!(view.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["1", ""]);
}

#[test]
fn outside_a_repository_a_notice_says_so_and_the_file_shows_plainly() {
    let fixture = FixtureProject::new().file("main.ts", "one\n").build();
    let (mut workbench, project) = open(&fixture, "main.ts");

    workbench.dispatch(project, Command::ShowDiffAgainstHead);
    workbench.settle().unwrap();

    let view = workbench.project(project).unwrap();
    assert_eq!(view.editor.unwrap().inline_diff, None);
    let notices: Vec<_> = view.notices.iter().map(|n| n.message.as_str()).collect();
    assert_eq!(notices, ["Can't show main.ts against HEAD: the project isn't in a git repository."]);
}
