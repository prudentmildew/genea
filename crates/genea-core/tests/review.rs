//! Review (ticket #53): changes made on disk by anything but Genea are
//! listed in Changes against the review baseline, and Keep or Revert settles
//! them, per file or for all files.

use std::path::PathBuf;

use genea_core::{
    ChangeItem, ChangeKind, Command, FinderMode, LARGE_FILE_BYTES, LeftColumnView, ProjectId, RevertAllPrompt,
    ToolchainPickerKind, Workbench,
};
use genea_testkit::{FixtureProject, TestHost};

/// Opens a project with these files and waits for the first snapshot.
fn open(files: &[(&str, &str)]) -> (FixtureProject, Workbench, ProjectId) {
    let fixture = files.iter().fold(FixtureProject::new(), |f, (path, text)| f.file(path, text)).build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    (fixture, workbench, project)
}

/// The Changes list as (path, kind).
fn changes(workbench: &Workbench, project: ProjectId) -> Vec<(PathBuf, ChangeKind)> {
    items(workbench, project).into_iter().map(|c| (c.path, c.kind)).collect()
}

fn items(workbench: &Workbench, project: ProjectId) -> Vec<ChangeItem> {
    workbench.project(project).unwrap().changes.to_vec()
}

fn banner(workbench: &Workbench, project: ProjectId) -> Option<String> {
    workbench.project(project).unwrap().review_banner
}

#[test]
fn an_external_edit_is_listed_in_changes_and_the_banner_shows() {
    let (fixture, mut workbench, project) = open(&[("src/main.ts", "let a = 1;\n"), ("README.md", "hi\n")]);
    assert_eq!(changes(&workbench, project), []);
    assert_eq!(banner(&workbench, project), None);

    fixture.write("src/main.ts", "let a = 2;\n");
    workbench.settle().unwrap();

    assert_eq!(changes(&workbench, project), [("src/main.ts".into(), ChangeKind::Modified)]);
    assert_eq!(banner(&workbench, project).as_deref(), Some("1 file changed outside Genea"));
}

#[test]
fn created_and_deleted_files_are_listed_and_a_rename_is_both() {
    let (fixture, mut workbench, project) = open(&[("a.ts", "a\n"), ("old.ts", "old\n"), ("gone.ts", "gone\n")]);

    fixture.write("src/new.ts", "new\n");
    fixture.remove("gone.ts");
    std::fs::rename(fixture.path("old.ts"), fixture.path("renamed.ts")).unwrap();
    workbench.settle().unwrap();

    assert_eq!(
        changes(&workbench, project),
        [
            ("gone.ts".into(), ChangeKind::Deleted),
            ("old.ts".into(), ChangeKind::Deleted),
            ("renamed.ts".into(), ChangeKind::Created),
            ("src/new.ts".into(), ChangeKind::Created),
        ]
    );
    assert_eq!(banner(&workbench, project).as_deref(), Some("4 files changed outside Genea"));
}

#[test]
fn a_folder_moved_out_or_in_lists_every_file_in_it() {
    let (fixture, mut workbench, project) = open(&[("lib/a.ts", "a\n"), ("lib/b.ts", "b\n")]);
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(outside.path().join("pkg/src")).unwrap();
    std::fs::write(outside.path().join("pkg/src/c.ts"), "c\n").unwrap();

    std::fs::rename(fixture.path("lib"), outside.path().join("lib")).unwrap();
    std::fs::rename(outside.path().join("pkg"), fixture.path("pkg")).unwrap();
    workbench.settle().unwrap();

    assert_eq!(
        changes(&workbench, project),
        [
            ("lib/a.ts".into(), ChangeKind::Deleted),
            ("lib/b.ts".into(), ChangeKind::Deleted),
            ("pkg/src/c.ts".into(), ChangeKind::Created),
        ]
    );
}

#[test]
fn edits_saved_in_genea_are_never_listed_and_move_the_baseline() {
    let (fixture, mut workbench, project) = open(&[("main.ts", "let a = 1;\n")]);
    workbench.dispatch(project, Command::OpenFile("main.ts".into()));
    workbench.settle().unwrap();

    for text in ["x", "y", "z"] {
        workbench.dispatch(project, Command::InsertText(text.into()));
        workbench.dispatch(project, Command::Save);
        workbench.settle().unwrap();
    }
    assert_eq!(fixture.read("main.ts"), "xyzlet a = 1;\n");
    assert_eq!(changes(&workbench, project), []);
    assert_eq!(banner(&workbench, project), None);

    // The baseline is the saved text now, so going back is a change.
    fixture.write("main.ts", "let a = 1;\n");
    workbench.settle().unwrap();
    assert_eq!(changes(&workbench, project), [("main.ts".into(), ChangeKind::Modified)]);
}

#[test]
fn saving_a_file_with_a_pending_change_keeps_it_listed_against_the_old_baseline() {
    let (fixture, mut workbench, project) = open(&[("main.ts", "one\n")]);
    fixture.write("main.ts", "two\n");
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::OpenFile("main.ts".into()));
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::InsertText("three ".into()));
    workbench.dispatch(project, Command::Save);
    workbench.settle().unwrap();
    assert_eq!(changes(&workbench, project), [("main.ts".into(), ChangeKind::Modified)]);

    workbench.dispatch(project, Command::RevertChange("main.ts".into()));
    workbench.settle().unwrap();
    assert_eq!(fixture.read("main.ts"), "one\n", "the baseline is still from before the external change");
    assert_eq!(changes(&workbench, project), []);
}

#[test]
fn creating_the_config_from_genea_is_not_listed() {
    let (fixture, mut workbench, project) = open(&[("main.ts", "a\n")]);

    workbench.dispatch(project, Command::OpenConfig);
    workbench.settle().unwrap();

    assert_eq!(fixture.read("genea.jsonc"), "{}\n");
    assert_eq!(changes(&workbench, project), []);
}

/// Opens a project with `kept.ts`, `edited.ts` and `deleted.ts`, then edits
/// `edited.ts`, deletes `deleted.ts` (and its folder) and creates
/// `created.ts` outside Genea.
fn three_changes() -> (FixtureProject, Workbench, ProjectId) {
    let (fixture, mut workbench, project) =
        open(&[("kept.ts", "kept\n"), ("edited.ts", "before\n"), ("old/deleted.ts", "deleted\n")]);
    fixture.write("edited.ts", "after\n");
    std::fs::remove_dir_all(fixture.path("old")).unwrap();
    fixture.write("created.ts", "created\n");
    workbench.settle().unwrap();
    assert_eq!(
        changes(&workbench, project),
        [
            ("created.ts".into(), ChangeKind::Created),
            ("edited.ts".into(), ChangeKind::Modified),
            ("old/deleted.ts".into(), ChangeKind::Deleted),
        ]
    );
    (fixture, workbench, project)
}

#[test]
fn keep_makes_the_disk_content_the_baseline_for_each_kind_of_change() {
    let (fixture, mut workbench, project) = three_changes();

    for path in ["created.ts", "edited.ts", "old/deleted.ts"] {
        workbench.dispatch(project, Command::KeepChange(path.into()));
    }
    workbench.settle().unwrap();
    assert_eq!(changes(&workbench, project), []);
    assert_eq!(banner(&workbench, project), None);
    assert_eq!(fixture.read("edited.ts"), "after\n", "Keep doesn't touch the disk");
    assert!(fixture.path("created.ts").exists());
    assert!(!fixture.path("old/deleted.ts").exists());

    // Further changes are against the kept contents.
    fixture.write("edited.ts", "before\n");
    fixture.remove("created.ts");
    fixture.write("old/deleted.ts", "deleted\n");
    workbench.settle().unwrap();
    assert_eq!(
        changes(&workbench, project),
        [
            ("created.ts".into(), ChangeKind::Deleted),
            ("edited.ts".into(), ChangeKind::Modified),
            ("old/deleted.ts".into(), ChangeKind::Created),
        ]
    );
}

#[test]
fn revert_writes_the_baseline_back_for_each_kind_of_change() {
    let (fixture, mut workbench, project) = three_changes();

    for path in ["created.ts", "edited.ts", "old/deleted.ts"] {
        workbench.dispatch(project, Command::RevertChange(path.into()));
    }
    workbench.settle().unwrap();

    assert_eq!(changes(&workbench, project), []);
    assert_eq!(fixture.read("edited.ts"), "before\n");
    assert!(!fixture.path("created.ts").exists(), "a created file is deleted");
    assert_eq!(fixture.read("old/deleted.ts"), "deleted\n", "a deleted file is restored, with its folder");
}

#[test]
fn keep_and_revert_act_only_on_their_own_file() {
    let (fixture, mut workbench, project) = three_changes();

    workbench.dispatch(project, Command::KeepChange("edited.ts".into()));
    workbench.dispatch(project, Command::RevertChange("created.ts".into()));
    workbench.dispatch(project, Command::KeepChange("kept.ts".into()));
    workbench.settle().unwrap();

    assert_eq!(changes(&workbench, project), [("old/deleted.ts".into(), ChangeKind::Deleted)]);
    assert_eq!(fixture.read("edited.ts"), "after\n");
    assert_eq!(fixture.read("kept.ts"), "kept\n");
}

#[test]
fn keep_all_accepts_every_change() {
    let (fixture, mut workbench, project) = three_changes();

    workbench.dispatch(project, Command::KeepAllChanges);
    workbench.settle().unwrap();

    assert_eq!(changes(&workbench, project), []);
    assert_eq!(fixture.read("edited.ts"), "after\n");
    assert_eq!(fixture.read("created.ts"), "created\n");
    assert!(!fixture.path("old/deleted.ts").exists());
}

fn revert_all_prompt(workbench: &Workbench, project: ProjectId) -> Option<RevertAllPrompt> {
    workbench.project(project).unwrap().revert_all_prompt
}

#[test]
fn revert_all_asks_first_and_restores_every_file_once_confirmed() {
    let (fixture, mut workbench, project) = three_changes();

    workbench.dispatch(project, Command::RevertAllChanges);
    workbench.settle().unwrap();
    assert_eq!(revert_all_prompt(&workbench, project), Some(RevertAllPrompt { files: 3 }));
    assert_eq!(changes(&workbench, project).len(), 3, "nothing is reverted before the answer");
    assert_eq!(fixture.read("edited.ts"), "after\n");

    workbench.dispatch(project, Command::ConfirmRevertAll);
    workbench.settle().unwrap();

    assert_eq!(revert_all_prompt(&workbench, project), None);
    assert_eq!(changes(&workbench, project), []);
    assert_eq!(fixture.read("edited.ts"), "before\n");
    assert!(!fixture.path("created.ts").exists());
    assert_eq!(fixture.read("old/deleted.ts"), "deleted\n");
}

#[test]
fn cancelling_revert_all_changes_nothing() {
    let (fixture, mut workbench, project) = three_changes();

    workbench.dispatch(project, Command::RevertAllChanges);
    workbench.dispatch(project, Command::CancelRevertAll);
    workbench.settle().unwrap();

    assert_eq!(revert_all_prompt(&workbench, project), None);
    assert_eq!(changes(&workbench, project).len(), 3);
    assert_eq!(fixture.read("edited.ts"), "after\n");
    assert_eq!(fixture.read("created.ts"), "created\n");
}

#[test]
fn revert_all_reverts_only_the_files_it_asked_about() {
    let (fixture, mut workbench, project) = three_changes();

    workbench.dispatch(project, Command::RevertAllChanges);
    workbench.settle().unwrap();
    fixture.write("later.ts", "later\n");
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::ConfirmRevertAll);
    workbench.settle().unwrap();

    assert_eq!(changes(&workbench, project), [("later.ts".into(), ChangeKind::Created)]);
    assert_eq!(fixture.read("later.ts"), "later\n");
}

#[test]
fn reverting_one_change_doesnt_ask() {
    let (fixture, mut workbench, project) = three_changes();

    workbench.dispatch(project, Command::RevertChange("edited.ts".into()));
    workbench.settle().unwrap();

    assert_eq!(revert_all_prompt(&workbench, project), None);
    assert_eq!(fixture.read("edited.ts"), "before\n");
}

#[test]
fn revert_reloads_an_open_editor() {
    let (fixture, mut workbench, project) = open(&[("main.ts", "mine\n")]);
    workbench.dispatch(project, Command::OpenFile("main.ts".into()));
    workbench.settle().unwrap();
    fixture.write("main.ts", "theirs\n");
    workbench.settle().unwrap();
    assert_eq!(editor_text(&workbench, project), ["theirs", ""]);

    workbench.dispatch(project, Command::RevertChange("main.ts".into()));
    workbench.settle().unwrap();

    assert_eq!(editor_text(&workbench, project), ["mine", ""]);
    assert!(!workbench.project(project).unwrap().editor.unwrap().modified);
    workbench.dispatch(project, Command::Undo);
    assert_eq!(editor_text(&workbench, project), ["theirs", ""], "the revert's reload is one undo step");
}

#[test]
fn revert_under_unsaved_edits_shows_the_conflict_bar() {
    let (fixture, mut workbench, project) = open(&[("main.ts", "mine\n")]);
    workbench.dispatch(project, Command::OpenFile("main.ts".into()));
    workbench.settle().unwrap();
    fixture.write("main.ts", "theirs\n");
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::InsertText("typed ".into()));

    workbench.dispatch(project, Command::RevertChange("main.ts".into()));
    workbench.settle().unwrap();

    let editor = workbench.project(project).unwrap().editor.unwrap();
    assert!(editor.conflict);
    assert_eq!(editor_text(&workbench, project), ["typed theirs", ""]);
    assert_eq!(fixture.read("main.ts"), "mine\n");
}

#[test]
fn gitignored_files_node_modules_and_git_are_never_listed() {
    let (fixture, mut workbench, project) = open(&[
        (".gitignore", "dist/\n*.log\n!keep.log\n"),
        ("web/.gitignore", "generated.ts\n"),
        ("genea.jsonc", r#"{ "exclude": ["src/"] }"#),
        ("src/main.ts", "a\n"),
    ]);

    fixture.write("dist/bundle.js", "x\n");
    fixture.write("debug.log", "x\n");
    fixture.write("keep.log", "x\n");
    fixture.write("web/generated.ts", "x\n");
    fixture.write("web/app.ts", "x\n");
    fixture.write("generated.ts", "x\n");
    fixture.write("node_modules/pkg/index.js", "x\n");
    fixture.write("web/node_modules/pkg/index.js", "x\n");
    fixture.write(".git/HEAD", "ref: refs/heads/main\n");
    fixture.write("src/main.ts", "b\n");
    workbench.settle().unwrap();

    assert_eq!(
        changes(&workbench, project),
        [
            ("generated.ts".into(), ChangeKind::Created),
            ("keep.log".into(), ChangeKind::Created),
            ("src/main.ts".into(), ChangeKind::Modified),
            ("web/app.ts".into(), ChangeKind::Created),
        ],
        "the config's `exclude` doesn't affect review"
    );
}

#[test]
fn files_ignored_at_open_are_not_in_the_baseline() {
    let (fixture, mut workbench, project) = open(&[(".gitignore", "out/\n"), ("out/a.js", "a\n"), ("a.ts", "a\n")]);

    fixture.remove("out/a.js");
    fixture.write("out/b.js", "b\n");
    workbench.settle().unwrap();

    assert_eq!(changes(&workbench, project), []);
}

#[test]
fn a_changed_gitignore_changes_what_is_reviewed() {
    let (fixture, mut workbench, project) = open(&[(".gitignore", "tmp/\n"), ("a.ts", "a\n")]);
    fixture.write("b.ts", "b\n");
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::KeepAllChanges);
    workbench.settle().unwrap();

    fixture.write(".gitignore", "tmp/\nb.ts\n");
    fixture.write("tmp/scratch.ts", "x\n");
    workbench.settle().unwrap();
    assert_eq!(changes(&workbench, project), [(".gitignore".into(), ChangeKind::Modified)]);

    fixture.write("b.ts", "changed\n");
    workbench.settle().unwrap();
    assert_eq!(changes(&workbench, project), [(".gitignore".into(), ChangeKind::Modified)], "b.ts is ignored now");

    fixture.write(".gitignore", "");
    workbench.settle().unwrap();
    assert_eq!(
        changes(&workbench, project),
        [
            (".gitignore".into(), ChangeKind::Modified),
            ("b.ts".into(), ChangeKind::Created),
            ("tmp/scratch.ts".into(), ChangeKind::Created),
        ],
        "files that come into review have no baseline yet"
    );
}

#[test]
fn binary_and_large_files_are_listed_without_a_diff_and_revert_still_restores_them() {
    let large = "let a = 1;\n".repeat(LARGE_FILE_BYTES / 10);
    let (fixture, mut workbench, project) = open(&[("text.ts", "a\n"), ("big.ts", &large)]);
    let image = [0x89, b'P', b'N', b'G', 0, 0, 0, 1];
    fixture.write("image.png", image);
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::KeepAllChanges);
    workbench.settle().unwrap();

    fixture.write("image.png", [0x89, b'P', b'N', b'G', 0, 0, 0, 2]);
    fixture.write("big.ts", format!("{large}// more\n"));
    fixture.write("text.ts", "b\n");
    workbench.settle().unwrap();

    let item = |path: &str, diffable: bool| ChangeItem {
        path: path.into(),
        kind: ChangeKind::Modified,
        diffable,
        can_revert: true,
    };
    assert_eq!(items(&workbench, project), [item("big.ts", false), item("image.png", false), item("text.ts", true)]);

    workbench.dispatch(project, Command::RevertAllChanges);
    workbench.dispatch(project, Command::ConfirmRevertAll);
    workbench.settle().unwrap();
    assert_eq!(changes(&workbench, project), []);
    assert_eq!(std::fs::read(fixture.path("image.png")).unwrap(), image);
    assert_eq!(fixture.read("big.ts"), large);
}

#[test]
fn a_toolchain_pin_written_by_genea_is_not_listed() {
    let host = TestHost::new();
    host.tools().node("24.18.0");
    host.tools().node("26.11.1");
    host.tools().pnpm("12.10.1");
    let package_json = r#"{
  "name": "app",
  "devEngines": { "runtime": { "name": "node", "version": "24.18.0" } },
  "packageManager": "pnpm@12.10.1"
}
"#;
    let fixture = FixtureProject::new().file("package.json", package_json).build();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::OpenToolchainPicker(ToolchainPickerKind::Runtime));
    workbench.settle().unwrap();
    let picker = workbench.project(project).unwrap().toolchain_picker.unwrap();
    let option = picker.options.iter().find(|o| o.version == "26.11.1").unwrap();
    workbench.dispatch(project, option.command.clone());
    workbench.settle().unwrap();

    assert!(fixture.read("package.json").contains("26.11.1"));
    assert_eq!(changes(&workbench, project), []);
}

#[test]
fn the_baseline_is_kept_in_the_support_folder_not_the_project() {
    let fixture = FixtureProject::new().file("main.ts", "one\n").build();
    let host = TestHost::new();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    fixture.write("main.ts", "two\n");
    workbench.settle().unwrap();
    assert_eq!(changes(&workbench, project), [("main.ts".into(), ChangeKind::Modified)]);
    drop(workbench);

    // A restart: the first open's baseline is still "one".
    let mut workbench = Workbench::new(host.shared());
    let project_again = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    fixture.write("main.ts", "one\n");
    workbench.settle().unwrap();
    assert_eq!(changes(&workbench, project_again), [], "back to the baseline");
    fixture.write("main.ts", "three\n");
    workbench.settle().unwrap();
    assert_eq!(changes(&workbench, project_again), [("main.ts".into(), ChangeKind::Modified)]);

    let in_project: Vec<_> = std::fs::read_dir(fixture.root()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(in_project, ["main.ts"], "review writes nothing into the project");
}

#[test]
fn adding_typescript_7_from_genea_is_not_listed() {
    let (fixture, mut workbench, project) = open(&[("package.json", "{\n  \"name\": \"app\"\n}\n")]);

    workbench.dispatch(project, Command::AddTypeScript);
    workbench.settle().unwrap();

    assert!(fixture.read("package.json").contains("typescript"));
    assert_eq!(changes(&workbench, project), []);
}

#[test]
fn find_action_shows_the_changes_view() {
    let (_fixture, mut workbench, project) = open(&[("a.ts", "a\n")]);

    workbench.dispatch(project, Command::OpenFinder(FinderMode::Actions));
    workbench.dispatch(project, Command::SetFinderQuery("changes".into()));
    workbench.settle().unwrap();
    let finder = workbench.project(project).unwrap().finder.unwrap();
    assert_eq!(finder.items.first().map(|item| item.label.as_str()), Some("Changes"));
    workbench.dispatch(project, Command::AcceptFinder);
    workbench.settle().unwrap();

    assert_eq!(workbench.project(project).unwrap().left_column, Some(LeftColumnView::Changes));
}

fn editor_text(workbench: &Workbench, project: ProjectId) -> Vec<String> {
    workbench.project(project).unwrap().editor.unwrap().lines.into_iter().map(|l| l.text).collect()
}
