//! Completions, hover and signature help (ticket #43), against the fake LSP
//! server (`genea_testkit::FakeLsp`), which the test host plays whenever
//! Genea starts the project's `tsc`.

use std::time::{Duration, Instant};

use genea_core::{CaretMove, Caret, Command, CompletionItem, CompletionKind, MarkupBlock, ProjectId, Workbench};
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

#[test]
fn accepting_a_completion_replaces_the_typed_word() {
    let fake = FakeLsp::new()
        .completion(json!({ "label": "totalCount", "kind": 6 }))
        .completion(json!({ "label": "toString", "kind": 2 }));
    let mut session = open_main("const a = 1;\n", &fake);
    session.dispatch(Command::PlaceCaret { line: 1, column: 0 });
    session.type_text("tot");
    session.settle();
    assert_eq!(session.completion_labels(), ["totalCount", "toString"], "`toString` has the typed letters in order");

    session.dispatch(Command::AcceptCompletion);

    let editor = session.editor();
    assert_eq!(editor.lines[1].text, "totalCount");
    assert_eq!(editor.caret, Caret { line: 1, column: 10 });
    assert_eq!(editor.completion, None);
}

#[test]
fn typing_on_narrows_the_list_without_asking_the_server_again() {
    let fake = FakeLsp::new()
        .completion(json!({ "label": "totalCount", "kind": 6 }))
        .completion(json!({ "label": "toString", "kind": 2 }))
        .completion(json!({ "label": "other", "kind": 6 }));
    let mut session = open_main("const a = 1;\n", &fake);
    session.dispatch(Command::PlaceCaret { line: 1, column: 0 });
    session.type_text("t");
    session.settle();
    assert_eq!(session.completion_labels(), ["toString", "totalCount"]);
    let asked = fake.received("textDocument/completion").len();

    session.type_text("ota");
    session.settle();

    assert_eq!(session.completion_labels(), ["totalCount"]);
    assert_eq!(fake.received("textDocument/completion").len(), asked);
    session.dispatch(Command::Delete(CaretMove::Left));
    session.dispatch(Command::Delete(CaretMove::Left));
    assert_eq!(session.completion_labels(), ["toString", "totalCount"], "deleting widens it again");

    session.type_text(";");
    assert_eq!(session.editor().completion, None, "a character that can't be in a word closes it");
}

#[test]
fn a_trigger_character_lists_every_completion_and_tells_the_server_why() {
    let fake = FakeLsp::new()
        .completion(json!({ "label": "toFixed", "kind": 2 }))
        .completion(json!({ "label": "toString", "kind": 2 }));
    let mut session = open_main("const a = 1;\n", &fake);
    session.dispatch(Command::PlaceCaret { line: 1, column: 0 });
    session.type_text("a.");
    session.settle();

    assert_eq!(session.completion_labels(), ["toFixed", "toString"]);
    assert_eq!(session.editor().completion.unwrap().at, Caret { line: 1, column: 2 });
    let asked = fake.received("textDocument/completion");
    let last = asked.last().unwrap();
    assert_eq!(last["context"], json!({ "triggerKind": 2, "triggerCharacter": "." }));
    assert_eq!(last["position"], json!({ "line": 1, "character": 2 }));
}

/// An auto-import of `addNumbers` from `./helper`, as tsgo offers it: the
/// import edit comes only when the item is resolved.
fn auto_import_server() -> FakeLsp {
    FakeLsp::new()
        .completion(json!({
            "label": "addNumbers", "kind": 3, "sortText": "16",
            "labelDetails": { "description": "./helper" },
            "data": { "name": "addNumbers", "source": "./helper" },
        }))
        .resolve(
            "addNumbers",
            json!({
                "detail": "Add import from \"./helper\"",
                "documentation": { "kind": "markdown", "value": "Adds two numbers." },
                "additionalTextEdits": [{
                    "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
                    "newText": "import { addNumbers } from \"./helper\";\n",
                }],
            }),
        )
}

fn lines(session: &Session) -> Vec<String> {
    session.editor().lines.iter().map(|l| l.text.clone()).collect()
}

#[test]
fn accepting_an_auto_import_adds_its_import_in_the_same_undo_step() {
    let fake = auto_import_server();
    let mut session = open_main("const a = 1;\n", &fake);
    session.dispatch(Command::PlaceCaret { line: 1, column: 0 });
    session.type_text("addN");
    session.settle();
    let completion = session.editor().completion.expect("a completion list");
    assert_eq!(completion.items[0].source.as_deref(), Some("./helper"));
    assert_eq!(completion.detail.as_deref(), Some("Add import from \"./helper\""), "the selected item is resolved");
    assert_eq!(completion.documentation, [MarkupBlock::Text("Adds two numbers.".into())]);
    assert_eq!(fake.received("completionItem/resolve")[0]["data"], json!({ "name": "addNumbers", "source": "./helper" }));

    session.dispatch(Command::AcceptCompletion);

    assert_eq!(lines(&session), ["import { addNumbers } from \"./helper\";", "const a = 1;", "addNumbers"]);
    assert_eq!(session.editor().caret, Caret { line: 2, column: 10 });
    session.dispatch(Command::Undo);
    assert_eq!(lines(&session), ["const a = 1;", "addN"]);
}

#[test]
fn moving_the_selection_shows_the_newly_selected_items_details() {
    let fake = FakeLsp::new()
        .completion(json!({ "label": "toFixed", "kind": 2 }))
        .completion(json!({ "label": "toString", "kind": 2 }))
        .resolve("toFixed", json!({ "detail": "(method) toFixed(digits?: number): string" }))
        .resolve("toString", json!({ "detail": "(method) toString(): string" }));
    let mut session = open_main("const a = 1;\n", &fake);
    session.dispatch(Command::PlaceCaret { line: 1, column: 0 });
    session.type_text("a.");
    session.settle();
    assert_eq!(session.editor().completion.unwrap().detail.as_deref(), Some("(method) toFixed(digits?: number): string"));

    session.dispatch(Command::MoveCompletionSelection(1));
    session.settle();
    let completion = session.editor().completion.unwrap();
    assert_eq!(completion.selected, 1);
    assert_eq!(completion.detail.as_deref(), Some("(method) toString(): string"));

    session.dispatch(Command::MoveCompletionSelection(1));
    assert_eq!(session.editor().completion.unwrap().selected, 0, "it wraps around");
    session.dispatch(Command::SelectCompletionItem(1));
    session.dispatch(Command::AcceptCompletion);
    assert_eq!(session.editor().lines[1].text, "a.toString");
    assert_eq!(fake.received("completionItem/resolve").len(), 2, "each item is resolved once");
}

/// Pumps (without settling) until the focused editor satisfies `done`;
/// fails after 10 s.
fn pump_until(session: &mut Session, done: impl Fn(&genea_core::EditorView) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(&session.editor()) {
        assert!(Instant::now() < deadline, "never happened: {:#?}", session.editor());
        session.workbench.pump();
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn an_auto_import_accepted_before_it_is_resolved_gets_its_import_when_the_answer_comes() {
    let fake = auto_import_server().slow("completionItem/resolve", Duration::from_millis(300));
    let mut session = open_main("const a = 1;\n", &fake);
    session.dispatch(Command::PlaceCaret { line: 1, column: 0 });
    session.type_text("addN");
    pump_until(&mut session, |editor| editor.completion.is_some());

    session.dispatch(Command::AcceptCompletion);
    session.type_text("(");
    assert_eq!(session.editor().lines[1].text, "addNumbers(", "typing goes on at once");
    session.settle();

    assert_eq!(lines(&session), ["import { addNumbers } from \"./helper\";", "const a = 1;", "addNumbers("]);
    assert_eq!(session.editor().caret, Caret { line: 2, column: 11 }, "the caret stays after what was typed");
}

#[test]
fn a_slow_completion_never_holds_up_typing() {
    let fake = FakeLsp::new()
        .completion(json!({ "label": "totalCount", "kind": 6 }))
        .slow("textDocument/completion", Duration::from_millis(1000));
    let mut session = open_main("const a = 1;\n", &fake);
    session.dispatch(Command::PlaceCaret { line: 1, column: 0 });

    let started = Instant::now();
    session.type_text("to");
    assert!(started.elapsed() < Duration::from_millis(500), "typing waited {:?}", started.elapsed());
    assert_eq!(session.editor().lines[1].text, "to");
    assert_eq!(session.editor().completion, None, "the answer isn't in yet");

    session.settle();
    assert_eq!(session.completion_labels(), ["totalCount"]);
}

#[test]
fn a_completion_answer_for_text_that_has_moved_on_is_dropped() {
    let fake = FakeLsp::new()
        .completion(json!({ "label": "totalCount", "kind": 6 }))
        .slow("textDocument/completion", Duration::from_millis(200));
    let mut session = open_main("const a = 1;\n", &fake);
    session.dispatch(Command::PlaceCaret { line: 1, column: 0 });
    session.type_text("to");
    // Still in the word, where the list would narrow to `t`: but the
    // answer is for the caret after `to`.
    session.dispatch(Command::MoveCaret(CaretMove::Left));
    session.settle();

    assert_eq!(fake.received("textDocument/completion").len(), 2, "the server answered");
    assert_eq!(session.editor().completion, None);
}

const COUNT_HOVER: &str = "```typescript\nlet count: number\n```\nHow many there are.";

fn count_hover() -> Vec<MarkupBlock> {
    vec![MarkupBlock::Code("let count: number".into()), MarkupBlock::Text("How many there are.".into())]
}

#[test]
fn resting_the_pointer_on_code_shows_the_servers_hover_until_the_caret_moves() {
    let fake = FakeLsp::new().hover("count", COUNT_HOVER);
    let mut session = open_main("let count = 1;\ncount += 1;\n", &fake);

    session.dispatch(Command::HoverAt { line: 1, column: 3 });
    session.settle();

    let hover = session.editor().hover.expect("a hover");
    assert_eq!(hover.at, Caret { line: 1, column: 0 }, "it is anchored where the word starts");
    assert_eq!(hover.contents, count_hover());
    assert_eq!(fake.received("textDocument/hover")[0]["position"], json!({ "line": 1, "character": 3 }));

    session.dispatch(Command::MoveCaret(CaretMove::Down));
    assert_eq!(session.editor().hover, None);
}

#[test]
fn the_pointer_past_the_end_of_a_line_or_over_nothing_known_shows_no_hover() {
    let fake = FakeLsp::new().hover("count", COUNT_HOVER);
    let mut session = open_main("let count = 1;\n", &fake);
    session.dispatch(Command::HoverAt { line: 0, column: 5 });
    session.settle();
    assert!(session.editor().hover.is_some());

    session.dispatch(Command::HoverAt { line: 0, column: 40 });
    assert_eq!(session.editor().hover, None, "past the end of the line it closes at once");

    session.dispatch(Command::HoverAt { line: 0, column: 0 });
    session.settle();
    assert_eq!(session.editor().hover, None, "`let` has no hover");
    assert_eq!(fake.received("textDocument/hover").len(), 2);

    session.dispatch(Command::HoverAt { line: 0, column: 5 });
    session.settle();
    session.dispatch(Command::HideHover);
    assert_eq!(session.editor().hover, None);
}

#[test]
fn quick_documentation_shows_the_hover_at_the_caret() {
    let fake = FakeLsp::new().hover("count", COUNT_HOVER);
    let mut session = open_main("let count = 1;\n", &fake);
    session.dispatch(Command::PlaceCaret { line: 0, column: 9 });

    session.dispatch(Command::ShowHover);
    session.settle();

    let hover = session.editor().hover.expect("a hover: the caret is just after `count`");
    assert_eq!(hover.at, Caret { line: 0, column: 4 });
    assert_eq!(hover.contents, count_hover());
}

#[test]
fn a_hover_answer_after_typing_is_dropped() {
    let fake = FakeLsp::new().hover("count", COUNT_HOVER).slow("textDocument/hover", Duration::from_millis(200));
    let mut session = open_main("let count = 1;\n", &fake);
    session.dispatch(Command::HoverAt { line: 0, column: 5 });
    session.dispatch(Command::PlaceCaret { line: 1, column: 0 });
    session.type_text("x");
    session.settle();

    assert_eq!(fake.received("textDocument/hover").len(), 1);
    assert_eq!(session.editor().hover, None);
}

const ADD: &str = "add(first: number, second: number): number";

fn add_server() -> FakeLsp {
    FakeLsp::new().signature("add", ADD, &["first: number", "second: number"], "Adds two numbers.")
}

/// The signature help's label with its active parameter, as `add([first: number], …)`.
fn signature(session: &Session) -> Option<String> {
    session.editor().signature_help.map(|help| {
        let chars: Vec<char> = help.label.chars().collect();
        match help.active_parameter {
            Some(range) => format!(
                "{}[{}]{}",
                chars[..range.start].iter().collect::<String>(),
                chars[range.clone()].iter().collect::<String>(),
                chars[range.end..].iter().collect::<String>()
            ),
            None => help.label,
        }
    })
}

#[test]
fn typing_a_call_shows_its_signature_and_follows_the_argument() {
    let mut session = open_main("const a = 1;\n", &add_server());
    session.dispatch(Command::PlaceCaret { line: 1, column: 0 });

    session.type_text("add(");
    session.settle();
    assert_eq!(signature(&session).as_deref(), Some("add([first: number], second: number): number"));
    let help = session.editor().signature_help.unwrap();
    assert_eq!(help.at, Caret { line: 1, column: 4 });
    assert_eq!(help.documentation, [MarkupBlock::Text("Adds two numbers.".into())]);
    assert_eq!((help.signature, help.signatures), (0, 1));

    session.type_text("1, 2");
    session.settle();
    assert_eq!(signature(&session).as_deref(), Some("add(first: number, [second: number]): number"));

    session.type_text(")");
    session.settle();
    assert_eq!(signature(&session), None, "the caret left the call");
}

#[test]
fn typing_on_before_the_signature_arrives_asks_again() {
    let fake = add_server().slow("textDocument/signatureHelp", Duration::from_millis(100));
    let mut session = open_main("const a = 1;\n", &fake);
    session.dispatch(Command::PlaceCaret { line: 1, column: 0 });

    session.type_text("add(1, 2");
    session.settle();

    assert_eq!(signature(&session).as_deref(), Some("add(first: number, [second: number]): number"));
}

#[test]
fn parameter_info_shows_the_signature_of_the_call_at_the_caret() {
    let mut session = open_main("add(1, 2);\n", &add_server());
    session.dispatch(Command::PlaceCaret { line: 0, column: 7 });

    session.dispatch(Command::ShowSignatureHelp);
    session.settle();
    assert_eq!(signature(&session).as_deref(), Some("add(first: number, [second: number]): number"));

    session.dispatch(Command::HideSignatureHelp);
    assert_eq!(signature(&session), None);
}

#[test]
fn signature_help_closed_before_its_answer_comes_stays_closed() {
    let fake = add_server().slow("textDocument/signatureHelp", Duration::from_millis(200));
    let mut session = open_main("add(1, 2);\n", &fake);
    session.dispatch(Command::PlaceCaret { line: 0, column: 7 });
    session.dispatch(Command::ShowSignatureHelp);
    session.dispatch(Command::HideSignatureHelp);
    session.settle();

    assert_eq!(fake.received("textDocument/signatureHelp").len(), 1);
    assert_eq!(signature(&session), None);
}
