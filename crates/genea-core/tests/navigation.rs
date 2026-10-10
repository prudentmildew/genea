//! Go to definition, find usages and rename (ticket #44), against the fake
//! LSP server (`genea_testkit::FakeLsp`), which plays tsgo. The fake
//! understands identifiers by their text: a word's definition is where a
//! declaration keyword (`function`, `const`, `class`, …) precedes it, and its
//! references are its whole-word occurrences in the project's files.

use std::path::PathBuf;

use genea_core::{Caret, Command, LeftColumnView, ProjectId, ProjectView, RenamePrompt, TextPosition, Workbench};
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
const MAIN_RENAMED: &str = "import { welcome } from \"./greeting\";\n\nconsole.log(welcome(\"world\"));\n";

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

const SHAPES: &str = "export interface Shape {\n  area(): number;\n}\n";
const CIRCLE: &str = "import { Shape } from \"./shapes\";\nexport class Circle implements Shape {\n  area() { return 3; }\n}\n";
const SQUARE: &str = "import { Shape } from \"./shapes\";\nexport class Square implements Shape {\n  area() { return 4; }\n}\n";
const DRAW: &str = "import { Circle } from \"./circle\";\nconst shape: Shape = new Circle();\nshape.area();\n";

/// An interface with two implementations, and a variable of its type.
fn shapes_project() -> FixtureBuilder {
    typescript_project()
        .file("src/shapes.ts", SHAPES)
        .file("src/circle.ts", CIRCLE)
        .file("src/square.ts", SQUARE)
        .file("src/draw.ts", DRAW)
}

#[test]
fn go_to_type_definition_jumps_to_the_type_of_the_symbol() {
    let fake = FakeLsp::new();
    let mut session = open(shapes_project().build(), &fake);
    // On `shape` in `shape.area()`.
    session.open_at("src/draw.ts", 2, 2);

    session.dispatch(Command::GoToTypeDefinition);
    session.settle();

    assert_eq!(session.focused(), ("src/shapes.ts".into(), Caret { line: 0, column: 17 }));
}

#[test]
fn several_implementations_are_listed_in_the_usages_view_and_one_opens_at_once() {
    let fake = FakeLsp::new();
    let mut session = open(shapes_project().build(), &fake);
    session.open_at("src/shapes.ts", 0, 18);

    session.dispatch(Command::GoToImplementation);
    session.settle();

    let view = session.view();
    assert_eq!(view.editor.as_ref().unwrap().path, PathBuf::from("src/shapes.ts"), "nothing opens yet");
    assert_eq!(view.left_column, Some(LeftColumnView::Usages));
    assert_eq!(view.usages.as_ref().unwrap().title, "Implementations of Shape");
    assert_eq!(
        usages(&view),
        [
            ("src/circle.ts".into(), "2:14".into(), "Circle".into()),
            ("src/square.ts".into(), "2:14".into(), "Square".into()),
        ]
    );

    // With one implementation, it opens.
    session.dispatch(Command::CloseTab { pane: 0, tab: 0 });
    std::fs::remove_file(session._fixture.path("src/square.ts")).unwrap();
    session.open_at("src/shapes.ts", 0, 18);
    session.dispatch(Command::GoToImplementation);
    session.settle();
    assert_eq!(session.focused(), ("src/circle.ts".into(), Caret { line: 1, column: 13 }));
}

#[test]
fn nothing_found_or_no_language_server_is_a_hint_until_the_next_command() {
    let fake = FakeLsp::new();
    let mut session = open(greeting_project().file("README.md", "# greet\n").build(), &fake);
    // On `console`, which nothing declares.
    session.open_at("src/main.ts", 2, 3);

    session.dispatch(Command::GoToDefinition);
    session.settle();

    assert_eq!(session.focused(), ("src/main.ts".into(), Caret { line: 2, column: 3 }));
    assert_eq!(session.view().hint.as_deref(), Some("No definition found for `console`."));
    session.dispatch(Command::MoveCaret(genea_core::CaretMove::Right));
    assert_eq!(session.view().hint, None);

    session.open_at("README.md", 0, 3);
    session.dispatch(Command::GoToDefinition);
    assert_eq!(session.view().hint.as_deref(), Some("Only TypeScript and JavaScript files have code navigation."));
}

#[test]
fn without_typescript_there_is_no_navigation() {
    let fixture = FixtureProject::new().file("package.json", "{}").file("src/main.ts", MAIN).build();
    let host = TestHost::new();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::OpenFile("src/main.ts".into()));
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::FindUsages);
    workbench.settle().unwrap();

    let view = workbench.project(project).unwrap();
    assert_eq!(view.hint.as_deref(), Some("TypeScript isn't running, so there's no Find Usages."));
    assert_eq!(view.usages, None);
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
fn find_action_runs_navigation_by_name_with_its_shortcut() {
    let fake = FakeLsp::new();
    let mut session = open(greeting_project().build(), &fake);
    session.open_at("src/main.ts", 2, 14);

    session.dispatch(Command::OpenFinder(genea_core::FinderMode::Actions));
    session.dispatch(Command::SetFinderQuery("find usages".into()));
    session.settle();
    let item = session.view().finder.unwrap().items[0].clone();
    assert_eq!((item.label.as_str(), item.shortcut.as_deref()), ("Find Usages", Some("⌥F7")));
    session.dispatch(Command::AcceptFinder);
    session.settle();

    assert_eq!(session.view().usages.unwrap().title, "Usages of greet");
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

impl Session {
    /// The focused file's text, from its visible lines.
    fn text(&self) -> String {
        let editor = self.view().editor.expect("a focused file");
        editor.lines.iter().map(|l| format!("{}\n", l.text)).collect::<String>().trim_end().to_owned() + "\n"
    }

    /// The tabs of the left pane: (file, modified).
    fn tabs(&self) -> Vec<(PathBuf, bool)> {
        self.view().panes[0].tabs.iter().map(|t| (t.path.clone(), t.modified)).collect()
    }

    fn select_tab(&mut self, path: &str) {
        let tab = self.tabs().iter().position(|(p, _)| p == std::path::Path::new(path)).expect("a tab");
        self.dispatch(Command::SelectTab { pane: 0, tab });
    }
}

#[test]
fn rename_edits_every_reference_and_opens_the_files_that_were_not_open() {
    let fake = FakeLsp::new();
    let mut session = open(greeting_project().build(), &fake);
    session.open_at("src/main.ts", 2, 14);

    session.dispatch(Command::StartRename);
    session.settle();
    assert_eq!(session.view().rename, Some(RenamePrompt { name: "greet".into() }));

    session.dispatch(Command::Rename("welcome".into()));
    session.settle();

    let view = session.view();
    assert_eq!(view.rename, None);
    assert_eq!(view.editor.unwrap().path, PathBuf::from("src/main.ts"), "the focus stays");
    assert_eq!(session.text(), MAIN_RENAMED);
    assert_eq!(
        session.tabs(),
        [("src/main.ts".into(), true), ("src/greeting.ts".into(), true)],
        "the file that wasn't open opens behind, unsaved"
    );
    assert_eq!(session._fixture.read("src/greeting.ts"), GREET, "nothing is written yet");
    session.select_tab("src/greeting.ts");
    assert_eq!(session.text(), GREET.replace("greet", "welcome"));

    // Each file's edits are one undo step.
    session.dispatch(Command::Undo);
    assert_eq!(session.text(), GREET);
    session.dispatch(Command::Redo);

    // Saving them is Genea's own write: no external change, no conflict.
    session.dispatch(Command::Save);
    session.select_tab("src/main.ts");
    session.dispatch(Command::Save);
    session.settle();
    assert_eq!(session._fixture.read("src/greeting.ts"), GREET.replace("greet", "welcome"));
    assert_eq!(session._fixture.read("src/main.ts"), MAIN_RENAMED);
    assert_eq!(session.tabs(), [("src/main.ts".into(), false), ("src/greeting.ts".into(), false)]);
    assert!(!session.view().editor.unwrap().conflict);
    session.select_tab("src/greeting.ts");
    assert!(!session.view().editor.unwrap().conflict);
    session.dispatch(Command::Undo);
    assert_eq!(session.text(), GREET, "the rename is still the last undo step: nothing reloaded");
}

#[test]
fn a_cancelled_or_unchanged_rename_edits_nothing() {
    let fake = FakeLsp::new();
    let mut session = open(greeting_project().build(), &fake);
    session.open_at("src/main.ts", 2, 14);

    session.dispatch(Command::StartRename);
    session.settle();
    session.dispatch(Command::CancelRename);
    assert_eq!(session.view().rename, None);

    session.dispatch(Command::StartRename);
    session.settle();
    session.dispatch(Command::Rename("greet".into()));
    session.settle();
    assert_eq!(session.view().rename, None);
    assert_eq!(session.tabs(), [("src/main.ts".into(), false)]);
    assert!(fake.received("textDocument/rename").is_empty());
}

#[test]
fn rename_where_nothing_can_be_renamed_is_a_hint() {
    let fake = FakeLsp::new();
    let mut session = open(greeting_project().build(), &fake);
    // On the blank line.
    session.open_at("src/main.ts", 1, 0);

    session.dispatch(Command::StartRename);
    session.settle();

    assert_eq!(session.view().rename, None);
    assert_eq!(session.view().hint.as_deref(), Some("Nothing to rename here."));
}
