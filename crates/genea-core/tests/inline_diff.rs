//! The inline diff (ticket #55): a file opened from Changes shows its
//! removed and added lines interleaved in the editor, against its review
//! baseline, with Keep and Revert.

use genea_core::{ChangeKind, Command, EditorView, ProjectId, Workbench};
use genea_testkit::{FixtureProject, TestHost};

/// Opens a project with these files and waits for the first snapshot.
fn open(files: &[(&str, &str)]) -> (FixtureProject, Workbench, ProjectId) {
    let fixture = files.iter().fold(FixtureProject::new(), |f, (path, text)| f.file(path, text)).build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    (fixture, workbench, project)
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

#[test]
fn opening_a_changed_file_from_changes_shows_removed_and_added_lines_interleaved() {
    let (fixture, mut workbench, project) = open(&[("main.ts", "one\ntwo\nthree\nfour\n")]);
    fixture.write("main.ts", "one\n2\nthree\nfour\nfive\n");
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::OpenChange("main.ts".into()));
    workbench.settle().unwrap();

    let view = editor(&workbench, project);
    assert_eq!(view.path, std::path::Path::new("main.ts"));
    assert_eq!(rows(&view), [" one", "-two", "+2", " three", " four", "+five", " "]);
}

#[test]
fn the_diff_offers_keep_and_revert_and_keep_accepts_the_change() {
    let (fixture, mut workbench, project) = open(&[("main.ts", "one\ntwo\n")]);
    fixture.write("main.ts", "one\n2\n");
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::OpenChange("main.ts".into()));
    workbench.settle().unwrap();

    let diff = editor(&workbench, project).inline_diff.unwrap();
    let change = diff.change.expect("the file's entry in Changes");
    assert_eq!((change.kind, change.can_revert), (ChangeKind::Modified, true));

    workbench.dispatch(project, Command::KeepChange(change.path));
    workbench.settle().unwrap();

    assert_eq!(workbench.project(project).unwrap().changes.len(), 0);
    let view = editor(&workbench, project);
    assert_eq!(view.inline_diff, None);
    assert_eq!(view.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["one", "2", ""]);
    assert_eq!(fixture.read("main.ts"), "one\n2\n");
}

#[test]
fn revert_from_the_diff_restores_the_baseline_and_shows_the_file_plainly() {
    let (fixture, mut workbench, project) = open(&[("main.ts", "one\ntwo\n")]);
    fixture.write("main.ts", "one\n2\n");
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::OpenChange("main.ts".into()));
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::RevertChange("main.ts".into()));
    workbench.settle().unwrap();

    assert_eq!(fixture.read("main.ts"), "one\ntwo\n");
    assert_eq!(workbench.project(project).unwrap().changes.len(), 0);
    let view = editor(&workbench, project);
    assert_eq!(view.inline_diff, None);
    assert_eq!(view.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["one", "two", ""]);
}

#[test]
fn the_diff_follows_edits_in_the_buffer() {
    let (fixture, mut workbench, project) = open(&[("main.ts", "one\ntwo\n")]);
    fixture.write("main.ts", "one\n2\n");
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::OpenChange("main.ts".into()));
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::PlaceCaret { line: 1, column: 0 });
    workbench.dispatch(project, Command::InsertText("two\n".into()));
    workbench.settle().unwrap();

    assert_eq!(rows(&editor(&workbench, project)), [" one", " two", "+2", " "]);
}

#[test]
fn scrolling_counts_removed_rows_and_carets_skip_them() {
    let base: String = (1..=10).map(|n| format!("line {n}\n")).collect();
    let (fixture, mut workbench, project) = open(&[("main.ts", &base)]);
    fixture.write("main.ts", "line 1\nline 10\n");
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::SetViewport { rows: 4.0 });
    workbench.dispatch(project, Command::OpenChange("main.ts".into()));
    workbench.settle().unwrap();

    // Rows: line 1, eight removed lines, line 10, the empty last line.
    workbench.dispatch(project, Command::MoveCaret(genea_core::CaretMove::Down));
    let view = editor(&workbench, project);
    assert_eq!(view.caret.line, 1);
    assert_eq!(view.scroll_top, 6.0);
    assert_eq!(rows(&view), ["-line 7", "-line 8", "-line 9", " line 10"]);

    workbench.dispatch(project, Command::ScrollBy { rows: 100.0 });
    assert_eq!(rows(&editor(&workbench, project)), ["-line 8", "-line 9", " line 10", " "]);
    assert_eq!(editor(&workbench, project).scroll_top, 7.0);
}

#[test]
fn removed_lines_inside_a_collapsed_fold_are_hidden_with_it() {
    let (fixture, mut workbench, project) =
        open(&[("main.ts", "function f() {\n  a();\n  b();\n}\nend();\n")]);
    fixture.write("main.ts", "function f() {\n  b();\n}\nfinish();\n");
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::OpenChange("main.ts".into()));
    workbench.settle().unwrap();
    assert_eq!(
        rows(&editor(&workbench, project)),
        [" function f() {", "-  a();", "   b();", " }", "-end();", "+finish();", " "]
    );

    workbench.dispatch(project, Command::CollapseFold);
    workbench.settle().unwrap();

    let view = editor(&workbench, project);
    assert_eq!(rows(&view), [" function f() {", " }", "-end();", "+finish();", " "]);
    assert_eq!(view.lines.iter().map(|l| (l.index, l.row)).collect::<Vec<_>>(), [(0, 0), (2, 1), (3, 3), (4, 4)]);
}

#[test]
fn a_file_not_in_changes_opens_plainly() {
    let (_fixture, mut workbench, project) = open(&[("main.ts", "one\n")]);

    workbench.dispatch(project, Command::OpenChange("main.ts".into()));
    workbench.settle().unwrap();

    let view = editor(&workbench, project);
    assert_eq!(view.inline_diff, None);
    assert_eq!(view.lines[0].text, "one");
}

#[test]
fn a_created_file_shows_as_all_added() {
    let (fixture, mut workbench, project) = open(&[("main.ts", "one\n")]);
    fixture.write("src/new.ts", "alpha\nbeta\n");
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::OpenChange("src/new.ts".into()));
    workbench.settle().unwrap();

    assert_eq!(rows(&editor(&workbench, project)), ["+alpha", "+beta", " "]);
}

#[test]
fn a_deleted_file_opens_read_only_and_shows_as_all_removed() {
    let (fixture, mut workbench, project) = open(&[("main.ts", "one\n"), ("old.ts", "first\nsecond\n")]);
    fixture.remove("old.ts");
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::OpenChange("old.ts".into()));
    workbench.settle().unwrap();

    let view = editor(&workbench, project);
    assert_eq!(view.path, std::path::Path::new("old.ts"));
    assert!(view.read_only);
    assert_eq!(rows(&view), ["-first", "-second", " "]);
}

#[test]
fn reverting_a_deleted_file_from_its_diff_restores_it_and_closes_the_diff() {
    let (fixture, mut workbench, project) = open(&[("main.ts", "one\n"), ("old.ts", "first\nsecond\n")]);
    fixture.remove("old.ts");
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::OpenFile("main.ts".into()));
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::OpenChange("old.ts".into()));
    workbench.settle().unwrap();
    let change = editor(&workbench, project).inline_diff.unwrap().change.unwrap();
    assert_eq!((change.kind, change.can_revert), (ChangeKind::Deleted, true));

    workbench.dispatch(project, Command::RevertChange("old.ts".into()));
    workbench.settle().unwrap();

    assert_eq!(fixture.read("old.ts"), "first\nsecond\n");
    let view = workbench.project(project).unwrap();
    assert_eq!(view.changes.len(), 0);
    let tabs: Vec<_> = view.panes[0].tabs.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(tabs, ["main.ts"]);
}

#[test]
fn keeping_a_deleted_file_from_its_diff_closes_the_diff() {
    let (fixture, mut workbench, project) = open(&[("old.ts", "first\n")]);
    fixture.remove("old.ts");
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::OpenChange("old.ts".into()));
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::KeepChange("old.ts".into()));
    workbench.settle().unwrap();

    assert!(!fixture.path("old.ts").exists());
    let view = workbench.project(project).unwrap();
    assert_eq!(view.changes.len(), 0);
    assert_eq!(view.editor, None);
}

#[test]
fn close_inline_diff_shows_the_file_plainly_and_open_change_shows_it_again() {
    let (fixture, mut workbench, project) = open(&[("main.ts", "one\ntwo\n")]);
    fixture.write("main.ts", "one\n2\n");
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::OpenChange("main.ts".into()));
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::CloseInlineDiff);
    workbench.settle().unwrap();
    let view = editor(&workbench, project);
    assert_eq!(view.inline_diff, None);
    let lines: Vec<_> = view.lines.iter().map(|l| (l.row, l.text.as_str())).collect();
    assert_eq!(lines, [(0, "one"), (1, "2"), (2, "")]);
    // Still listed: closing the diff isn't a review.
    assert_eq!(workbench.project(project).unwrap().changes.len(), 1);

    workbench.dispatch(project, Command::OpenChange("main.ts".into()));
    workbench.settle().unwrap();
    assert_eq!(rows(&editor(&workbench, project)), [" one", "-two", "+2", " "]);
}
