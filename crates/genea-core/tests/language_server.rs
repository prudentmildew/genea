//! tsgo and live diagnostics (ticket #42), against the fake LSP server
//! (`genea_testkit::FakeLsp`), which the test host plays whenever Genea
//! starts the project's `tsc`.

use std::{path::PathBuf, time::Duration};

use genea_core::{
    CaretMove, Command, InlineProblem, LARGE_FILE_BYTES, LanguageServerState, LanguageServerStatus, ProblemSource,
    MAX_RESTARTS, ProcessSpec, ProjectId, RESTART_DELAY, RESTART_WINDOW, START_TIMEOUT, Severity, Workbench,
};
use genea_testkit::{FakeLsp, FixtureBuilder, FixtureProject, TestHost};
use serde_json::json;

/// The tsgo binary in a project with TypeScript 7 installed by pnpm: the
/// platform package sits beside `typescript` in the virtual store.
const STORE: &str = "node_modules/.pnpm/typescript@7.0.2/node_modules";
const TSGO: &str = "node_modules/.pnpm/typescript@7.0.2/node_modules/@typescript/typescript-darwin-arm64/lib/tsc";

const TYPE_ERROR: &str = "Type 'string' is not assignable to type 'number'.";

/// A project with TypeScript 7 installed (as pnpm lays it out, with
/// `node_modules/typescript` a symlink into the store).
fn typescript_project() -> FixtureBuilder {
    FixtureProject::new()
        .file("package.json", r#"{ "name": "app", "devDependencies": { "typescript": "^7.0.2" } }"#)
        .file("tsconfig.json", "{}")
        .file(format!("{STORE}/typescript/package.json"), r#"{ "name": "typescript", "version": "7.0.2" }"#)
        .file(TSGO, "#!/bin/sh\n")
}

fn link_typescript(fixture: &FixtureProject) {
    std::os::unix::fs::symlink(fixture.path(format!("{STORE}/typescript")), fixture.path("node_modules/typescript"))
        .unwrap();
}

struct Session {
    fixture: FixtureProject,
    host: TestHost,
    workbench: Workbench,
    project: ProjectId,
}

/// Opens `fixture` with tsgo played by `fake`.
fn open(fixture: FixtureProject, fake: &FakeLsp) -> Session {
    link_typescript(&fixture);
    let host = TestHost::new();
    fake.install(&host, "tsc");
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    Session { fixture, host, workbench, project }
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

    /// TypeScript's problems: (path, location, severity, message).
    fn typescript_problems(&self) -> Vec<(PathBuf, String, Severity, String)> {
        self.view()
            .problems
            .into_iter()
            .filter(|p| p.source == ProblemSource::TypeScript)
            .map(|p| (p.path, p.location, p.severity, p.message))
            .collect()
    }
}

#[test]
fn opening_a_file_with_a_type_error_shows_it_inline_and_in_problems() {
    let fixture = typescript_project().file("src/main.ts", "let a: number = \"oops\";\n").build();
    let fake = FakeLsp::new().error("\"oops\"", TYPE_ERROR);
    let mut session = open(fixture, &fake);

    session.dispatch(Command::OpenFile("src/main.ts".into()));
    session.settle();

    assert_eq!(
        session.typescript_problems(),
        [("src/main.ts".into(), "1:17".into(), Severity::Error, TYPE_ERROR.into())]
    );
    let editor = session.view().editor.unwrap();
    assert_eq!(
        editor.problems,
        [InlineProblem { line: 0, columns: 16..22, severity: Severity::Error, message: TYPE_ERROR.into() }]
    );
}

#[test]
fn tsgo_runs_from_node_modules_in_the_project_root_as_its_only_workspace_folder() {
    let fake = FakeLsp::new();
    let mut session = open(typescript_project().build(), &fake);
    session.settle();

    let root = session.fixture.root().to_owned();
    let spawned: Vec<ProcessSpec> =
        session.host.processes().spawned().into_iter().filter(|spec| spec.program_name() == "tsc").collect();
    assert_eq!(spawned.len(), 1, "one tsgo per project");
    assert_eq!(spawned[0].program, root.join(TSGO));
    assert_eq!(spawned[0].args, ["--lsp", "--stdio"]);
    assert_eq!(spawned[0].cwd.as_deref(), Some(root.as_path()));
    assert!(spawned[0].clear_env, "it gets the project environment");

    let initialize = &fake.received("initialize")[0];
    let name = root.file_name().unwrap().to_str().unwrap();
    assert_eq!(initialize["workspaceFolders"], json!([{ "uri": format!("file://{}", root.display()), "name": name }]));
    assert_eq!(initialize["processId"], std::process::id());
    let watching = &initialize["capabilities"]["workspace"]["didChangeWatchedFiles"]["dynamicRegistration"];
    assert_eq!(watching, true, "Genea's watcher feeds the server, so it runs none of its own");
    assert_eq!(session.view().status.language_servers, [ready("TypeScript 7.0.0-fake")]);
}

fn status(state: LanguageServerState, label: &str) -> LanguageServerStatus {
    LanguageServerStatus { name: "TypeScript".into(), state, label: label.into() }
}

fn ready(label: &str) -> LanguageServerStatus {
    status(LanguageServerState::Ready, label)
}

#[test]
fn edits_reach_the_server_and_update_the_diagnostics() {
    let fixture = typescript_project().file("src/main.ts", "let a = 1;\n").build();
    let fake = FakeLsp::new().error("\"oops\"", TYPE_ERROR);
    let mut session = open(fixture, &fake);
    session.dispatch(Command::OpenFile("src/main.ts".into()));
    session.settle();
    assert_eq!(session.typescript_problems(), []);

    session.dispatch(Command::MoveCaret(CaretMove::LineEnd));
    for c in [" ", "\"", "o", "o", "p", "s", "\""] {
        session.dispatch(Command::InsertText(c.into()));
    }
    session.settle();
    assert_eq!(
        session.typescript_problems(),
        [("src/main.ts".into(), "1:12".into(), Severity::Error, TYPE_ERROR.into())]
    );

    session.dispatch(Command::Undo);
    session.settle();
    assert_eq!(session.typescript_problems(), [], "undo is a change too");
    let versions: Vec<i64> = fake
        .received("textDocument/didChange")
        .iter()
        .map(|change| change["textDocument"]["version"].as_i64().unwrap())
        .collect();
    assert!(versions.windows(2).all(|pair| pair[0] < pair[1]), "versions only go up: {versions:?}");
}

#[test]
fn diagnostics_count_columns_in_the_encoding_the_server_chose() {
    // The fake takes UTF-8, which Genea offers first; `é` and `😀` are
    // wider in it than in chars.
    let fixture = typescript_project().file("src/main.ts", "let s = \"é😀\"; oops\n").build();
    let fake = FakeLsp::new().warning("oops", "Unused.");
    let mut session = open(fixture, &fake);
    session.dispatch(Command::OpenFile("src/main.ts".into()));
    session.settle();

    assert_eq!(
        session.typescript_problems(),
        [("src/main.ts".into(), "1:15".into(), Severity::Warning, "Unused.".into())]
    );
}

#[test]
fn information_and_hints_stay_out_of_problems() {
    let fixture = typescript_project().file("src/main.ts", "info hint\n").build();
    let fake = FakeLsp::new().marker("info", 3, "Info.", None).marker("hint", 4, "Hint.", None);
    let mut session = open(fixture, &fake);
    session.dispatch(Command::OpenFile("src/main.ts".into()));
    session.settle();

    assert_eq!(fake.received("textDocument/diagnostic").len(), 1);
    assert_eq!(session.typescript_problems(), []);
}

#[test]
fn closing_a_file_closes_it_on_the_server_and_drops_its_diagnostics() {
    let fixture = typescript_project().file("src/main.ts", "oops\n").build();
    let fake = FakeLsp::new().error("oops", TYPE_ERROR);
    let mut session = open(fixture, &fake);
    session.dispatch(Command::OpenFile("src/main.ts".into()));
    session.settle();
    assert_eq!(session.typescript_problems().len(), 1);

    session.dispatch(Command::CloseTab { pane: 0, tab: 0 });
    session.settle();

    assert_eq!(session.typescript_problems(), []);
    let closed = fake.wait_for("textDocument/didClose", 1);
    let uri = format!("file://{}", session.fixture.path("src/main.ts").display());
    assert_eq!(closed[0]["textDocument"]["uri"], uri);
}

#[test]
fn only_first_class_language_files_under_the_large_file_size_are_synced() {
    let large = format!("// {}\n", "x".repeat(LARGE_FILE_BYTES));
    let fixture = typescript_project()
        .file("src/small.tsx", "oops\n")
        .file("src/big.ts", &large)
        .file("README.md", "oops\n")
        .file("src/data.json", "{}\n")
        .build();
    let fake = FakeLsp::new().error("oops", TYPE_ERROR);
    let mut session = open(fixture, &fake);
    for file in ["src/big.ts", "README.md", "src/data.json", "src/small.tsx"] {
        session.dispatch(Command::OpenFile(file.into()));
        session.settle();
    }

    let opened: Vec<(String, String)> = fake
        .received("textDocument/didOpen")
        .iter()
        .map(|open| {
            let document = &open["textDocument"];
            let uri = document["uri"].as_str().unwrap();
            (uri.rsplit('/').next().unwrap().to_owned(), document["languageId"].as_str().unwrap().to_owned())
        })
        .collect();
    assert_eq!(opened, [("small.tsx".to_owned(), "typescriptreact".to_owned())]);
    assert_eq!(session.typescript_problems().len(), 1);
}

#[test]
fn closing_the_project_shuts_the_server_down() {
    let fake = FakeLsp::new();
    let mut session = open(typescript_project().build(), &fake);
    session.settle();

    session.workbench.close_project(session.project);
    session.workbench.settle().unwrap();

    assert_eq!(fake.wait_for("shutdown", 1).len(), 1);
    assert_eq!(fake.wait_for("exit", 1).len(), 1);
}

// --- Crashes -------------------------------------------------------------------

impl Session {
    fn language_server(&self) -> LanguageServerStatus {
        self.view().status.language_servers.into_iter().next().expect("a language server item")
    }

    /// Moves the host clock on, then settles.
    fn advance(&mut self, by: Duration) {
        self.host.clock().advance(by);
        self.settle();
    }
}

#[test]
fn a_crashing_server_is_restarted_three_times_within_five_minutes_then_marked_failed() {
    let fixture = typescript_project().file("src/main.ts", "oops\n").build();
    let fake = FakeLsp::new().error("oops", TYPE_ERROR).crash_on("textDocument/diagnostic");
    let mut session = open(fixture, &fake);
    session.dispatch(Command::OpenFile("src/main.ts".into()));
    session.settle();
    assert_eq!(fake.starts(), 1);
    assert_eq!(session.language_server(), status(LanguageServerState::Restarting, "TypeScript restarting…"));

    for restart in 1..=MAX_RESTARTS {
        session.advance(RESTART_DELAY);
        assert_eq!(fake.starts(), 1 + restart, "restart {restart}");
    }
    assert_eq!(session.language_server(), status(LanguageServerState::Failed, "TypeScript stopped"));
    let notice = session.view().notices.into_iter().find(|n| n.message.contains("crashing")).expect("a notice");
    assert!(notice.message.contains("exited with code 1"), "{}", notice.message);
    session.advance(RESTART_WINDOW);
    assert_eq!(fake.starts(), 1 + MAX_RESTARTS, "a failed server stays stopped");

    // Fixed, it comes back with "Restart language server", the notice's action.
    fake.set_crash_on(None);
    session.dispatch(notice.action.unwrap().command);
    session.settle();
    assert_eq!(fake.starts(), 2 + MAX_RESTARTS);
    assert_eq!(session.language_server(), ready("TypeScript 7.0.0-fake"));
    assert_eq!(session.typescript_problems().len(), 1, "the open file is synced again");
    assert!(session.view().notices.iter().all(|n| !n.message.contains("crashing")));
}

#[test]
fn crashes_more_than_five_minutes_apart_keep_being_restarted() {
    let fake = FakeLsp::new().crash_on("initialize");
    let mut session = open(typescript_project().build(), &fake);
    session.settle();

    for restart in 1..=2 * MAX_RESTARTS {
        session.advance(RESTART_WINDOW / 2);
        assert_eq!(fake.starts(), 1 + restart);
    }
    assert_eq!(session.language_server().state, LanguageServerState::Restarting);
}

#[test]
fn a_crash_drops_the_servers_diagnostics_until_it_is_back() {
    let fixture = typescript_project().file("src/main.ts", "oops\n").build();
    let fake = FakeLsp::new().error("oops", TYPE_ERROR);
    let mut session = open(fixture, &fake);
    session.dispatch(Command::OpenFile("src/main.ts".into()));
    session.settle();
    assert_eq!(session.typescript_problems().len(), 1);

    fake.set_crash_on(Some("textDocument/diagnostic"));
    session.dispatch(Command::InsertText(" ".into()));
    session.settle();
    assert_eq!(session.typescript_problems(), [], "no stale diagnostics from a dead server");

    fake.set_crash_on(None);
    session.advance(RESTART_DELAY);
    assert_eq!(session.typescript_problems().len(), 1);
}

#[test]
fn restart_language_server_starts_a_fresh_process() {
    let fake = FakeLsp::new();
    let mut session = open(typescript_project().build(), &fake);
    session.settle();

    session.dispatch(Command::RestartLanguageServer);
    session.settle();

    assert_eq!(fake.starts(), 2);
    assert_eq!(fake.wait_for("exit", 1).len(), 1, "the old one was shut down");
    assert_eq!(session.language_server(), ready("TypeScript 7.0.0-fake"));
}

// --- A slow, silent or noisy server ------------------------------------------------

#[test]
fn a_server_that_never_answers_doesnt_hold_up_typing() {
    let fixture = typescript_project().file("src/main.ts", "let a = 1;\n").build();
    let fake = FakeLsp::new().silent();
    let mut session = open(fixture, &fake);
    // Nothing waits for it, and after a while the status bar says so.
    session.host.clock().advance(START_TIMEOUT);
    session.dispatch(Command::OpenFile("src/main.ts".into()));
    session.settle();
    assert_eq!(session.language_server(), status(LanguageServerState::NotResponding, "TypeScript isn't responding"));

    for _ in 0..200 {
        session.dispatch(Command::InsertText("x".into()));
    }
    let editor = session.view().editor.unwrap();
    assert_eq!(editor.lines[0].text, format!("{}let a = 1;", "x".repeat(200)));
    assert_eq!(fake.starts(), 1, "it isn't killed: it may still answer");
}

#[test]
fn a_flood_of_server_messages_doesnt_hold_up_typing() {
    let fixture = typescript_project().file("src/main.ts", "oops\n").build();
    let fake = FakeLsp::new().error("oops", TYPE_ERROR).flood(100_000);
    let mut session = open(fixture, &fake);
    session.dispatch(Command::OpenFile("src/main.ts".into()));
    session.settle();
    for _ in 0..100 {
        session.dispatch(Command::InsertText("x".into()));
    }
    assert_eq!(session.view().editor.unwrap().lines[0].text, format!("{}oops", "x".repeat(100)));
    session.settle();

    let editor = session.view().editor.unwrap();
    assert_eq!(editor.lines[0].text, format!("{}oops", "x".repeat(100)));
    assert_eq!(
        session.typescript_problems(),
        [("src/main.ts".into(), "1:101".into(), Severity::Error, TYPE_ERROR.into())]
    );
}

#[test]
fn a_slow_server_gets_only_the_latest_text_and_its_diagnostics_win() {
    let fixture = typescript_project().file("src/main.ts", "\n").build();
    let fake = FakeLsp::new().error("oops", TYPE_ERROR).delay(Duration::from_millis(20));
    let mut session = open(fixture, &fake);
    session.dispatch(Command::OpenFile("src/main.ts".into()));
    session.settle();

    for c in "oops".chars() {
        session.dispatch(Command::InsertText(c.into()));
        session.workbench.pump();
    }
    session.settle();

    assert_eq!(
        session.typescript_problems(),
        [("src/main.ts".into(), "1:1".into(), Severity::Error, TYPE_ERROR.into())]
    );
    let pulls = fake.received("textDocument/diagnostic").len();
    assert!(pulls <= 5, "one pull in flight per file, not one per keystroke: {pulls}");
}
