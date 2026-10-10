//! Oxlint diagnostics (ticket #49): `oxlint --lsp` from the project's
//! `node_modules`, run on the pinned runtime, against the fake LSP server
//! (`genea_testkit::FakeLsp`), which the test host plays whenever Genea
//! starts the runtime (`node`, or `bun`).

use std::path::PathBuf;

use genea_core::{Command, InlineProblem, ProblemSource, ProjectId, Severity, Workbench};
use genea_testkit::{FakeLsp, FixtureBuilder, FixtureProject, TestHost};

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
