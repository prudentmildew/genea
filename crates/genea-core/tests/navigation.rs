//! Go to definition, find usages and rename (ticket #44), against the fake
//! LSP server (`genea_testkit::FakeLsp`), which plays tsgo. The fake
//! understands identifiers by their text: a word's definition is where a
//! declaration keyword (`function`, `const`, `class`, …) precedes it, and its
//! references are its whole-word occurrences in the project's files.

use std::path::PathBuf;

use genea_core::{Caret, Command, ProjectId, ProjectView, Workbench};
use genea_testkit::{FakeLsp, FixtureBuilder, FixtureProject, TestHost};

const STORE: &str = "node_modules/.pnpm/typescript@7.0.2/node_modules";
const TSGO: &str = "node_modules/.pnpm/typescript@7.0.2/node_modules/@typescript/typescript-darwin-arm64/lib/tsc";

/// A project with TypeScript 7 installed (as pnpm lays it out).
fn typescript_project() -> FixtureBuilder {
    FixtureProject::new()
        .file("package.json", r#"{ "name": "app", "devDependencies": { "typescript": "^7.0.2" } }"#)
        .file("tsconfig.json", "{}")
        .file(format!("{STORE}/typescript/package.json"), r#"{ "name": "typescript", "version": "7.0.2" }"#)
        .file(TSGO, "#!/bin/sh\n")
}

const GREET: &str = "export function greet(name: string) {\n  return `Hello, ${name}`;\n}\n";
const MAIN: &str = "import { greet } from \"./greet\";\n\nconsole.log(greet(\"world\"));\n";

/// `greet` is declared in one file and used in another.
fn greeting_project() -> FixtureBuilder {
    typescript_project().file("src/greet.ts", GREET).file("src/main.ts", MAIN)
}

struct Session {
    _fixture: FixtureProject,
    workbench: Workbench,
    project: ProjectId,
    _host: TestHost,
}

/// Opens `fixture` with tsgo played by `fake`, and settles.
fn open(fixture: FixtureProject, fake: &FakeLsp) -> Session {
    std::os::unix::fs::symlink(fixture.path(format!("{STORE}/typescript")), fixture.path("node_modules/typescript"))
        .unwrap();
    let host = TestHost::new();
    fake.install(&host, "tsc");
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    let mut session = Session { _fixture: fixture, workbench, project, _host: host };
    session.settle();
    session
}

impl Session {
    fn dispatch(&mut self, command: Command) {
        self.workbench.dispatch(self.project, command);
    }

    fn settle(&mut self) {
        self.workbench.settle().unwrap();
    }

    fn view(&self) -> ProjectView {
        self.workbench.project(self.project).unwrap()
    }

    /// Opens a file and puts the caret at a 0-based line and column.
    fn open_at(&mut self, path: &str, line: usize, column: usize) {
        self.dispatch(Command::OpenFile(path.into()));
        self.settle();
        self.dispatch(Command::PlaceCaret { line, column });
    }

    /// The focused file and its caret.
    fn focused(&self) -> (PathBuf, Caret) {
        let editor = self.view().editor.expect("a focused file");
        (editor.path, editor.caret)
    }
}

#[test]
fn go_to_definition_jumps_to_the_declaration_in_another_file() {
    let fake = FakeLsp::new();
    let mut session = open(greeting_project().build(), &fake);
    // On `greet` in `console.log(greet("world"))`.
    session.open_at("src/main.ts", 2, 14);

    session.dispatch(Command::GoToDefinition);
    session.settle();

    assert_eq!(session.focused(), ("src/greet.ts".into(), Caret { line: 0, column: 16 }));
}
