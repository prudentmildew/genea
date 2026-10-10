//! Session restore (ticket #59): what a project's window showed comes back
//! after a restart, and the projects that were open when Genea quit reopen.
//!
//! A restart is a second workbench on the same test host (they share the
//! application-support folder). `Workbench::quit` is what the app calls as
//! it quits: it writes the session at once.

use std::time::Duration;

use genea_core::{
    CaretMove, Command, LeftColumnView, MAX_FONT_SIZE, MIN_FONT_SIZE, ProjectId, ProjectView, TerminalStatus,
    TerminalTab, WindowFrame, WindowLayout, Workbench,
};
use genea_testkit::{FixtureProject, TestHost};

/// Session state is written in the background a short pause after a
/// change. Advancing past it and settling waits for the write.
const SAVED: Duration = Duration::from_secs(5);

fn view(workbench: &Workbench, project: ProjectId) -> ProjectView {
    workbench.project(project).unwrap()
}

/// Each side's tab titles, with the active one in brackets.
fn tabs(workbench: &Workbench, project: ProjectId) -> Vec<Vec<String>> {
    view(workbench, project)
        .panes
        .iter()
        .map(|pane| {
            pane.tabs
                .iter()
                .enumerate()
                .map(|(i, tab)| if pane.active == Some(i) { format!("[{}]", tab.title) } else { tab.title.clone() })
                .collect()
        })
        .collect()
}

fn run(workbench: &mut Workbench, project: ProjectId, commands: impl IntoIterator<Item = Command>) {
    for command in commands {
        workbench.dispatch(project, command);
        workbench.settle().unwrap();
    }
}

fn open(path: &str) -> Command {
    Command::OpenFile(path.into())
}

/// Quits `workbench` and starts Genea again on the same host, restoring
/// the session. Returns the new workbench and the projects it reopened.
fn restart(mut workbench: Workbench, host: &TestHost) -> (Workbench, Vec<ProjectId>) {
    workbench.quit();
    drop(workbench);
    let mut restarted = Workbench::new(host.shared());
    let projects = restarted.restore_session();
    restarted.settle().unwrap();
    (restarted, projects)
}

#[test]
fn quitting_and_relaunching_reopens_the_project_with_its_tabs() {
    let fixture = FixtureProject::new().file("a.ts", "let a = 1;\n").file("b.ts", "let b = 2;\n").build();
    let host = TestHost::new();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    run(&mut workbench, project, [open("a.ts"), open("b.ts")]);

    let (restarted, projects) = restart(workbench, &host);

    let [project] = projects[..] else { panic!("one project reopens, not {projects:?}") };
    assert_eq!(view(&restarted, project).root, fixture.root().canonicalize().unwrap());
    assert_eq!(tabs(&restarted, project), [["a.ts", "[b.ts]"]]);
    assert_eq!(view(&restarted, project).editor.unwrap().lines[0].text, "let b = 2;");
}

/// `count` numbered lines.
fn numbered(count: usize) -> String {
    (1..=count).map(|n| format!("line {n}\n")).collect()
}

#[test]
fn the_split_carets_selections_and_scroll_positions_come_back() {
    let fixture = FixtureProject::new().file("a.ts", &numbered(100)).file("b.ts", &numbered(100)).build();
    let host = TestHost::new();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    run(
        &mut workbench,
        project,
        [
            Command::SetViewport { rows: 10.0 },
            open("a.ts"),
            Command::PlaceCaret { line: 40, column: 2 },
            Command::Select(CaretMove::Right),
            Command::AddCaret { line: 42, column: 0 },
            Command::SplitRight,
            open("b.ts"),
            Command::ScrollPane { pane: 1, rows: 20.0 },
            Command::FocusPane(0),
        ],
    );
    let before = view(&workbench, project);
    assert_eq!(tabs(&workbench, project), [vec!["[a.ts]"], vec!["a.ts", "[b.ts]"]]);

    let (mut restarted, projects) = restart(workbench, &host);
    let project = projects[0];
    run(&mut restarted, project, [Command::SetViewport { rows: 10.0 }]);

    let after = view(&restarted, project);
    assert_eq!(tabs(&restarted, project), [vec!["[a.ts]"], vec!["a.ts", "[b.ts]"]]);
    assert_eq!(after.focused_pane, 0);
    let editor = after.editor.clone().unwrap();
    assert_eq!((editor.caret.line, editor.caret.column), (42, 0));
    assert_eq!(editor.carets.len(), 2);
    assert_eq!(after.panes[1].editor.as_ref().unwrap().scroll_top, 20.0);
    assert_eq!(after.panes, before.panes);
}

fn terminal_tab(title: &str, status: TerminalStatus) -> TerminalTab {
    TerminalTab { title: title.into(), status, shell: title == "zsh" }
}

#[test]
fn shell_tabs_come_back_fresh_in_their_places_and_script_tabs_dont_restart() {
    let fixture = FixtureProject::new()
        .file("package.json", r#"{ "name": "shop", "packageManager": "pnpm@12.10.1" }"#)
        .file("pnpm-workspace.yaml", "packages:\n  - apps/*\n")
        .file("apps/web/package.json", r#"{ "name": "@shop/web", "scripts": { "dev": "vite" } }"#)
        .build();
    let host = TestHost::new();
    host.tools().node("24.21.0");
    host.tools().pnpm("12.10.1");
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    run(
        &mut workbench,
        project,
        [
            Command::RunScript { package: "apps/web".into(), script: "dev".into() },
            Command::NewTerminalTab,
            Command::SelectTerminalTab(1),
        ],
    );
    host.ptys().wait_for_spawn(3);

    let (mut restarted, projects) = restart(workbench, &host);
    let project = projects[0];

    // Two fresh shells; the script didn't run again.
    host.ptys().wait_for_spawn(5);
    restarted.settle().unwrap();
    let after_restart: Vec<_> = host.ptys().spawned()[3..].iter().map(|pty| pty.spec().args).collect();
    assert_eq!(after_restart.len(), 2);
    assert!(after_restart.iter().all(|args| !args.iter().any(|arg| arg == "dev")),"{after_restart:?}");
    let view = view(&restarted, project).terminal;
    assert_eq!(
        view.tabs,
        [
            terminal_tab("zsh", TerminalStatus::Running),
            terminal_tab("@shop/web: dev", TerminalStatus::Stopped),
            terminal_tab("zsh", TerminalStatus::Running),
        ]
    );
    assert_eq!(view.active_tab, 1);
    assert!(view.visible);

    // Re-run starts it in its package's folder, as before.
    run(&mut restarted, project, [Command::RerunTerminalTab(1)]);
    let dev = host.ptys().wait_for_spawn(6).spec();
    assert_eq!(dev.args, ["run", "dev"]);
    assert_eq!(dev.cwd.as_deref(), Some(fixture.root().canonicalize().unwrap().join("apps/web").as_path()));
}

#[test]
fn zoom_steps_the_editor_and_terminal_font_a_point_at_a_time_within_limits() {
    let fixture = FixtureProject::new().build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    assert_eq!(view(&workbench, project).font_size, 13.0);

    run(&mut workbench, project, [Command::ZoomIn, Command::ZoomIn]);
    assert_eq!(view(&workbench, project).font_size, 15.0);
    run(&mut workbench, project, [Command::ZoomOut]);
    assert_eq!(view(&workbench, project).font_size, 14.0);
    run(&mut workbench, project, [Command::ResetZoom]);
    assert_eq!(view(&workbench, project).font_size, 13.0);

    run(&mut workbench, project, (0..40).map(|_| Command::ZoomIn));
    assert_eq!(view(&workbench, project).font_size, MAX_FONT_SIZE);
    run(&mut workbench, project, (0..40).map(|_| Command::ZoomOut));
    assert_eq!(view(&workbench, project).font_size, MIN_FONT_SIZE);
}

#[test]
fn zoom_is_kept_per_project() {
    let zoomed = FixtureProject::new().build();
    let plain = FixtureProject::new().build();
    let host = TestHost::new();
    let mut workbench = Workbench::new(host.shared());
    let a = workbench.open_project(zoomed.root()).unwrap();
    let b = workbench.open_project(plain.root()).unwrap();
    run(&mut workbench, a, [Command::ZoomIn, Command::ZoomIn, Command::ZoomIn]);
    run(&mut workbench, b, [Command::ZoomOut]);
    assert_eq!(view(&workbench, a).font_size, 16.0);

    let (restarted, projects) = restart(workbench, &host);

    let [a, b] = projects[..] else { panic!("two projects reopen, not {projects:?}") };
    assert_eq!(view(&restarted, a).font_size, 16.0);
    assert_eq!(view(&restarted, b).font_size, 12.0);
}

#[test]
fn the_left_column_and_the_window_layout_come_back() {
    let fixture = FixtureProject::new().build();
    let host = TestHost::new();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    assert_eq!(view(&workbench, project).window_layout, None);
    let layout = WindowLayout {
        frame: WindowFrame { x: 40.0, y: 60.0, width: 1300.0, height: 850.5 },
        left_column_width: 260.0,
        terminal_width: 480.0,
        terminal_height: 300.0,
    };
    run(
        &mut workbench,
        project,
        [Command::ToggleLeftColumn(LeftColumnView::Problems), Command::SetWindowLayout(layout)],
    );

    let (restarted, projects) = restart(workbench, &host);

    let restored = view(&restarted, projects[0]);
    assert_eq!(restored.left_column, Some(LeftColumnView::Problems));
    assert_eq!(restored.window_layout, Some(layout));
}

#[test]
fn a_collapsed_left_column_and_a_hidden_terminal_stay_so() {
    let fixture = FixtureProject::new().build();
    let host = TestHost::new();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    run(
        &mut workbench,
        project,
        [Command::ToggleLeftColumn(LeftColumnView::Files), Command::FocusTerminal, Command::ToggleTerminal],
    );
    assert!(!view(&workbench, project).terminal.visible);

    let (restarted, projects) = restart(workbench, &host);

    let restored = view(&restarted, projects[0]);
    assert_eq!(restored.left_column, None);
    assert!(!restored.terminal.visible);
}

#[test]
fn a_file_gone_since_leaves_its_tab_out_and_a_shorter_one_clamps_its_caret() {
    let fixture = FixtureProject::new().file("gone.ts", "x\n").file("short.ts", &numbered(50)).build();
    let host = TestHost::new();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    run(&mut workbench, project, [open("short.ts"), Command::PlaceCaret { line: 40, column: 3 }, open("gone.ts")]);
    workbench.quit();
    drop(workbench);
    fixture.remove("gone.ts");
    fixture.write("short.ts", "only line\n");

    let mut restarted = Workbench::new(host.shared());
    let project = restarted.restore_session()[0];
    restarted.settle().unwrap();

    assert_eq!(tabs(&restarted, project), [["[short.ts]"]]);
    let caret = view(&restarted, project).editor.unwrap().caret;
    assert_eq!((caret.line, caret.column), (1, 0));
}

#[test]
fn a_file_opened_while_the_session_is_restored_keeps_the_focus() {
    let fixture = FixtureProject::new().file("a.ts", "a\n").file("b.ts", "b\n").file("c.ts", "c\n").build();
    let host = TestHost::new();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    run(&mut workbench, project, [open("a.ts"), open("b.ts")]);
    workbench.quit();
    drop(workbench);

    // `genea FOLDER FILE`: the file is opened right after the folder.
    let mut restarted = Workbench::new(host.shared());
    let project = restarted.open_project(fixture.root()).unwrap();
    restarted.dispatch(project, open("c.ts"));
    restarted.settle().unwrap();

    assert_eq!(tabs(&restarted, project), [["a.ts", "b.ts", "[c.ts]"]]);
}

#[test]
fn the_session_is_saved_in_the_background_so_it_survives_a_crash() {
    let fixture = FixtureProject::new().file("a.ts", "a\n").file("b.ts", "b\n").build();
    let host = TestHost::new();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    run(&mut workbench, project, [open("a.ts"), open("b.ts")]);
    host.clock().advance(SAVED);
    workbench.settle().unwrap();
    // No quit: Genea stopped.
    drop(workbench);

    let mut restarted = Workbench::new(host.shared());
    let projects = restarted.restore_session();
    restarted.settle().unwrap();

    assert_eq!(projects.len(), 1);
    assert_eq!(tabs(&restarted, projects[0]), [["a.ts", "[b.ts]"]]);
}

#[test]
fn a_closed_project_isnt_reopened_but_opens_again_as_it_was() {
    let fixture = FixtureProject::new().file("a.ts", "a\n").build();
    let other = FixtureProject::new().build();
    let host = TestHost::new();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.open_project(other.root()).unwrap();
    run(&mut workbench, project, [open("a.ts")]);
    workbench.close_project(project);
    workbench.settle().unwrap();

    let (mut restarted, projects) = restart(workbench, &host);
    assert_eq!(projects.len(), 1);
    assert_eq!(view(&restarted, projects[0]).root, other.root().canonicalize().unwrap());

    let reopened = restarted.open_project(fixture.root()).unwrap();
    restarted.settle().unwrap();
    assert_eq!(tabs(&restarted, reopened), [["[a.ts]"]]);
}

#[test]
fn quitting_with_no_project_open_starts_with_the_welcome() {
    let fixture = FixtureProject::new().build();
    let host = TestHost::new();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.close_project(project);

    let (restarted, projects) = restart(workbench, &host);

    assert_eq!(projects, []);
    assert!(restarted.welcome().is_some());
}

#[test]
fn quitting_saves_the_recent_projects_at_once() {
    let fixture = FixtureProject::new().build();
    let host = TestHost::new();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.close_project(project);
    // Quit well within the recent list's save delay.

    let (restarted, _) = restart(workbench, &host);

    let welcome = restarted.welcome().unwrap();
    assert_eq!(welcome.recent_projects[0].root, fixture.root().canonicalize().unwrap());
}
