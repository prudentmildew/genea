//! The Files view (⌘1): the project's file tree, kept up to date by the
//! watcher, with `node_modules` and the config's `exclude` hidden (ticket
//! #30).

use std::{fs, path::Path};

use genea_core::{Command, FileRow, FileRowKind, ProjectId, Workbench};
use genea_testkit::{FixtureBuilder, FixtureProject, TestHost};

fn open(fixture: FixtureBuilder) -> (FixtureProject, Workbench, ProjectId) {
    let fixture = fixture.build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    (fixture, workbench, project)
}

/// The tree as the user sees it: one line per row, indented two spaces per
/// level, folders with a trailing `/` and `+` (collapsed) or `-` (expanded).
fn tree(workbench: &Workbench, project: ProjectId) -> Vec<String> {
    let rows: Vec<FileRow> = workbench.project(project).unwrap().files.to_vec();
    rows.iter()
        .map(|row| {
            let indent = "  ".repeat(row.depth);
            match row.kind {
                FileRowKind::File => format!("{indent}{}", row.name),
                FileRowKind::Folder { expanded: false } => format!("{indent}+ {}/", row.name),
                FileRowKind::Folder { expanded: true } => format!("{indent}- {}/", row.name),
            }
        })
        .collect()
}

#[test]
fn the_tree_lists_the_project_root_folders_first_without_node_modules() {
    let (_fixture, workbench, project) = open(
        FixtureProject::new()
            .file("README.md", "# App\n")
            .file("package.json", "{}\n")
            .file("src/main.ts", "")
            .file("node_modules/left-pad/index.js", "")
            .dir("assets"),
    );

    assert_eq!(tree(&workbench, project), ["+ assets/", "+ src/", "package.json", "README.md"]);
}

#[test]
fn expanding_a_folder_shows_its_entries_beneath_it_and_collapsing_hides_them() {
    let (_fixture, mut workbench, project) = open(
        FixtureProject::new()
            .file("src/main.ts", "")
            .file("src/lib/util.ts", "")
            .file("src/node_modules/x/index.js", "")
            .file("tsconfig.json", "{}\n"),
    );

    workbench.dispatch(project, Command::ToggleFolder("src".into()));
    assert_eq!(tree(&workbench, project), ["- src/", "  + lib/", "  main.ts", "tsconfig.json"]);

    workbench.dispatch(project, Command::ToggleFolder("src/lib".into()));
    assert_eq!(tree(&workbench, project), ["- src/", "  - lib/", "    util.ts", "  main.ts", "tsconfig.json"]);

    workbench.dispatch(project, Command::ToggleFolder("src".into()));
    assert_eq!(tree(&workbench, project), ["+ src/", "tsconfig.json"]);

    // A folder remembers what was expanded inside it.
    workbench.dispatch(project, Command::ToggleFolder("src".into()));
    assert_eq!(tree(&workbench, project), ["- src/", "  - lib/", "    util.ts", "  main.ts", "tsconfig.json"]);
}

#[test]
fn files_created_deleted_or_renamed_on_disk_show_in_the_tree() {
    let (fixture, mut workbench, project) =
        open(FixtureProject::new().file("src/main.ts", "").file("src/old.ts", "").file("README.md", ""));
    workbench.dispatch(project, Command::ToggleFolder("src".into()));

    fixture.write("src/new.ts", "");
    fixture.remove("README.md");
    fs::rename(fixture.path("src/old.ts"), fixture.path("src/renamed.ts")).unwrap();
    workbench.settle().unwrap();

    assert_eq!(tree(&workbench, project), ["- src/", "  main.ts", "  new.ts", "  renamed.ts"]);
}

#[test]
fn a_folder_created_or_moved_in_on_disk_shows_with_its_contents() {
    let (fixture, mut workbench, project) = open(FixtureProject::new().file("src/main.ts", ""));
    let outside = FixtureProject::new().file("lib/a.ts", "").file("lib/deep/b.ts", "").build();

    fixture.write("test/unit/main.test.ts", "");
    fs::rename(outside.path("lib"), fixture.path("lib")).unwrap();
    workbench.settle().unwrap();
    for folder in ["lib", "lib/deep", "test", "test/unit"] {
        workbench.dispatch(project, Command::ToggleFolder(folder.into()));
    }

    assert_eq!(
        tree(&workbench, project),
        ["- lib/", "  - deep/", "    b.ts", "  a.ts", "+ src/", "- test/", "  - unit/", "    main.test.ts"]
    );
}

#[test]
fn a_folder_deleted_on_disk_leaves_the_tree() {
    let (fixture, mut workbench, project) =
        open(FixtureProject::new().file("src/main.ts", "").file("dist/out/index.js", ""));
    workbench.dispatch(project, Command::ToggleFolder("dist".into()));

    fs::remove_dir_all(fixture.path("dist")).unwrap();
    workbench.settle().unwrap();

    assert_eq!(tree(&workbench, project), ["+ src/"]);
}

#[test]
fn changes_inside_node_modules_stay_hidden() {
    let (fixture, mut workbench, project) = open(FixtureProject::new().file("src/main.ts", ""));

    fixture.write("node_modules/left-pad/index.js", "");
    fixture.write("src/node_modules/x/index.js", "");
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::ToggleFolder("src".into()));

    assert_eq!(tree(&workbench, project), ["- src/", "  main.ts"]);
}

#[test]
fn clicking_a_file_in_the_tree_opens_it() {
    let (_fixture, mut workbench, project) = open(FixtureProject::new().file("src/main.ts", "let a = 1;\n"));
    workbench.dispatch(project, Command::ToggleFolder("src".into()));
    let row = workbench.project(project).unwrap().files[1].clone();

    workbench.dispatch(project, Command::OpenFile(row.path));
    workbench.settle().unwrap();

    let editor = workbench.project(project).unwrap().editor.unwrap();
    assert_eq!(editor.path, Path::new("src/main.ts"));
    assert_eq!(editor.lines[0].text, "let a = 1;");
}
