//! Symbols in the finder (ticket #47): the current file's symbols (⌘F12,
//! `textDocument/documentSymbol`), the project's (⌥⌘O,
//! `workspace/symbol`), and symbols in Search Everywhere (⇧⇧), against the
//! fake LSP server, which reads the declarations in the files.

use genea_core::{Caret, Command, FinderMode, ProjectId, RESTART_DELAY, Workbench};
use genea_testkit::{FakeLsp, FixtureBuilder, FixtureProject, TestHost};

/// The tsgo binary in a project with TypeScript 7 installed by pnpm.
const STORE: &str = "node_modules/.pnpm/typescript@7.0.2/node_modules";
const TSGO: &str = "node_modules/.pnpm/typescript@7.0.2/node_modules/@typescript/typescript-darwin-arm64/lib/tsc";

/// A project with TypeScript 7 installed (`node_modules/typescript` is
/// linked when it opens).
fn typescript_project() -> FixtureBuilder {
    FixtureProject::new()
        .file("package.json", r#"{ "name": "app", "devDependencies": { "typescript": "^7.0.2" } }"#)
        .file("tsconfig.json", "{}")
        .file(format!("{STORE}/typescript/package.json"), r#"{ "name": "typescript", "version": "7.0.2" }"#)
        .file(TSGO, "#!/bin/sh\n")
}

const SHAPES: &str = "\
export interface Shape {
  area(): number;
}

export class Circle implements Shape {
  radius = 1;
  area() {
    return Math.PI * this.radius ** 2;
  }
}

export function circle(radius: number): Circle {
  return new Circle();
}
";

struct Session {
    _fixture: FixtureProject,
    host: TestHost,
    workbench: Workbench,
    project: ProjectId,
}

/// Opens `fixture` with tsgo played by the fake server, and waits until it
/// is ready.
fn open(fixture: FixtureBuilder) -> Session {
    open_with(fixture, &FakeLsp::new())
}

/// Opens `fixture` with tsgo played by `fake`, and settles.
fn open_with(fixture: FixtureBuilder, fake: &FakeLsp) -> Session {
    let fixture = fixture.build();
    std::os::unix::fs::symlink(fixture.path(format!("{STORE}/typescript")), fixture.path("node_modules/typescript"))
        .unwrap();
    let host = TestHost::new();
    fake.install(&host, "tsc");
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    Session { _fixture: fixture, host, workbench, project }
}

impl Session {
    fn dispatch(&mut self, command: Command) {
        self.workbench.dispatch(self.project, command);
        self.workbench.settle().unwrap();
    }

    /// The finder's results as the user reads them: `label  detail`.
    fn results(&self) -> Vec<String> {
        let finder = self.workbench.project(self.project).unwrap().finder.expect("the finder is open");
        let items = finder.items.iter();
        items.map(|item| if item.detail.is_empty() { item.label.clone() } else { format!("{}  {}", item.label, item.detail) }).collect()
    }

    /// The focused editor's file and caret.
    fn caret(&self) -> (String, Caret) {
        let editor = self.workbench.project(self.project).unwrap().editor.expect("a file is open");
        (editor.path.to_string_lossy().into_owned(), editor.caret)
    }
}

#[test]
fn file_symbols_list_the_current_files_symbols_and_choosing_one_moves_the_caret() {
    let mut session = open(typescript_project().file("src/shapes.ts", SHAPES));
    session.dispatch(Command::OpenFile("src/shapes.ts".into()));

    session.dispatch(Command::OpenFinder(FinderMode::FileSymbols));
    // In the file's order, members with their class or interface beside them.
    assert_eq!(session.results(), ["Shape", "area  Shape", "Circle", "radius  Circle", "area  Circle", "circle"]);

    session.dispatch(Command::SetFinderQuery("rad".into()));
    assert_eq!(session.results(), ["radius  Circle"]);

    session.dispatch(Command::AcceptFinder);
    assert!(session.workbench.project(session.project).unwrap().finder.is_none());
    assert_eq!(session.caret(), ("src/shapes.ts".into(), Caret { line: 5, column: 2 }));
}

#[test]
fn project_symbols_find_symbols_across_the_project_and_open_them() {
    let mut session = open(
        typescript_project()
            .file("src/shapes.ts", SHAPES)
            // The name comes after a non-ASCII comment: tsgo counts UTF-8
            // bytes, the caret counts characters.
            .file("src/main.ts", "/* café */ export function drawCircle() {}\n"),
    );
    session.dispatch(Command::OpenFile("src/shapes.ts".into()));

    session.dispatch(Command::OpenFinder(FinderMode::ProjectSymbols));
    assert!(session.results().is_empty(), "nothing before a query");

    session.dispatch(Command::SetFinderQuery("circle".into()));
    assert_eq!(session.results(), ["Circle  src/shapes.ts", "circle  src/shapes.ts", "drawCircle  src/main.ts"]);

    session.dispatch(Command::SetFinderQuery("drawc".into()));
    assert_eq!(session.results(), ["drawCircle  src/main.ts"]);
    session.dispatch(Command::AcceptFinder);
    assert_eq!(session.caret(), ("src/main.ts".into(), Caret { line: 0, column: 27 }));

    // Members show what they are declared in.
    session.dispatch(Command::OpenFinder(FinderMode::ProjectSymbols));
    session.dispatch(Command::SetFinderQuery("radius".into()));
    assert_eq!(session.results(), ["radius  Circle · src/shapes.ts"]);
}

#[test]
fn project_symbols_leave_out_node_modules_and_what_the_config_excludes() {
    let mut session = open(
        typescript_project()
            .file("src/area.ts", "export function areaOf() {}\n")
            .file("dist/area.js", "export function areaOf() {}\n")
            .file("node_modules/geometry/index.d.ts", "export declare function areaOf(): number;\n")
            .file("genea.jsonc", r#"{ "exclude": ["dist/"] }"#),
    );

    session.dispatch(Command::OpenFinder(FinderMode::ProjectSymbols));
    session.dispatch(Command::SetFinderQuery("areaOf".into()));
    assert_eq!(session.results(), ["areaOf  src/area.ts"]);
}

#[test]
fn search_everywhere_includes_symbols_and_choosing_one_opens_it() {
    let mut session = open(typescript_project().file("src/shapes.ts", SHAPES).file("src/circle.css", ""));

    session.dispatch(Command::OpenFinder(FinderMode::Everywhere));
    session.dispatch(Command::SetFinderQuery("circle".into()));
    // Whole names beat part of a file name.
    assert_eq!(session.results(), ["Circle  src/shapes.ts", "circle  src/shapes.ts", "circle.css  src"]);

    session.dispatch(Command::SetFinderQuery("restart".into()));
    assert_eq!(session.results(), ["Restart Language Server"]);

    session.dispatch(Command::SetFinderQuery("circle".into()));
    session.dispatch(Command::MoveFinderSelection(1));
    session.dispatch(Command::AcceptFinder);
    assert_eq!(session.caret(), ("src/shapes.ts".into(), Caret { line: 11, column: 16 }));
}

#[test]
fn file_symbols_asked_for_while_the_server_restarts_come_once_it_is_ready() {
    let fake = FakeLsp::new().crash_on("initialize");
    let mut session = open_with(typescript_project().file("src/shapes.ts", SHAPES), &fake);
    session.dispatch(Command::OpenFile("src/shapes.ts".into()));
    session.dispatch(Command::OpenFinder(FinderMode::FileSymbols));
    assert!(session.results().is_empty());

    fake.set_crash_on(None);
    session.host.clock().advance(RESTART_DELAY);
    session.workbench.settle().unwrap();
    assert_eq!(session.results(), ["Shape", "area  Shape", "Circle", "radius  Circle", "area  Circle", "circle"]);
}
