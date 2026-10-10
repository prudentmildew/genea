//! Go to definition, find usages and rename (ticket #44), against the fake
//! LSP server (`genea_testkit::FakeLsp`), which plays tsgo. The fake
//! understands identifiers by their text: a word's definition is where a
//! declaration keyword (`function`, `const`, `class`, …) precedes it, and its
//! references are its whole-word occurrences in the project's files.

use std::path::PathBuf;

use genea_core::{Caret, Command, LeftColumnView, ProjectId, ProjectView, TextPosition, Workbench};
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
const MAIN: &str = "import { greet } from \"./greeting\";\n\nconsole.log(greet(\"world\"));\n";

/// `greet` is declared in one file and used in another.
fn greeting_project() -> FixtureBuilder {
    typescript_project().file("src/greeting.ts", GREET).file("src/main.ts", MAIN)
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

    assert_eq!(session.focused(), ("src/greeting.ts".into(), Caret { line: 0, column: 16 }));
}

/// The Usages view's places: (file, `line:column`, the matched text).
fn usages(view: &ProjectView) -> Vec<(PathBuf, String, String)> {
    let usages = view.usages.as_ref().expect("the Usages view");
    let places: Vec<_> = usages
        .files
        .iter()
        .flat_map(|f| f.matches.iter().map(|m| (f.path.clone(), m.location.clone(), m.matched.clone())))
        .collect();
    assert_eq!(usages.count, places.len());
    places
}

#[test]
fn find_usages_lists_every_reference_by_file_and_a_click_opens_one() {
    let fake = FakeLsp::new();
    let fixture = greeting_project()
        .file("src/app/welcome.ts", "import { greet } from \"../greeting\";\nexport const welcome = greet(\"you\") + greet(\"me\");\n")
        .build();
    let mut session = open(fixture, &fake);
    // On `greet` in its declaration.
    session.open_at("src/greeting.ts", 0, 18);

    session.dispatch(Command::FindUsages);
    session.settle();

    let view = session.view();
    assert_eq!(view.left_column, Some(LeftColumnView::Usages));
    let usages_view = view.usages.as_ref().unwrap();
    assert_eq!(usages_view.title, "Usages of greet");
    assert!(!usages_view.finding);
    let greet = String::from("greet");
    assert_eq!(
        usages(&view),
        [
            ("src/app/welcome.ts".into(), "1:10".into(), greet.clone()),
            ("src/app/welcome.ts".into(), "2:24".into(), greet.clone()),
            ("src/app/welcome.ts".into(), "2:39".into(), greet.clone()),
            ("src/main.ts".into(), "1:10".into(), greet.clone()),
            ("src/main.ts".into(), "3:13".into(), greet.clone()),
        ],
        "every reference but the declaration, folders first as in the Files view"
    );
    let usage = &view.usages.as_ref().unwrap().files[1].matches[1];
    assert_eq!((usage.before.as_str(), usage.after.as_str()), ("console.log(", "(\"world\"));"));

    // A click on a usage.
    assert_eq!(usage.position, TextPosition { line: 2, column: 12 });
    session.dispatch(Command::OpenFileAt { path: "src/main.ts".into(), at: usage.position });
    session.settle();
    assert_eq!(session.focused(), ("src/main.ts".into(), Caret { line: 2, column: 12 }));
}

#[test]
fn find_usages_says_so_while_waiting_and_when_there_are_none() {
    let fake = FakeLsp::new();
    let mut session = open(typescript_project().file("src/lonely.ts", "export const lonely = 1;\n").build(), &fake);
    session.open_at("src/lonely.ts", 0, 15);

    session.dispatch(Command::FindUsages);
    let waiting = session.view().usages.unwrap();
    assert_eq!((waiting.title.as_str(), waiting.finding), ("Usages of lonely", true));
    session.settle();

    let found = session.view().usages.unwrap();
    assert_eq!((found.count, found.finding), (0, false));
}
