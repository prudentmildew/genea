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
    #[allow(dead_code)] // Keeps the temp folder.
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
}

fn titles(view: Option<QuickFixesView>) -> Vec<String> {
    view.expect("the quick fixes are showing").items
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
