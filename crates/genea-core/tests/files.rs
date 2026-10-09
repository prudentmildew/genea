//! The Files view (⌘1): the project's file tree, kept up to date by the
//! watcher, with `node_modules` and the config's `exclude` hidden (ticket
//! #30).

use genea_core::{FileRow, FileRowKind, ProjectId, Workbench};
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
