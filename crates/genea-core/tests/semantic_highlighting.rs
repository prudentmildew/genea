//! Semantic highlighting (ticket #46): tsgo's semantic tokens refine the
//! tree-sitter highlighting once they arrive, against the fake LSP server
//! (`genea_testkit::FakeLsp`), which the test host plays as the project's
//! `tsc`.

use genea_core::{CaretMove, Command, Highlight, ProjectId, Workbench};
use genea_testkit::{FakeLsp, FixtureProject, TestHost};

const STORE: &str = "node_modules/.pnpm/typescript@7.0.2/node_modules";
const TSGO: &str = "node_modules/.pnpm/typescript@7.0.2/node_modules/@typescript/typescript-darwin-arm64/lib/tsc";

struct Session {
    _fixture: FixtureProject,
    workbench: Workbench,
    project: ProjectId,
}

/// Opens a project with TypeScript 7 installed, tsgo played by `fake`,
/// and `src/main.ts` holding `text` open.
fn open(text: &str, fake: &FakeLsp) -> Session {
    let fixture = FixtureProject::new()
        .file("package.json", r#"{ "name": "app", "devDependencies": { "typescript": "^7.0.2" } }"#)
        .file("tsconfig.json", "{}")
        .file(format!("{STORE}/typescript/package.json"), r#"{ "name": "typescript", "version": "7.0.2" }"#)
        .file(TSGO, "#!/bin/sh\n")
        .file("src/main.ts", text)
        .build();
    std::os::unix::fs::symlink(fixture.path(format!("{STORE}/typescript")), fixture.path("node_modules/typescript"))
        .unwrap();
    let host = TestHost::new();
    fake.install(&host, "tsc");
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::SetViewport { rows: 20.0 });
    workbench.dispatch(project, Command::OpenFile("src/main.ts".into()));
    workbench.settle().unwrap();
    Session { _fixture: fixture, workbench, project }
}

impl Session {
    fn dispatch(&mut self, command: Command) {
        self.workbench.dispatch(self.project, command);
    }

    fn settle(&mut self) {
        self.workbench.settle().unwrap();
    }

    /// A visible line's highlight spans as (the text they cover, highlight).
    fn spans(&self, line: usize) -> Vec<(String, Highlight)> {
        let editor = self.workbench.project(self.project).unwrap().editor.unwrap();
        let line = editor.lines.iter().find(|l| l.index == line).expect("the line is visible");
        let chars: Vec<char> = line.text.chars().collect();
        line.highlights.iter().map(|s| (chars[s.columns.clone()].iter().collect(), s.highlight)).collect()
    }

    /// The highlight of the `nth` occurrence of `text` on a line, if it is
    /// one span.
    fn highlight_of(&self, line: usize, text: &str, nth: usize) -> Option<Highlight> {
        self.spans(line).into_iter().filter(|(t, _)| t == text).nth(nth).map(|(_, h)| h)
    }
}

#[test]
fn semantic_tokens_recolour_parameters_and_types_once_they_arrive() {
    let text = "function area(size: Size) { return size.width * size.height; }\ninterface Size { width: number; height: number }\n";
    // Tree-sitter alone sees a variable where the parameter is used.
    let plain = open(text, &FakeLsp::new());
    assert_eq!(plain.highlight_of(0, "size", 1), Some(Highlight::Variable));

    let fake = FakeLsp::new().token("size", "parameter", &[]).token("Size", "interface", &[]);
    let session = open(text, &fake);

    assert_eq!(session.highlight_of(0, "size", 0), Some(Highlight::Parameter));
    assert_eq!(session.highlight_of(0, "size", 1), Some(Highlight::Parameter));
    assert_eq!(session.highlight_of(0, "size", 2), Some(Highlight::Parameter));
    assert_eq!(session.highlight_of(0, "Size", 0), Some(Highlight::Type));
    assert_eq!(session.highlight_of(1, "Size", 0), Some(Highlight::Type));
}

#[test]
fn typing_doesnt_wait_for_fresh_tokens_the_last_ones_move_with_the_text() {
    let text = "function area(size: number) { return size; }\n";
    let fake = FakeLsp::new().token("size", "parameter", &[]);
    let mut session = open(text, &fake);
    let asked = fake.received("textDocument/semanticTokens/full").len();

    // Typed before `size`, with no settle: the server hasn't answered for
    // this text, and the tokens it sent for the last one have moved along.
    for _ in 0.."function area(".len() {
        session.dispatch(Command::MoveCaret(CaretMove::Right));
    }
    session.dispatch(Command::InsertText("count: number, ".into()));
    assert_eq!(session.highlight_of(0, "count", 0), None);
    assert_eq!(session.highlight_of(0, "size", 0), Some(Highlight::Parameter));
    assert_eq!(session.highlight_of(0, "size", 1), Some(Highlight::Parameter));

    // Fresh tokens for the new text replace them.
    session.settle();
    assert!(fake.received("textDocument/semanticTokens/full").len() > asked, "asked again after the edit");
    assert_eq!(session.highlight_of(0, "size", 1), Some(Highlight::Parameter));
}
