//! The project check (ticket #48): `tsc -b --noEmit` with the project's
//! TypeScript 7, played by `genea_testkit::FakeTsc` (which hands `--lsp`
//! starts of the same `tsc` to a `FakeLsp`).

use std::path::PathBuf;

use genea_core::{Command, ProblemSource, ProjectId, Severity, Workbench};
use genea_testkit::{FakeLsp, FakeTsc, FixtureBuilder, FixtureProject, TestHost};

/// The tsgo binary in a project with TypeScript 7 installed by pnpm.
const STORE: &str = "node_modules/.pnpm/typescript@7.0.2/node_modules";
const TSGO: &str = "node_modules/.pnpm/typescript@7.0.2/node_modules/@typescript/typescript-darwin-arm64/lib/tsc";

const TYPE_ERROR: &str = "Type 'string' is not assignable to type 'number'.";

/// A project with TypeScript 7 installed (as pnpm lays it out).
fn typescript_project() -> FixtureBuilder {
    FixtureProject::new()
        .file("package.json", r#"{ "name": "app", "devDependencies": { "typescript": "^7.0.2" } }"#)
        .file("tsconfig.json", "{}")
        .file(format!("{STORE}/typescript/package.json"), r#"{ "name": "typescript", "version": "7.0.2" }"#)
        .file(TSGO, "#!/bin/sh\n")
}

struct Session {
    fixture: FixtureProject,
    #[allow(dead_code)]
    host: TestHost,
    workbench: Workbench,
    project: ProjectId,
}

/// Opens `fixture` with `tsc` played by `tsc` (checks) and `lsp` (tsgo).
fn open_with(fixture: FixtureProject, tsc: &FakeTsc, lsp: &FakeLsp) -> Session {
    std::os::unix::fs::symlink(fixture.path(format!("{STORE}/typescript")), fixture.path("node_modules/typescript"))
        .unwrap();
    let host = TestHost::new();
    tsc.install(&host, "tsc", lsp);
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    let mut session = Session { fixture, host, workbench, project };
    session.settle();
    session
}

fn open(fixture: FixtureProject, tsc: &FakeTsc) -> Session {
    open_with(fixture, tsc, &FakeLsp::new())
}

/// A problem as the Problems view lists it: (source, path, location,
/// severity, message).
type Item = (ProblemSource, PathBuf, String, Severity, String);

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

    fn check(&mut self) {
        self.dispatch(Command::RunProjectCheck);
        self.settle();
    }

    /// The project check's problems in Problems.
    fn checked(&self) -> Vec<Item> {
        self.problems().into_iter().filter(|p| p.0 == ProblemSource::ProjectCheck).collect()
    }

    fn problems(&self) -> Vec<Item> {
        self.view().problems.into_iter().map(|p| (p.source, p.path, p.location, p.severity, p.message)).collect()
    }
}

fn error(path: &str, location: &str, message: &str) -> Item {
    (ProblemSource::ProjectCheck, path.into(), location.into(), Severity::Error, message.into())
}

#[test]
fn a_project_check_fills_problems_with_errors_from_unopened_files() {
    let fixture = typescript_project()
        .file("src/main.ts", "let a: number = \"oops\";\n")
        .file("src/util.ts", "// 😀\nfoo();\n")
        .build();
    let tsc = FakeTsc::new().reports(
        &format!(
            "src/main.ts(1,17): error TS2322: {TYPE_ERROR}\n\
             src/util.ts(2,1): error TS2304: Cannot find name 'foo'.\n"
        ),
        1,
    );
    let mut session = open(fixture, &tsc);

    session.check();

    assert_eq!(
        session.checked(),
        [error("src/main.ts", "1:17", TYPE_ERROR), error("src/util.ts", "2:1", "Cannot find name 'foo'.")]
    );
    let root = session.fixture.root().to_owned();
    let checks = tsc.checks();
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].program, root.join(TSGO), "the project's own TypeScript 7");
    assert_eq!(checks[0].args, ["-b", "--noEmit", "--pretty", "false"]);
    assert_eq!(checks[0].cwd.as_deref(), Some(root.as_path()));
    assert!(checks[0].clear_env, "it gets the project environment");
}
