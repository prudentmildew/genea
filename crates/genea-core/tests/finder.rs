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

#[test]
fn file_results_follow_files_created_and_deleted_while_the_finder_is_open() {
    let (fixture, mut workbench, project) = open(FixtureProject::new().file("src/main.ts", ""));

    workbench.dispatch(project, Command::OpenFinder(FinderMode::Files));
    search(&mut workbench, project, "widget");
    assert!(results(&workbench, project).is_empty());

    fixture.write("src/widgets/Widget.tsx", "");
    workbench.settle().unwrap();
    assert_eq!(results(&workbench, project), ["Widget.tsx  src/widgets"]);

    fixture.write("src/widget.ts", "");
    workbench.settle().unwrap();
    assert_eq!(results(&workbench, project), ["widget.ts  src", "Widget.tsx  src/widgets"]);

    std::fs::remove_dir_all(fixture.path("src/widgets")).unwrap();
    workbench.settle().unwrap();
    assert_eq!(results(&workbench, project), ["widget.ts  src"]);
}

#[test]
fn the_file_finder_hides_what_the_config_excludes() {
    let (fixture, mut workbench, project) = open(
        FixtureProject::new().file("src/app.ts", "").file("dist/app.js", "").file("src/app.test.ts", ""),
    );

    fixture.write("genea.jsonc", r#"{ "exclude": ["dist/", "*.test.ts"] }"#);
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::OpenFinder(FinderMode::Files));
    search(&mut workbench, project, "app");
    assert_eq!(results(&workbench, project), ["app.ts  src"]);

    fixture.write("genea.jsonc", "{}");
    workbench.settle().unwrap();
    assert_eq!(results(&workbench, project), ["app.ts  src", "app.js  dist", "app.test.ts  src"]);
}

#[test]
fn choosing_a_file_opens_it_and_closes_the_finder() {
    let (_fixture, mut workbench, project) =
        open(FixtureProject::new().file("src/a.ts", "let a = 1;\n").file("src/b.ts", "let b = 2;\n"));

    workbench.dispatch(project, Command::OpenFinder(FinderMode::Files));
    search(&mut workbench, project, "ts");
    assert_eq!(results(&workbench, project), ["a.ts  src", "b.ts  src"]);

    workbench.dispatch(project, Command::MoveFinderSelection(1));
    assert_eq!(workbench.project(project).unwrap().finder.unwrap().selected, Some(1));
    workbench.dispatch(project, Command::AcceptFinder);
    workbench.settle().unwrap();

    let view = workbench.project(project).unwrap();
    assert_eq!(view.finder, None);
    assert_eq!(view.editor.unwrap().lines[0].text, "let b = 2;");
}

#[test]
fn the_selection_wraps_around_and_a_click_picks_a_result() {
    let (_fixture, mut workbench, project) =
        open(FixtureProject::new().file("a.ts", "").file("b.ts", "").file("c.ts", ""));
    let selected = |workbench: &Workbench| workbench.project(project).unwrap().finder.unwrap().selected;

    workbench.dispatch(project, Command::OpenFinder(FinderMode::Files));
    search(&mut workbench, project, "ts");
    workbench.dispatch(project, Command::MoveFinderSelection(-1));
    assert_eq!(selected(&workbench), Some(2));
    workbench.dispatch(project, Command::MoveFinderSelection(1));
    assert_eq!(selected(&workbench), Some(0));

    workbench.dispatch(project, Command::SelectFinderItem(1));
    workbench.dispatch(project, Command::AcceptFinder);
    workbench.settle().unwrap();
    assert_eq!(workbench.project(project).unwrap().editor.unwrap().title, "b.ts");
}

#[test]
fn escape_closes_the_finder_without_opening_anything() {
    let (_fixture, mut workbench, project) = open(FixtureProject::new().file("a.ts", ""));

    workbench.dispatch(project, Command::OpenFinder(FinderMode::Files));
    search(&mut workbench, project, "a");
    workbench.dispatch(project, Command::CloseFinder);
    workbench.settle().unwrap();

    let view = workbench.project(project).unwrap();
    assert_eq!(view.finder, None);
    assert_eq!(view.editor, None);
}
