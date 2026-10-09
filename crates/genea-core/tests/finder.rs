//! The fuzzy finder (ticket #33): files (⌘⇧O), recent files (⌘E), actions
//! (⌘⇧A) and Search Everywhere (⇧⇧), matched off the main thread over the
//! live file index.

use genea_core::{Command, FinderItem, FinderMode, ProjectId, Workbench};
use genea_testkit::{FixtureBuilder, FixtureProject, TestHost};

fn open(fixture: FixtureBuilder) -> (FixtureProject, Workbench, ProjectId) {
    let fixture = fixture.build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    (fixture, workbench, project)
}

/// Types a query into the open finder and waits for its results.
fn search(workbench: &mut Workbench, project: ProjectId, query: &str) {
    workbench.dispatch(project, Command::SetFinderQuery(query.into()));
    workbench.settle().unwrap();
}

fn items(workbench: &Workbench, project: ProjectId) -> Vec<FinderItem> {
    workbench.project(project).unwrap().finder.expect("the finder is open").items
}

/// The results as the user reads them: `label  detail`.
fn results(workbench: &Workbench, project: ProjectId) -> Vec<String> {
    items(workbench, project)
        .iter()
        .map(|item| if item.detail.is_empty() { item.label.clone() } else { format!("{}  {}", item.label, item.detail) })
        .collect()
}

#[test]
fn the_file_finder_lists_the_files_matching_the_query_without_node_modules() {
    let (_fixture, mut workbench, project) = open(
        FixtureProject::new()
            .file("src/button.ts", "")
            .file("src/components/IconButton.tsx", "")
            .file("src/main.ts", "")
            .file("README.md", "")
            .file("node_modules/button/index.js", ""),
    );

    workbench.dispatch(project, Command::OpenFinder(FinderMode::Files));
    search(&mut workbench, project, "button");

    let finder = workbench.project(project).unwrap().finder.unwrap();
    assert_eq!(finder.mode, FinderMode::Files);
    assert_eq!(finder.query, "button");
    assert_eq!(results(&workbench, project), ["button.ts  src", "IconButton.tsx  src/components"]);
    assert_eq!(finder.selected, Some(0));
}
