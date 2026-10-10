//! Oxlint diagnostics (ticket #49): `oxlint --lsp` from the project's
//! `node_modules`, run on the pinned runtime, against the fake LSP server
//! (`genea_testkit::FakeLsp`), which the test host plays whenever Genea
//! starts the runtime (`node`, or `bun`).

use std::path::{Path, PathBuf};

use genea_core::{
    Command, InlineProblem, LanguageServerState, LanguageServerStatus, MAX_RESTARTS, Notice, ProblemSource,
    ProcessSpec, ProjectId, RESTART_DELAY, Severity, Workbench,
};
use genea_testkit::{FakeLsp, FixtureBuilder, FixtureProject, TestHost};
use serde_json::{Value, json};

/// The Oxlint launcher in `node_modules` (a Node script).
const OXLINT: &str = "node_modules/oxlint/bin/oxlint";

const NO_DEBUGGER: &str = "`debugger` statement is not allowed";

/// A project pinning Node and pnpm, with Oxlint and Oxfmt installed.
fn oxc_project() -> FixtureBuilder {
    project_with(
        r#"{
  "name": "app",
  "packageManager": "pnpm@12.10.1",
  "devEngines": { "runtime": { "name": "node", "version": "24.21.0" } },
  "devDependencies": { "oxfmt": "^0.72.0", "oxlint": "^1.87.0" }
}
"#,
    )
}

/// A project with this `package.json` and Oxlint installed.
fn project_with(package_json: &str) -> FixtureBuilder {
    FixtureProject::new()
        .file("package.json", package_json)
        .file("node_modules/oxlint/package.json", r#"{ "name": "oxlint", "version": "1.87.0" }"#)
        .file(OXLINT, "#!/usr/bin/env node\n")
}

struct Session {
    fixture: FixtureProject,
    host: TestHost,
    workbench: Workbench,
    project: ProjectId,
}

/// Opens `fixture` on a host with the pinned tools published, with
/// `runtime` (`node` or `bun`) played by `fake`.
fn open(fixture: FixtureProject, fake: &FakeLsp, runtime: &str) -> Session {
    let host = TestHost::new();
    host.tools().node("24.21.0");
    host.tools().bun("1.4.2");
    host.tools().pnpm("12.10.1");
    fake.install(&host, runtime);
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    Session { fixture, host, workbench, project }
}

impl Session {
    fn dispatch(&mut self, command: Command) {
        self.workbench.dispatch(self.project, command);
    }

    fn settle(&mut self) {
        self.workbench.settle().unwrap();
    }

    fn view(&self) -> genea_core::ProjectView {
        self.workbench.project(self.project).unwrap()
    }

    /// Oxlint's problems: (path, location, severity, message).
    fn oxlint_problems(&self) -> Vec<(PathBuf, String, Severity, String)> {
        self.view()
            .problems
            .into_iter()
            .filter(|p| p.source == ProblemSource::Oxlint)
            .map(|p| (p.path, p.location, p.severity, p.message))
            .collect()
    }
}

#[test]
fn a_lint_violation_shows_inline_and_in_problems() {
    let fixture = oxc_project().file("src/main.ts", "let a = 1;\ndebugger;\n").build();
    let fake = FakeLsp::new().warning("debugger;", NO_DEBUGGER);
    let mut session = open(fixture, &fake, "node");

    session.settle();
    session.dispatch(Command::OpenFile("src/main.ts".into()));
    session.settle();

    assert_eq!(
        session.oxlint_problems(),
        [("src/main.ts".into(), "2:1".into(), Severity::Warning, NO_DEBUGGER.into())]
    );
    let editor = session.view().editor.unwrap();
    assert_eq!(
        editor.problems,
        [InlineProblem { line: 1, columns: 0..9, severity: Severity::Warning, message: NO_DEBUGGER.into() }]
    );
}

impl Session {
    /// Every start of `program` (by file name) so far.
    fn spawned(&self, program: &str) -> Vec<ProcessSpec> {
        self.host.processes().spawned().into_iter().filter(|spec| spec.program_name() == program).collect()
    }
}

/// What a program prints when run with `--version`.
fn version_of(program: &Path) -> String {
    let out = std::process::Command::new(program).arg("--version").output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

#[test]
fn oxlint_runs_from_node_modules_on_the_pinned_node_with_the_root_as_its_workspace() {
    let fake = FakeLsp::new();
    let mut session = open(oxc_project().build(), &fake, "node");
    session.settle();

    let root = session.fixture.root().to_owned();
    let spawned = session.spawned("node");
    assert_eq!(spawned.len(), 1, "one Oxlint per project");
    let node = &spawned[0];
    assert!(node.program.starts_with(session.host.support_dir()), "{} isn't from the store", node.program.display());
    assert_eq!(version_of(&node.program), "v24.21.0");
    assert_eq!(node.args, [root.join(OXLINT).into_os_string(), "--lsp".into()]);
    assert_eq!(node.cwd.as_deref(), Some(root.as_path()));
    assert!(node.clear_env, "it gets the project environment");

    let initialize = &fake.received("initialize")[0];
    let name = root.file_name().unwrap().to_str().unwrap();
    assert_eq!(initialize["workspaceFolders"], json!([{ "uri": format!("file://{}", root.display()), "name": name }]));
    let oxlint = session.view().status.language_servers.into_iter().find(|s| s.name == "Oxlint").unwrap();
    assert_eq!(oxlint.state, LanguageServerState::Ready);
}

#[test]
fn a_bun_runtime_pin_runs_oxlint_under_bun() {
    let package_json = r#"{
  "name": "app",
  "packageManager": "bun@1.4.2",
  "devEngines": { "runtime": { "name": "bun", "version": "1.4.2" } },
  "devDependencies": { "oxfmt": "^0.72.0", "oxlint": "^1.87.0" }
}
"#;
    let fixture = project_with(package_json).file("src/main.ts", "debugger;\n").build();
    let fake = FakeLsp::new().warning("debugger;", NO_DEBUGGER);
    let mut session = open(fixture, &fake, "bun");
    session.settle();
    session.dispatch(Command::OpenFile("src/main.ts".into()));
    session.settle();

    let spawned = session.spawned("bun");
    assert_eq!(spawned.len(), 1);
    assert!(spawned[0].program.starts_with(session.host.support_dir()));
    assert_eq!(version_of(&spawned[0].program), "1.4.2");
    assert_eq!(spawned[0].args, [session.fixture.path(OXLINT).into_os_string(), "--lsp".into()]);
    assert_eq!(session.spawned("node"), [], "no Node in a Bun project");
    assert_eq!(session.oxlint_problems().len(), 1);
}

// --- Type-aware linting ---------------------------------------------------------

/// The `typeAware` option Oxlint was started with, per start.
fn type_aware(fake: &FakeLsp) -> Vec<Value> {
    fake.received("initialize").iter().map(|params| params["initializationOptions"][0]["options"]["typeAware"].clone()).collect()
}

#[test]
fn type_aware_linting_is_off_unless_the_oxlintrc_turns_it_on() {
    let fake = FakeLsp::new();
    let mut session = open(oxc_project().file(".oxlintrc.json", r#"{ "categories": {} }"#).build(), &fake, "node");
    session.settle();

    assert_eq!(type_aware(&fake), [json!(false)]);
    let options = &fake.received("initialize")[0]["initializationOptions"][0];
    assert_eq!(options["workspaceUri"], format!("file://{}", session.fixture.root().display()));
}

#[test]
fn turning_type_aware_linting_on_in_the_oxlintrc_restarts_oxlint_with_it() {
    let fake = FakeLsp::new();
    let mut session = open(oxc_project().build(), &fake, "node");
    session.settle();
    assert_eq!(type_aware(&fake), [json!(false)]);

    // JSONC, as Oxlint reads it.
    session.fixture.write(".oxlintrc.json", "{\n  // types\n  \"options\": { \"typeAware\": true, },\n}\n");
    session.settle();

    assert_eq!(type_aware(&fake), [json!(false), json!(true)]);
    assert_eq!(fake.wait_for("exit", 1).len(), 1, "the old one was shut down");
}

// --- Alongside tsgo, and the lifecycle -------------------------------------------

#[test]
fn oxlint_and_tsgo_diagnostics_show_side_by_side() {
    const STORE: &str = "node_modules/.pnpm/typescript@7.0.2/node_modules";
    let fixture = oxc_project()
        .file(format!("{STORE}/typescript/package.json"), r#"{ "name": "typescript", "version": "7.0.2" }"#)
        .file(format!("{STORE}/@typescript/typescript-darwin-arm64/lib/tsc"), "#!/bin/sh\n")
        .file("src/main.ts", "let a: number = \"oops\";\ndebugger;\n")
        .build();
    std::os::unix::fs::symlink(fixture.path(format!("{STORE}/typescript")), fixture.path("node_modules/typescript"))
        .unwrap();
    let oxlint = FakeLsp::new().warning("debugger;", NO_DEBUGGER);
    let tsgo = FakeLsp::new().error("\"oops\"", "Type 'string' is not assignable to type 'number'.");
    let mut session = open(fixture, &oxlint, "node");
    tsgo.install(&session.host, "tsc");
    session.settle();
    session.dispatch(Command::OpenFile("src/main.ts".into()));
    session.settle();

    let problems: Vec<(ProblemSource, String)> =
        session.view().problems.into_iter().map(|p| (p.source, p.location)).collect();
    assert_eq!(problems, [(ProblemSource::TypeScript, "1:17".into()), (ProblemSource::Oxlint, "2:1".into())]);
    assert_eq!(session.view().editor.unwrap().problems.len(), 2);
    let names: Vec<String> = session.view().status.language_servers.into_iter().map(|s| s.name).collect();
    assert_eq!(names, ["TypeScript", "Oxlint"]);

    // Each server's crash and restart is its own.
    oxlint.set_crash_on(Some("textDocument/diagnostic"));
    session.dispatch(Command::InsertText(" ".into()));
    session.settle();
    let sources: Vec<ProblemSource> = session.view().problems.into_iter().map(|p| p.source).collect();
    assert_eq!(sources, [ProblemSource::TypeScript]);
    assert_eq!(session.oxlint_status().unwrap().state, LanguageServerState::Restarting);
    assert_eq!(tsgo.starts(), 1);

    oxlint.set_crash_on(None);
    session.host.clock().advance(RESTART_DELAY);
    session.settle();
    assert_eq!(oxlint.starts(), 2);
    assert_eq!(session.oxlint_problems().len(), 1);
}

#[test]
fn a_crashing_oxlint_is_restarted_three_times_then_marked_failed_until_restarted() {
    let fixture = oxc_project().file("src/main.ts", "debugger;\n").build();
    let fake = FakeLsp::new().warning("debugger;", NO_DEBUGGER).crash_on("textDocument/diagnostic");
    let mut session = open(fixture, &fake, "node");
    session.settle();
    session.dispatch(Command::OpenFile("src/main.ts".into()));
    session.settle();

    for restart in 1..=MAX_RESTARTS {
        session.host.clock().advance(RESTART_DELAY);
        session.settle();
        assert_eq!(fake.starts(), 1 + restart, "restart {restart}");
    }
    assert_eq!(
        session.oxlint_status(),
        Some(LanguageServerStatus { name: "Oxlint".into(), state: LanguageServerState::Failed, label: "Oxlint stopped".into() })
    );
    let notice = session.view().notices.into_iter().find(|n| n.message.contains("Oxlint language server")).unwrap();

    fake.set_crash_on(None);
    session.dispatch(notice.action.unwrap().command);
    session.settle();
    assert_eq!(fake.starts(), 2 + MAX_RESTARTS);
    assert_eq!(session.oxlint_status().unwrap().state, LanguageServerState::Ready);
    assert_eq!(session.oxlint_problems().len(), 1);
}

#[test]
fn closing_the_project_shuts_oxlint_down() {
    let fake = FakeLsp::new();
    let mut session = open(oxc_project().build(), &fake, "node");
    session.settle();

    session.workbench.close_project(session.project);
    session.settle();

    assert_eq!(fake.wait_for("shutdown", 1).len(), 1);
    assert_eq!(fake.wait_for("exit", 1).len(), 1);
}

// --- Without Oxlint and Oxfmt ---------------------------------------------------

const WITHOUT_OXC: &str = "{\n  \"name\": \"app\",\n  \"packageManager\": \"pnpm@12.10.1\",\n  \"devEngines\": { \"runtime\": { \"name\": \"node\", \"version\": \"24.21.0\" } },\n  \"devDependencies\": {\n    \"typescript\": \"^7.0.2\"\n  }\n}\n";

impl Session {
    /// The notice offering "Add Oxlint and Oxfmt", if the window shows one.
    fn add_oxc_notice(&self) -> Option<Notice> {
        let notices = self.view().notices;
        notices.into_iter().find(|n| n.action.as_ref().is_some_and(|a| a.command == Command::AddOxlintAndOxfmt))
    }

    fn oxlint_status(&self) -> Option<LanguageServerStatus> {
        self.view().status.language_servers.into_iter().find(|s| s.name == "Oxlint")
    }
}

#[test]
fn a_project_without_oxlint_and_oxfmt_has_lint_off_and_offers_to_add_them() {
    let fixture = FixtureProject::new().file("package.json", WITHOUT_OXC).file("src/main.ts", "debugger;\n").build();
    let fake = FakeLsp::new().warning("debugger;", NO_DEBUGGER);
    let mut session = open(fixture, &fake, "node");
    session.settle();
    session.dispatch(Command::OpenFile("src/main.ts".into()));
    session.settle();

    let notice = session.add_oxc_notice().expect("no notice offering Oxlint and Oxfmt");
    assert_eq!(notice.message, "Lint and format are off: this project doesn't have Oxlint and Oxfmt.");
    assert_eq!(notice.action.unwrap().label, "Add Oxlint and Oxfmt");
    assert_eq!(
        session.oxlint_status(),
        Some(LanguageServerStatus { name: "Oxlint".into(), state: LanguageServerState::Off, label: "Oxlint off".into() })
    );
    assert_eq!(session.spawned("node"), []);
    assert_eq!(session.oxlint_problems(), []);
}

#[test]
fn oxlint_alone_isnt_enough() {
    let package_json = r#"{ "name": "app", "devDependencies": { "oxlint": "^1.87.0" } }"#;
    let fake = FakeLsp::new();
    let mut session = open(project_with(package_json).build(), &fake, "node");
    session.settle();

    assert!(session.add_oxc_notice().is_some());
    assert_eq!(session.spawned("node"), []);
}

#[test]
fn adding_oxlint_and_oxfmt_writes_them_to_package_json_and_lint_starts_once_installed() {
    let fixture = FixtureProject::new().file("package.json", WITHOUT_OXC).file("src/main.ts", "debugger;\n").build();
    let fake = FakeLsp::new().warning("debugger;", NO_DEBUGGER);
    let mut session = open(fixture, &fake, "node");
    session.settle();
    session.dispatch(Command::OpenFile("src/main.ts".into()));

    let action = session.add_oxc_notice().unwrap().action.unwrap();
    session.dispatch(action.command);
    session.settle();

    assert_eq!(
        session.fixture.read("package.json"),
        "{\n  \"name\": \"app\",\n  \"packageManager\": \"pnpm@12.10.1\",\n  \"devEngines\": {\n    \"runtime\": {\n      \"name\": \"node\",\n      \"version\": \"24.21.0\"\n    }\n  },\n  \"devDependencies\": {\n    \"typescript\": \"^7.0.2\",\n    \"oxfmt\": \"^0.72.0\",\n    \"oxlint\": \"^1.87.0\"\n  }\n}\n"
    );
    assert_eq!(session.add_oxc_notice(), None);
    let waiting = session.view().notices.into_iter().find(|n| n.message.contains("Oxlint is installed"));
    assert!(waiting.is_some(), "a notice says lint waits for the install");
    assert_eq!(session.spawned("node"), []);

    // The user installs the dependencies.
    session.fixture.write("node_modules/oxlint/package.json", r#"{ "name": "oxlint", "version": "1.87.0" }"#);
    session.fixture.write(OXLINT, "#!/usr/bin/env node\n");
    session.settle();

    assert_eq!(session.spawned("node").len(), 1);
    assert_eq!(session.oxlint_problems().len(), 1);
    assert!(session.view().notices.iter().all(|n| !n.message.contains("Oxlint")));
}
