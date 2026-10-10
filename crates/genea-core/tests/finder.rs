//! The fuzzy finder (ticket #33): files (⌘⇧O), recent files (⌘E), actions
//! (⌘⇧A) and Search Everywhere (⇧⇧), matched off the main thread over the
//! live file index.

use genea_core::{Command, FinderItem, FinderItemKind, FinderMode, ProjectId, Workbench};
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

/// Opens files one after the other, as the user would.
fn open_files(workbench: &mut Workbench, project: ProjectId, paths: &[&str]) {
    for path in paths {
        workbench.dispatch(project, Command::OpenFile(path.into()));
        workbench.settle().unwrap();
    }
}

#[test]
fn recent_files_lists_files_in_the_order_they_were_last_opened() {
    let (_fixture, mut workbench, project) =
        open(FixtureProject::new().file("a.ts", "").file("src/b.ts", "").file("c.ts", "").file("d.ts", ""));

    open_files(&mut workbench, project, &["a.ts", "src/b.ts", "c.ts", "a.ts"]);
    workbench.dispatch(project, Command::OpenFinder(FinderMode::RecentFiles));
    workbench.settle().unwrap();

    assert_eq!(results(&workbench, project), ["a.ts", "c.ts", "b.ts  src"]);
    // The file before the current one is selected, so ⌘E Return goes back.
    assert_eq!(workbench.project(project).unwrap().finder.unwrap().selected, Some(1));
}

#[test]
fn selecting_a_tab_counts_as_opening_its_file() {
    let (_fixture, mut workbench, project) = open(FixtureProject::new().file("a.ts", "").file("b.ts", ""));

    open_files(&mut workbench, project, &["a.ts", "b.ts"]);
    workbench.dispatch(project, Command::SelectTab { pane: 0, tab: 0 });
    workbench.dispatch(project, Command::OpenFinder(FinderMode::RecentFiles));
    workbench.settle().unwrap();

    assert_eq!(results(&workbench, project), ["a.ts", "b.ts"]);
}

#[test]
fn a_query_narrows_recent_files_keeping_their_order() {
    let (_fixture, mut workbench, project) = open(
        FixtureProject::new().file("api/user.ts", "").file("web/users.tsx", "").file("web/app.tsx", ""),
    );

    open_files(&mut workbench, project, &["web/users.tsx", "web/app.tsx", "api/user.ts"]);
    workbench.dispatch(project, Command::OpenFinder(FinderMode::RecentFiles));
    search(&mut workbench, project, "user");

    assert_eq!(results(&workbench, project), ["user.ts  api", "users.tsx  web"]);
    assert_eq!(workbench.project(project).unwrap().finder.unwrap().selected, Some(0));
}

#[test]
fn a_deleted_file_leaves_recent_files() {
    let (fixture, mut workbench, project) = open(FixtureProject::new().file("a.ts", "").file("b.ts", ""));

    open_files(&mut workbench, project, &["a.ts", "b.ts"]);
    fixture.remove("a.ts");
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::OpenFinder(FinderMode::RecentFiles));
    workbench.settle().unwrap();

    assert_eq!(results(&workbench, project), ["b.ts"]);
}

#[test]
fn the_file_finder_starts_with_the_recent_files() {
    let (_fixture, mut workbench, project) =
        open(FixtureProject::new().file("a.ts", "").file("b.ts", "").file("c.ts", ""));

    open_files(&mut workbench, project, &["c.ts", "a.ts"]);
    workbench.dispatch(project, Command::OpenFinder(FinderMode::Files));
    workbench.settle().unwrap();

    assert_eq!(results(&workbench, project), ["a.ts", "c.ts"]);
    assert_eq!(workbench.project(project).unwrap().finder.unwrap().selected, Some(0));
}

#[test]
fn find_action_lists_every_action_with_its_shortcut() {
    let (_fixture, mut workbench, project) = open(FixtureProject::new());

    workbench.dispatch(project, Command::OpenFinder(FinderMode::Actions));
    workbench.settle().unwrap();

    let items = items(&workbench, project);
    let shortcut = |label: &str| {
        let item = items.iter().find(|item| item.label == label).unwrap_or_else(|| panic!("{label} is listed"));
        item.shortcut.clone()
    };
    assert_eq!(shortcut("Save"), Some("⌘S".into()));
    assert_eq!(shortcut("Redo"), Some("⇧⌘Z".into()));
    assert_eq!(shortcut("Go to File…"), Some("⇧⌘O".into()));
    assert_eq!(shortcut("Recent Files"), Some("⌘E".into()));
    assert_eq!(shortcut("Search Everywhere"), Some("⇧⇧".into()));
    assert_eq!(shortcut("Split Right"), None);
    assert_eq!(shortcut("Comment with Line Comment"), Some("⌘/".into()));
    assert_eq!(shortcut("Expand Selection"), Some("⌥↑".into()));
    assert_eq!(shortcut("Collapse Fold"), Some("⌥⌘-".into()));
    assert_eq!(shortcut("Set Runtime…"), None);
    assert_eq!(shortcut("Install Dependencies"), None);
    assert_eq!(shortcut("Search"), Some("⇧⌘F".into()));
    assert_eq!(shortcut("Terminal"), Some("⌥F12".into()));
    assert_eq!(shortcut("New Terminal Tab"), Some("⌘T".into()));
    assert_eq!(shortcut("Scripts"), None);
    assert!(items.iter().all(|item| matches!(item.kind, FinderItemKind::Action(_))));
}

#[test]
fn choosing_an_action_runs_its_command() {
    let (_fixture, mut workbench, project) = open(FixtureProject::new().file("a.ts", "let a = 1;\n"));
    open_files(&mut workbench, project, &["a.ts"]);

    workbench.dispatch(project, Command::OpenFinder(FinderMode::Actions));
    search(&mut workbench, project, "split right");
    assert_eq!(items(&workbench, project)[0].label, "Split Right");
    workbench.dispatch(project, Command::AcceptFinder);
    workbench.settle().unwrap();

    let view = workbench.project(project).unwrap();
    assert_eq!(view.finder, None);
    assert_eq!(view.panes.len(), 2);
}

#[test]
fn an_action_on_the_current_tab_acts_on_the_focused_one() {
    let (_fixture, mut workbench, project) =
        open(FixtureProject::new().file("a.ts", "").file("b.ts", "").file("c.ts", ""));
    open_files(&mut workbench, project, &["a.ts", "b.ts", "c.ts"]);
    workbench.dispatch(project, Command::SelectTab { pane: 0, tab: 1 });

    workbench.dispatch(project, Command::OpenFinder(FinderMode::Actions));
    search(&mut workbench, project, "close tab");
    workbench.dispatch(project, Command::AcceptFinder);
    workbench.settle().unwrap();

    let tabs: Vec<String> = workbench.project(project).unwrap().panes[0].tabs.iter().map(|t| t.title.clone()).collect();
    assert_eq!(tabs, ["a.ts", "c.ts"]);
}

#[test]
fn choosing_a_finder_action_switches_the_finder_to_it() {
    let (_fixture, mut workbench, project) = open(FixtureProject::new().file("a.ts", ""));
    open_files(&mut workbench, project, &["a.ts"]);

    workbench.dispatch(project, Command::OpenFinder(FinderMode::Actions));
    search(&mut workbench, project, "recent files");
    workbench.dispatch(project, Command::AcceptFinder);
    workbench.settle().unwrap();

    let finder = workbench.project(project).unwrap().finder.unwrap();
    assert_eq!((finder.mode, finder.query.as_str()), (FinderMode::RecentFiles, ""));
    assert_eq!(results(&workbench, project), ["a.ts"]);
}

#[test]
fn search_everywhere_mixes_files_and_actions() {
    let (_fixture, mut workbench, project) =
        open(FixtureProject::new().file("src/split/pane.ts", "").file("src/main.ts", ""));

    workbench.dispatch(project, Command::OpenFinder(FinderMode::Everywhere));
    search(&mut workbench, project, "split");

    let labels: Vec<String> = items(&workbench, project).iter().map(|item| item.label.clone()).collect();
    for expected in ["pane.ts", "Split Right", "Close Split"] {
        assert!(labels.contains(&expected.to_owned()), "{expected} in {labels:?}");
    }
    assert!(!labels.contains(&"main.ts".to_owned()));
}

#[test]
fn search_everywhere_starts_with_the_recent_files() {
    let (_fixture, mut workbench, project) = open(FixtureProject::new().file("a.ts", "").file("b.ts", ""));
    open_files(&mut workbench, project, &["b.ts"]);

    workbench.dispatch(project, Command::OpenFinder(FinderMode::Everywhere));
    workbench.settle().unwrap();

    assert_eq!(results(&workbench, project), ["b.ts"]);
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

fn terminal_focused(workbench: &Workbench, project: ProjectId) -> bool {
    workbench.project(project).unwrap().terminal.focused
}

#[test]
fn a_file_chosen_while_the_terminal_has_the_focus_takes_it() {
    let (_fixture, mut workbench, project) = open(FixtureProject::new().file("a.ts", "let a = 1;\n"));
    workbench.dispatch(project, Command::ToggleTerminal);
    assert!(terminal_focused(&workbench, project));

    workbench.dispatch(project, Command::OpenFinder(FinderMode::Files));
    search(&mut workbench, project, "a.ts");
    workbench.dispatch(project, Command::AcceptFinder);
    workbench.settle().unwrap();

    assert!(!terminal_focused(&workbench, project));
    assert_eq!(workbench.project(project).unwrap().editor.unwrap().title, "a.ts");
}

#[test]
fn the_terminal_actions_show_it_and_the_search_view() {
    let (_fixture, mut workbench, project) = open(FixtureProject::new());
    let run = |workbench: &mut Workbench, name: &str| {
        workbench.dispatch(project, Command::OpenFinder(FinderMode::Actions));
        search(workbench, project, name);
        let index = items(workbench, project).iter().position(|item| item.label == name).unwrap();
        workbench.dispatch(project, Command::SelectFinderItem(index));
        workbench.dispatch(project, Command::AcceptFinder);
        workbench.settle().unwrap();
    };

    run(&mut workbench, "Terminal");
    assert!(terminal_focused(&workbench, project));
    run(&mut workbench, "Search");
    assert_eq!(workbench.project(project).unwrap().left_column, Some(genea_core::LeftColumnView::Search));
}

#[test]
fn an_editing_action_does_nothing_while_the_terminal_has_the_focus() {
    let (_fixture, mut workbench, project) = open(FixtureProject::new().file("a.ts", "let a = 1;\n"));
    open_files(&mut workbench, project, &["a.ts"]);
    let comment = |workbench: &mut Workbench| {
        workbench.dispatch(project, Command::OpenFinder(FinderMode::Actions));
        search(workbench, project, "comment with line comment");
        workbench.dispatch(project, Command::AcceptFinder);
        workbench.settle().unwrap();
    };
    let first_line = |workbench: &Workbench| workbench.project(project).unwrap().editor.unwrap().lines[0].text.clone();

    workbench.dispatch(project, Command::FocusTerminal);
    comment(&mut workbench);
    assert_eq!(first_line(&workbench), "let a = 1;");
    assert!(terminal_focused(&workbench, project));

    workbench.dispatch(project, Command::FocusPane(0));
    comment(&mut workbench);
    assert_eq!(first_line(&workbench), "// let a = 1;");
}
