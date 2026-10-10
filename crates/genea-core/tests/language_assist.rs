//! Completions, hover and signature help (ticket #43), against the fake LSP
//! server (`genea_testkit::FakeLsp`), which the test host plays whenever
//! Genea starts the project's `tsc`.

use genea_core::{Caret, Command, CompletionItem, CompletionKind, ProjectId, Workbench};
use genea_testkit::{FakeLsp, FixtureBuilder, FixtureProject, TestHost};
use serde_json::json;

/// The tsgo binary in a project with TypeScript 7 installed by pnpm.
const STORE: &str = "node_modules/.pnpm/typescript@7.0.2/node_modules";
const TSGO: &str = "node_modules/.pnpm/typescript@7.0.2/node_modules/@typescript/typescript-darwin-arm64/lib/tsc";

/// A project with TypeScript 7 installed.
fn typescript_project() -> FixtureBuilder {
    FixtureProject::new()
        .file("package.json", r#"{ "name": "app", "devDependencies": { "typescript": "^7.0.2" } }"#)
        .file("tsconfig.json", "{}")
        .file(format!("{STORE}/typescript/package.json"), r#"{ "name": "typescript", "version": "7.0.2" }"#)
        .file(TSGO, "#!/bin/sh\n")
}

struct Session {
    _fixture: FixtureProject,
    _host: TestHost,
    workbench: Workbench,
    project: ProjectId,
}

/// Opens a project whose `src/main.ts` holds `text`, with tsgo played by
/// `fake`, and opens that file.
fn open_main(text: &str, fake: &FakeLsp) -> Session {
    let fixture = typescript_project().file("src/main.ts", text).build();
    std::os::unix::fs::symlink(fixture.path(format!("{STORE}/typescript")), fixture.path("node_modules/typescript"))
        .unwrap();
    let host = TestHost::new();
    fake.install(&host, "tsc");
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    let mut session = Session { _fixture: fixture, _host: host, workbench, project };
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

    fn editor(&self) -> genea_core::EditorView {
        self.workbench.project(self.project).unwrap().editor.unwrap()
    }

    fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.dispatch(Command::InsertText(c.into()));
        }
    }

    /// The labels in the completion list, best first; empty while it is
    /// closed.
    fn completion_labels(&self) -> Vec<String> {
        self.editor().completion.map(|c| c.items.into_iter().map(|i| i.label).collect()).unwrap_or_default()
    }
}

#[test]
fn typing_a_word_lists_the_servers_completions_that_match_it() {
    let fake = FakeLsp::new()
        .completion(json!({ "label": "totalCount", "kind": 6, "sortText": "11" }))
        .completion(json!({ "label": "toString", "kind": 2, "sortText": "11", "labelDetails": { "detail": "()" } }))
        .completion(json!({ "label": "other", "kind": 6, "sortText": "11" }));
    let mut session = open_main("const a = 1;\n", &fake);

    session.dispatch(Command::PlaceCaret { line: 1, column: 0 });
    session.type_text("to");
    session.settle();

    let completion = session.editor().completion.expect("a completion list");
    assert_eq!(completion.at, Caret { line: 1, column: 0 }, "it opens where the word starts");
    assert_eq!(
        completion.items,
        [
            CompletionItem { label: "toString".into(), detail: Some("()".into()), source: None, kind: CompletionKind::Method },
            CompletionItem { label: "totalCount".into(), detail: None, source: None, kind: CompletionKind::Variable },
        ]
    );
    assert_eq!(completion.selected, 0);
}
