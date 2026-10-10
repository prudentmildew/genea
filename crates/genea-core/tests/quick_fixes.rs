//! Quick fixes (⌥⏎) and organize imports (⌃⌥O) (ticket #45), against the
//! fake LSP server (`genea_testkit::FakeLsp`) playing tsgo.

use genea_core::{Command, ProjectId, QuickFixesView, Workbench};
use genea_testkit::{FakeLsp, FixtureBuilder, FixtureProject, TestHost};
use serde_json::json;

/// The tsgo binary in a project with TypeScript 7 installed by pnpm.
const STORE: &str = "node_modules/.pnpm/typescript@7.0.2/node_modules";
const TSGO: &str = "node_modules/.pnpm/typescript@7.0.2/node_modules/@typescript/typescript-darwin-arm64/lib/tsc";

const MISSING_NAME: &str = "Cannot find name 'helper'.";
const ADD_IMPORT: &str = "Add import from \"./util.js\"";
const IMPORT: &str = "import { helper } from \"./util.js\";\n";

/// A project with TypeScript 7 installed and `src/main.ts`.
fn typescript_project(main: &str) -> FixtureBuilder {
    FixtureProject::new()
        .file("package.json", r#"{ "name": "app", "devDependencies": { "typescript": "^7.0.2" } }"#)
        .file("tsconfig.json", "{}")
        .file(format!("{STORE}/typescript/package.json"), r#"{ "name": "typescript", "version": "7.0.2" }"#)
        .file(TSGO, "#!/bin/sh\n")
        .file("src/main.ts", main)
}

/// tsgo reporting `helper` as missing, and offering to import it.
fn missing_import() -> FakeLsp {
    FakeLsp::new().marker("helper", 1, MISSING_NAME, Some(2304)).quick_fix(ADD_IMPORT, "helper", &[("", IMPORT)])
}

struct Session {
    fixture: FixtureProject,
    workbench: Workbench,
    project: ProjectId,
}

/// Opens the project with tsgo played by `fake`, and `src/main.ts` in the
/// editor with its diagnostics in.
fn open(fixture: FixtureBuilder, fake: &FakeLsp) -> Session {
    let fixture = fixture.build();
    std::os::unix::fs::symlink(fixture.path(format!("{STORE}/typescript")), fixture.path("node_modules/typescript"))
        .unwrap();
    let host = TestHost::new();
    fake.install(&host, "tsc");
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    let mut session = Session { fixture, workbench, project };
    session.dispatch(Command::OpenFile("src/main.ts".into()));
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

    fn view(&self) -> genea_core::ProjectView {
        self.workbench.project(self.project).unwrap()
    }

    fn quick_fixes(&self) -> Option<QuickFixesView> {
        self.view().quick_fixes
    }

    /// The focused file's text.
    fn text(&self) -> String {
        let lines: Vec<String> = self.view().editor.unwrap().lines.into_iter().map(|line| line.text).collect();
        lines.join("\n")
    }

    /// The primary caret: (line, column).
    fn caret(&self) -> (usize, usize) {
        let caret = self.view().editor.unwrap().caret;
        (caret.line, caret.column)
    }

    /// Opens the popup at a place and waits for the servers.
    fn show_quick_fixes_at(&mut self, line: usize, column: usize) {
        self.dispatch(Command::PlaceCaret { line, column });
        self.dispatch(Command::ShowQuickFixes);
        self.settle();
    }
}

#[test]
fn alt_enter_lists_the_servers_fixes_for_the_problem_at_the_caret() {
    let fake = missing_import();
    let mut session = open(typescript_project("export const x = helper();\n"), &fake);
    session.dispatch(Command::PlaceCaret { line: 0, column: 19 });

    session.dispatch(Command::ShowQuickFixes);
    session.settle();

    assert_eq!(session.quick_fixes(), Some(QuickFixesView { items: vec![ADD_IMPORT.into()], selected: 0 }));
    // The server gets the problem it is asked to fix, code and all.
    let request = &fake.received("textDocument/codeAction")[0];
    assert_eq!(request["context"]["diagnostics"][0]["code"], json!(2304));
    assert_eq!(request["range"]["start"], json!({ "line": 0, "character": 19 }));
}

#[test]
fn choosing_a_fix_applies_its_edit_as_one_undo_step() {
    let fake = missing_import();
    let mut session = open(typescript_project("export const x = helper();\n"), &fake);
    session.show_quick_fixes_at(0, 19);

    session.dispatch(Command::ApplyQuickFix(0));
    session.settle();

    assert_eq!(session.text(), format!("{IMPORT}export const x = helper();\n"));
    assert_eq!(session.caret(), (1, 19), "the caret stays on its code");
    assert_eq!(session.quick_fixes(), None, "the popup closes");

    session.dispatch(Command::Undo);
    assert_eq!(session.text(), "export const x = helper();\n");
}

#[test]
fn the_popup_says_when_there_are_no_fixes() {
    let fake = missing_import();
    let mut session = open(typescript_project("export const x = helper();\nlet y = 2;\n"), &fake);

    session.show_quick_fixes_at(1, 4);

    assert_eq!(session.quick_fixes(), Some(QuickFixesView { items: vec![], selected: 0 }));
}

#[test]
fn without_a_language_server_the_popup_says_there_are_no_fixes() {
    let fixture = FixtureProject::new().file("package.json", "{}").file("src/main.ts", "let a = 1;\n").build();
    let host = TestHost::new();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::OpenFile("src/main.ts".into()));
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::ShowQuickFixes);

    assert_eq!(workbench.project(project).unwrap().quick_fixes, Some(QuickFixesView { items: vec![], selected: 0 }));
}

#[test]
fn up_and_down_move_the_selection_around_the_fixes() {
    let fake = missing_import().quick_fix("Add all missing imports", "helper", &[("", IMPORT)]);
    let mut session = open(typescript_project("export const x = helper();\n"), &fake);
    session.show_quick_fixes_at(0, 19);
    assert_eq!(session.quick_fixes().unwrap().items, [ADD_IMPORT, "Add all missing imports"]);

    session.dispatch(Command::MoveQuickFixSelection(1));
    assert_eq!(session.quick_fixes().unwrap().selected, 1);
    session.dispatch(Command::MoveQuickFixSelection(1));
    assert_eq!(session.quick_fixes().unwrap().selected, 0, "it wraps around");
    session.dispatch(Command::MoveQuickFixSelection(-1));
    assert_eq!(session.quick_fixes().unwrap().selected, 1);
}

#[test]
fn any_other_command_closes_the_popup() {
    let fake = missing_import();
    let mut session = open(typescript_project("export const x = helper();\n"), &fake);
    session.show_quick_fixes_at(0, 19);

    session.dispatch(Command::InsertText("h".into()));
    session.settle();
    assert_eq!(session.quick_fixes(), None);

    session.show_quick_fixes_at(0, 19);
    session.dispatch(Command::CloseQuickFixes);
    assert_eq!(session.quick_fixes(), None);
}

#[test]
fn an_answer_that_comes_after_the_popup_closed_is_dropped() {
    let fake = missing_import().delay(std::time::Duration::from_millis(200));
    let mut session = open(typescript_project("export const x = helper();\n"), &fake);
    session.dispatch(Command::PlaceCaret { line: 0, column: 19 });

    session.dispatch(Command::ShowQuickFixes);
    session.dispatch(Command::CloseQuickFixes);
    session.settle();

    assert_eq!(session.quick_fixes(), None);
}

#[test]
fn a_fix_for_a_file_that_changed_on_disk_since_is_not_applied() {
    let fake = missing_import();
    let mut session = open(typescript_project("export const x = helper();\n"), &fake);
    session.show_quick_fixes_at(0, 19);

    // The file is reloaded under the open popup.
    session.fixture.write("src/main.ts", "export const z = helper();\n");
    session.settle();
    session.dispatch(Command::ApplyQuickFix(0));
    session.settle();

    assert_eq!(session.text(), "export const z = helper();\n");
    let notices: Vec<String> = session.view().notices.into_iter().map(|n| n.message).collect();
    assert!(notices.iter().any(|n| n.contains("changed since")), "{notices:?}");
}

const UNSORTED: &str = "import { b } from \"./b.js\";\nimport { a } from \"./a.js\";\nconsole.log(a, b);\n";
const SORTED: &str = "import { a } from \"./a.js\";\nimport { b } from \"./b.js\";\nconsole.log(a, b);\n";

/// tsgo organizing `UNSORTED` as it does: one edit rewriting the first
/// import line as both, sorted, and one deleting the second line.
fn organizer() -> FakeLsp {
    FakeLsp::new().organize_imports(&[
        ("import { b } from \"./b.js\";\n", "import { a } from \"./a.js\";\nimport { b } from \"./b.js\";\n"),
        ("import { a } from \"./a.js\";\n", ""),
    ])
}

#[test]
fn ctrl_alt_o_organizes_the_imports_of_the_current_file_as_one_undo_step() {
    let fake = organizer();
    let mut session = open(typescript_project(UNSORTED), &fake);
    session.dispatch(Command::PlaceCaret { line: 2, column: 4 });

    session.dispatch(Command::OrganizeImports);
    session.settle();

    assert_eq!(session.text(), SORTED);
    assert_eq!(session.caret(), (2, 4));
    let request = &fake.received("textDocument/codeAction")[0];
    assert_eq!(request["context"]["only"], json!(["source.organizeImports"]));

    session.dispatch(Command::Undo);
    assert_eq!(session.text(), UNSORTED);
}

#[test]
fn find_action_runs_organize_imports() {
    let fake = organizer();
    let mut session = open(typescript_project(UNSORTED), &fake);

    session.dispatch(Command::OpenFinder(genea_core::FinderMode::Actions));
    session.dispatch(Command::SetFinderQuery("organize imports".into()));
    session.settle();
    let finder = session.view().finder.unwrap();
    assert_eq!(finder.items[0].label, "Organize Imports");
    assert_eq!(finder.items[0].shortcut.as_deref(), Some("⌃⌥O"));
    session.dispatch(Command::AcceptFinder);
    session.settle();

    assert_eq!(session.text(), SORTED);
}

#[test]
fn imports_are_not_organized_if_the_file_changes_before_the_server_answers() {
    let fake = organizer().delay(std::time::Duration::from_millis(200));
    let mut session = open(typescript_project(UNSORTED), &fake);
    session.dispatch(Command::MoveCaret(genea_core::CaretMove::DocumentEnd));

    session.dispatch(Command::OrganizeImports);
    session.dispatch(Command::InsertText("//".into()));
    session.settle();

    assert_eq!(session.text(), format!("{UNSORTED}//"));
}

#[test]
fn saving_never_organizes_imports() {
    let fake = organizer();
    let mut session = open(typescript_project(UNSORTED), &fake);
    session.dispatch(Command::MoveCaret(genea_core::CaretMove::DocumentEnd));
    session.dispatch(Command::InsertText("// edited\n".into()));

    session.dispatch(Command::Save);
    session.settle();

    assert_eq!(session.fixture.read("src/main.ts"), format!("{UNSORTED}// edited\n"));
    assert_eq!(fake.received("textDocument/codeAction"), Vec::<serde_json::Value>::new());
}
