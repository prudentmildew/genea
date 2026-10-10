//! Foreign tools (ticket #51, ADR 0001): a tool a project uses for a role
//! Genea doesn't bless turns off that role's features, and Genea never runs
//! it. npm or Yarn (from a root lockfile or `packageManager`) turns off
//! installs and the script runner; ESLint, Prettier, Biome or dprint config
//! at the root or a package root turns off format and fix on save. Either
//! way one warning in Problems and one status-bar item say so, and
//! everything else (editing, language intelligence, the terminal) works.

use genea_core::{Command, ProblemItem, ProblemSource, ProjectId, ProjectView, Severity, TextPosition, Workbench};
use genea_testkit::{FixtureBuilder, FixtureProject, TestHost};

const UNPINNED: &str = "{\n  \"name\": \"app\",\n  \"scripts\": { \"dev\": \"vite\" }\n}\n";

fn project(package_json: &str) -> FixtureBuilder {
    FixtureProject::new().file("package.json", package_json)
}

/// Opens `fixture` on a host with Genea's default tools published.
fn open(host: &TestHost, fixture: &FixtureProject) -> (Workbench, ProjectId) {
    host.tools().node("24.21.0");
    host.tools().pnpm("12.10.1");
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    (workbench, project)
}

fn view(workbench: &Workbench, project: ProjectId) -> ProjectView {
    workbench.project(project).unwrap()
}

fn foreign_problems(workbench: &Workbench, project: ProjectId) -> Vec<ProblemItem> {
    view(workbench, project).problems.into_iter().filter(|p| p.source == ProblemSource::ForeignTools).collect()
}

fn warning(path: &str, position: TextPosition, message: &str) -> ProblemItem {
    ProblemItem {
        source: ProblemSource::ForeignTools,
        severity: Severity::Warning,
        path: path.into(),
        position,
        location: format!("{}:{}", position.line + 1, position.column + 1),
        message: message.into(),
        stale: false,
    }
}

const NPM_OFF: &str =
    "package-lock.json is an npm lockfile. Genea doesn't run npm, so installing dependencies and running scripts are off.";

#[test]
fn an_npm_lockfile_shows_one_warning_in_problems_and_the_status_bar() {
    let host = TestHost::new();
    let fixture = project(UNPINNED).file("package-lock.json", "{}\n").build();

    let (workbench, project) = open(&host, &fixture);

    assert_eq!(foreign_problems(&workbench, project), [warning("package-lock.json", TextPosition::default(), NPM_OFF)]);
    assert_eq!(view(&workbench, project).status.foreign_tools.as_deref(), Some("Reduced mode: npm"));
}

const YARN: &str =
    "{\n  \"name\": \"app\",\n  \"packageManager\": \"yarn@4.5.0\",\n  \"scripts\": { \"dev\": \"vite\" }\n}\n";

const YARN_OFF: &str =
    "packageManager pins Yarn. Genea doesn't run Yarn, so installing dependencies and running scripts are off.";

#[test]
fn a_yarn_package_manager_pin_shows_the_warning_at_the_pin() {
    let host = TestHost::new();
    let fixture = project(YARN).build();

    let (workbench, project) = open(&host, &fixture);

    assert_eq!(
        foreign_problems(&workbench, project),
        [warning("package.json", TextPosition { line: 2, column: 2 }, YARN_OFF)]
    );
    let view = view(&workbench, project);
    assert_eq!(view.status.foreign_tools.as_deref(), Some("Reduced mode: Yarn"));
    assert_eq!(view.toolchain.package_manager.unwrap().tool, "Yarn");
}

#[test]
fn an_npm_lockfile_appearing_or_going_turns_reduced_mode_on_and_off() {
    let host = TestHost::new();
    let fixture = project(UNPINNED).build();
    let (mut workbench, project) = open(&host, &fixture);
    assert_eq!(view(&workbench, project).toolchain.package_manager.unwrap().tool, "pnpm");

    fixture.write("package-lock.json", "{}\n");
    workbench.settle().unwrap();

    assert_eq!(foreign_problems(&workbench, project), [warning("package-lock.json", TextPosition::default(), NPM_OFF)]);
    assert_eq!(view(&workbench, project).toolchain.package_manager.unwrap().tool, "npm");
    assert_eq!(view(&workbench, project).scripts, []);

    fixture.remove("package-lock.json");
    workbench.settle().unwrap();

    assert_eq!(foreign_problems(&workbench, project), []);
    let view = view(&workbench, project);
    assert_eq!(view.status.foreign_tools, None);
    assert_eq!(view.toolchain.package_manager.unwrap().tool, "pnpm");
    assert_eq!(view.scripts_off, None);
    assert_eq!(view.scripts[0].scripts[0].name, "dev");
}

#[test]
fn a_pinned_package_manager_decides_over_an_npm_or_yarn_lockfile_which_goes_stale() {
    let pnpm = "{\n  \"name\": \"app\",\n  \"packageManager\": \"pnpm@12.10.1\"\n}\n";
    for (lockfile, kind) in [("package-lock.json", "an npm"), ("yarn.lock", "a Yarn")] {
        let host = TestHost::new();
        let fixture = project(pnpm).file(lockfile, "").build();

        let (workbench, project) = open(&host, &fixture);

        assert_eq!(foreign_problems(&workbench, project), []);
        let view = view(&workbench, project);
        assert_eq!(view.status.foreign_tools, None);
        assert_eq!(view.toolchain.package_manager.unwrap().tool, "pnpm");
        let lockfile_warnings: Vec<String> = view
            .problems
            .into_iter()
            .filter(|p| p.source == ProblemSource::Toolchain)
            .map(|p| p.message)
            .collect();
        assert_eq!(
            lockfile_warnings,
            [format!("packageManager pins pnpm, but {lockfile} is {kind} lockfile. Genea uses pnpm, so {lockfile} goes stale.")]
        );
    }
}

#[test]
fn npm_and_yarn_workspaces_package_roots_count_for_formatter_config() {
    let workspaces = "{\n  \"name\": \"app\",\n  \"workspaces\": [\"packages/*\"]\n}\n";
    for lockfile in ["package-lock.json", "yarn.lock"] {
        let host = TestHost::new();
        let fixture = project(workspaces)
            .file(lockfile, "")
            .file("packages/ui/package.json", r#"{ "name": "ui" }"#)
            .file("packages/ui/.eslintrc.json", "{}\n")
            .build();

        let (workbench, project) = open(&host, &fixture);

        let problems = foreign_problems(&workbench, project);
        assert_eq!(problems.len(), 2, "{lockfile}: {problems:?}");
        let eslint = std::path::Path::new("packages/ui/.eslintrc.json");
        assert!(problems.iter().any(|p| p.path == eslint), "{lockfile}: {problems:?}");
        let status = view(&workbench, project).status.foreign_tools.unwrap();
        assert!(status.ends_with(", ESLint"), "{lockfile}: {status}");
    }
}

/// npm by its lockfile, and Yarn by `packageManager`.
fn npm_and_yarn() -> [(&'static str, FixtureProject); 2] {
    [
        ("npm", project(UNPINNED).file("package-lock.json", "{}\n").build()),
        ("Yarn", project(YARN).file("yarn.lock", "").build()),
    ]
}

#[test]
fn npm_and_yarn_projects_offer_no_install_and_genea_never_runs_them() {
    for (tool, fixture) in npm_and_yarn() {
        let host = TestHost::new();
        let (mut workbench, project) = open(&host, &fixture);
        let notices: Vec<String> = view(&workbench, project).notices.into_iter().map(|n| n.message).collect();
        assert!(
            !notices.iter().any(|n| n.contains("dependencies aren't installed")),
            "{tool}: no install is offered: {notices:?}"
        );

        workbench.dispatch(project, Command::InstallDependencies);
        workbench.settle().unwrap();

        let view = view(&workbench, project);
        assert!(view.terminal.tabs.iter().all(|tab| tab.shell), "{tool}: no install tab: {:?}", view.terminal.tabs);
        let why = format!("Genea doesn't run {tool}, so it can't install this project's dependencies.");
        assert!(view.notices.iter().any(|n| n.message == why), "{tool}: {:?}", view.notices);
        let ran: Vec<_> = host.ptys().spawned().iter().map(|pty| pty.spec()).collect();
        assert!(ran.iter().all(|spec| spec.args == ["-l"]), "{tool}: only the shell ran: {ran:?}");
    }
}

#[test]
fn npm_and_yarn_projects_have_no_script_runner() {
    for (tool, fixture) in npm_and_yarn() {
        let host = TestHost::new();
        let (mut workbench, project) = open(&host, &fixture);
        let before = view(&workbench, project);
        assert_eq!(before.scripts, [], "{tool}: no scripts are listed");
        assert_eq!(
            before.scripts_off.as_deref(),
            Some(format!("Genea doesn't run {tool}, so it doesn't run this project's scripts.").as_str())
        );

        workbench.dispatch(project, Command::RunScript { package: "".into(), script: "dev".into() });
        workbench.settle().unwrap();

        let view = view(&workbench, project);
        assert!(view.terminal.tabs.iter().all(|tab| tab.shell), "{tool}: no script tab: {:?}", view.terminal.tabs);
        let ran: Vec<_> = host.ptys().spawned().iter().map(|pty| pty.spec()).collect();
        assert!(ran.iter().all(|spec| spec.args == ["-l"]), "{tool}: only the shell ran: {ran:?}");
    }
}
