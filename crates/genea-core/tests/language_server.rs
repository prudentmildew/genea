//! tsgo and live diagnostics (ticket #42), against the fake LSP server
//! (`genea_testkit::FakeLsp`), which the test host plays whenever Genea
//! starts the project's `tsc`.

use std::path::PathBuf;

use genea_core::{Command, InlineProblem, ProblemSource, ProjectId, Severity, Workbench};
use genea_testkit::{FakeLsp, FixtureBuilder, FixtureProject, TestHost};

/// The tsgo binary in a project with TypeScript 7 installed by pnpm: the
/// platform package sits beside `typescript` in the virtual store.
const STORE: &str = "node_modules/.pnpm/typescript@7.0.2/node_modules";
const TSGO: &str = "node_modules/.pnpm/typescript@7.0.2/node_modules/@typescript/typescript-darwin-arm64/lib/tsc";

const TYPE_ERROR: &str = "Type 'string' is not assignable to type 'number'.";

/// A project with TypeScript 7 installed (as pnpm lays it out, with
/// `node_modules/typescript` a symlink into the store).
fn typescript_project() -> FixtureBuilder {
    FixtureProject::new()
        .file("package.json", r#"{ "name": "app", "devDependencies": { "typescript": "^7.0.2" } }"#)
        .file("tsconfig.json", "{}")
        .file(format!("{STORE}/typescript/package.json"), r#"{ "name": "typescript", "version": "7.0.2" }"#)
        .file(TSGO, "#!/bin/sh\n")
}

fn link_typescript(fixture: &FixtureProject) {
    std::os::unix::fs::symlink(fixture.path(format!("{STORE}/typescript")), fixture.path("node_modules/typescript"))
        .unwrap();
}

struct Session {
    fixture: FixtureProject,
    host: TestHost,
    workbench: Workbench,
    project: ProjectId,
}

/// Opens `fixture` with tsgo played by `fake`.
fn open(fixture: FixtureProject, fake: &FakeLsp) -> Session {
    link_typescript(&fixture);
    let host = TestHost::new();
    fake.install(&host, "tsc");
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

    /// TypeScript's problems: (path, location, severity, message).
    fn typescript_problems(&self) -> Vec<(PathBuf, String, Severity, String)> {
        self.view()
            .problems
            .into_iter()
            .filter(|p| p.source == ProblemSource::TypeScript)
            .map(|p| (p.path, p.location, p.severity, p.message))
            .collect()
    }
}

#[test]
fn opening_a_file_with_a_type_error_shows_it_inline_and_in_problems() {
    let fixture = typescript_project().file("src/main.ts", "let a: number = \"oops\";\n").build();
    let fake = FakeLsp::new().error("\"oops\"", TYPE_ERROR);
    let mut session = open(fixture, &fake);

    session.dispatch(Command::OpenFile("src/main.ts".into()));
    session.settle();

    assert_eq!(
        session.typescript_problems(),
        [("src/main.ts".into(), "1:17".into(), Severity::Error, TYPE_ERROR.into())]
    );
    let editor = session.view().editor.unwrap();
    assert_eq!(
        editor.problems,
        [InlineProblem { line: 0, columns: 16..22, severity: Severity::Error, message: TYPE_ERROR.into() }]
    );
}
