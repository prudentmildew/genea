//! Inlay hints and code lenses (ticket #46): shown while the `inlayHints`
//! and `codeLens` config keys are on (both are off by default), live as the
//! config changes. tsgo is played by the fake LSP server
//! (`genea_testkit::FakeLsp`), which, like tsgo, serves them only if
//! Genea's `workspace/configuration` answer turns them on.

use genea_core::{Command, Highlight, ProjectId, Workbench};
use genea_testkit::{FakeLsp, FixtureBuilder, FixtureProject, TestHost};

const STORE: &str = "node_modules/.pnpm/typescript@7.0.2/node_modules";
const TSGO: &str = "node_modules/.pnpm/typescript@7.0.2/node_modules/@typescript/typescript-darwin-arm64/lib/tsc";

struct Session {
    fixture: FixtureProject,
    workbench: Workbench,
    project: ProjectId,
}

/// A project with TypeScript 7 installed and `src/main.ts` holding `text`.
fn project(text: &str) -> FixtureBuilder {
    FixtureProject::new()
        .file("package.json", r#"{ "name": "app", "devDependencies": { "typescript": "^7.0.2" } }"#)
        .file("tsconfig.json", "{}")
        .file(format!("{STORE}/typescript/package.json"), r#"{ "name": "typescript", "version": "7.0.2" }"#)
        .file(TSGO, "#!/bin/sh\n")
        .file("src/main.ts", text)
}

/// Opens `fixture` with tsgo played by `fake`, and `src/main.ts` in it.
fn open(fixture: FixtureBuilder, fake: &FakeLsp) -> Session {
    let fixture = fixture.build();
    std::os::unix::fs::symlink(fixture.path(format!("{STORE}/typescript")), fixture.path("node_modules/typescript"))
        .unwrap();
    let host = TestHost::new();
    fake.install(&host, "tsc");
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::SetViewport { rows: 20.0 });
    workbench.dispatch(project, Command::OpenFile("src/main.ts".into()));
    workbench.settle().unwrap();
    Session { fixture, workbench, project }
}

impl Session {
    fn dispatch(&mut self, command: Command) {
        self.workbench.dispatch(self.project, command);
    }

    fn settle(&mut self) {
        self.workbench.settle().unwrap();
    }

    /// Changes `genea.jsonc` on disk and waits for it to apply.
    fn config(&mut self, text: &str) {
        self.fixture.write("genea.jsonc", text);
        self.settle();
    }

    fn line(&self, index: usize) -> genea_core::VisibleLine {
        let editor = self.workbench.project(self.project).unwrap().editor.unwrap();
        editor.lines.into_iter().find(|l| l.index == index).expect("the line is visible")
    }

    /// The primary caret's cell as drawn: (line, grid column).
    fn caret(&self) -> (usize, usize) {
        let caret = self.workbench.project(self.project).unwrap().editor.unwrap().caret;
        (caret.line, caret.column)
    }

    /// The status bar's caret position.
    fn status_caret(&self) -> String {
        self.workbench.project(self.project).unwrap().status.caret.unwrap()
    }

    /// The text of a visible line as drawn.
    fn text(&self, index: usize) -> String {
        self.line(index).text
    }

    /// The stretches of a visible line drawn as hints.
    fn hints(&self, index: usize) -> Vec<String> {
        let line = self.line(index);
        let chars: Vec<char> = line.text.chars().collect();
        line.highlights
            .iter()
            .filter(|s| s.highlight == Highlight::Hint)
            .map(|s| chars[s.columns.clone()].iter().collect())
            .collect()
    }
}

#[test]
fn inlay_hints_are_off_by_default() {
    let fake = FakeLsp::new().type_hint("total", ": number");
    let session = open(project("const total = 1 + 2;\n"), &fake);

    assert_eq!(session.text(0), "const total = 1 + 2;");
    assert_eq!(fake.received("textDocument/inlayHint"), Vec::<serde_json::Value>::new());
}

#[test]
fn turning_inlay_hints_on_and_off_shows_and_hides_them_live() {
    let fake = FakeLsp::new().type_hint("total", ": number").parameter_hint("2)", "count:");
    let mut session = open(project("const total = twice(2);\n"), &fake);

    session.config(r#"{ "inlayHints": true }"#);
    assert_eq!(session.text(0), "const total: number = twice(count: 2);");
    assert_eq!(session.hints(0), [": number", "count: "]);

    session.config(r#"{ "inlayHints": false }"#);
    assert_eq!(session.text(0), "const total = twice(2);");
    assert_eq!(session.hints(0), Vec::<String>::new());
}

#[test]
fn hints_push_the_text_along_but_carets_and_clicks_stay_on_the_files_text() {
    let fake = FakeLsp::new().type_hint("total", ": number");
    let mut session = open(project("const total = 1 + 2;\n").file("genea.jsonc", r#"{ "inlayHints": true }"#), &fake);
    assert_eq!(session.text(0), "const total: number = 1 + 2;");

    // A click on the `=` puts the caret before it in the file: the status
    // bar counts the file's columns, the view the drawn ones.
    session.dispatch(Command::PlaceCaret { line: 0, column: 20 });
    assert_eq!(session.caret(), (0, 20));
    assert_eq!(session.status_caret(), "1:13");

    // A click in the hint puts the caret where the hint is, before it.
    session.dispatch(Command::PlaceCaret { line: 0, column: 15 });
    assert_eq!(session.caret(), (0, 11));
    assert_eq!(session.status_caret(), "1:12");

    // Typing there types onto the name, and its hint moves along at once.
    session.dispatch(Command::InsertText("s".into()));
    assert_eq!(session.text(0), "const totals: number = 1 + 2;");
    assert_eq!(session.caret(), (0, 12));
}
