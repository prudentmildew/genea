//! Completion, hover and signature help against the real tsgo (ticket #43;
//! spec #19, Testing Decisions): catches protocol drift when TypeScript is
//! bumped. Opt-in, like `language_server_slow.rs`, because installing
//! TypeScript needs the network once:
//!
//! ```sh
//! cargo test -p genea-core --test language_assist_slow -- --ignored
//! ```

use std::{
    ffi::OsString,
    io::Write,
    path::Path,
    process::Command as Process,
    sync::Arc,
    time::{Duration, Instant},
};

use genea_core::{Command, EditorView, LanguageServerState, MarkupBlock, ProjectId, Workbench};
use genea_host::{Clipboard, Clock, DownloadError, Downloads, Host, Processes, Ptys, RealHost};
use genea_testkit::FixtureProject;

/// The TypeScript the fixture installs: the templates' version.
const TYPESCRIPT: &str = "7.0.2";

/// How long tsgo may take to answer.
const PATIENCE: Duration = Duration::from_secs(30);

/// The real host's clock and processes, with no `SHELL` (tsgo gets Genea's
/// own environment) and no downloads.
struct SlowHost {
    real: RealHost,
    support: tempfile::TempDir,
}

struct NoDownloads;

impl Downloads for NoDownloads {
    fn fetch(&self, _url: &str, _sink: &mut dyn Write) -> Result<u64, DownloadError> {
        Err(DownloadError::Transport("the slow lane downloads nothing".into()))
    }
}

impl Host for SlowHost {
    fn clock(&self) -> &dyn Clock {
        self.real.clock()
    }

    fn processes(&self) -> &dyn Processes {
        self.real.processes()
    }

    fn ptys(&self) -> &dyn Ptys {
        self.real.ptys()
    }

    fn downloads(&self) -> &dyn Downloads {
        &NoDownloads
    }

    fn clipboard(&self) -> &dyn Clipboard {
        self.real.clipboard()
    }

    fn support_dir(&self) -> &Path {
        self.support.path()
    }

    fn launch_environment(&self) -> Vec<(OsString, OsString)> {
        self.real.launch_environment().into_iter().filter(|(key, _)| key != "SHELL").collect()
    }
}

/// A fixture project with TypeScript 7 installed by pnpm.
fn installed(files: &[(&str, &str)]) -> FixtureProject {
    let package_json =
        format!(r#"{{ "name": "slow-lane", "private": true, "devDependencies": {{ "typescript": "{TYPESCRIPT}" }} }}"#);
    let mut fixture = FixtureProject::new().file("package.json", package_json);
    for (path, text) in files {
        fixture = fixture.file(path, text);
    }
    let fixture = fixture.build();
    let output = Process::new("pnpm")
        .args(["install", "--config.confirmModulesPurge=false"])
        .current_dir(fixture.root())
        .env("CI", "1")
        .output()
        .expect("run pnpm (the slow lane needs it on PATH)");
    assert!(output.status.success(), "pnpm install failed:\n{}", String::from_utf8_lossy(&output.stderr));
    fixture
}

struct Session {
    _fixture: FixtureProject,
    workbench: Workbench,
    project: ProjectId,
}

impl Session {
    fn dispatch(&mut self, command: Command) {
        self.workbench.dispatch(self.project, command);
    }

    fn editor(&self) -> EditorView {
        self.workbench.project(self.project).unwrap().editor.unwrap()
    }

    /// Settles until the editor satisfies `done`, dispatching `ask` before
    /// each try (tsgo may still be loading the project), for up to
    /// [`PATIENCE`].
    fn eventually(&mut self, ask: Option<Command>, done: impl Fn(&EditorView) -> bool) -> EditorView {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Some(command) = &ask {
                self.dispatch(command.clone());
            }
            self.workbench.settle().unwrap();
            let editor = self.editor();
            if done(&editor) {
                return editor;
            }
            assert!(Instant::now() < deadline, "tsgo never got there: {editor:#?}");
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

#[test]
#[ignore = "installs TypeScript 7 with pnpm (network); opt in with --ignored"]
fn real_tsgo_completes_with_an_auto_import_and_shows_hover_and_signature_help() {
    let fixture = installed(&[
        ("tsconfig.json", r#"{ "compilerOptions": { "strict": true, "noEmit": true, "module": "nodenext" }, "include": ["src"] }"#),
        (
            "src/helper.ts",
            "/** Adds two numbers. */\nexport function addNumbers(first: number, second: number): number {\n  return first + second;\n}\n",
        ),
        ("src/main.ts", "const count = 1;\n"),
    ]);
    let host = SlowHost { real: RealHost::new(), support: tempfile::tempdir().unwrap() };
    let mut workbench = Workbench::new(Arc::new(host));
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    let mut session = Session { _fixture: fixture, workbench, project };
    let server = session.workbench.project(project).unwrap().status.language_servers.first().cloned();
    assert_eq!(server.map(|s| s.state), Some(LanguageServerState::Ready));
    session.dispatch(Command::OpenFile("src/main.ts".into()));
    session.workbench.settle().unwrap();
    session.dispatch(Command::PlaceCaret { line: 1, column: 0 });
    session.dispatch(Command::InsertText("addNum".into()));

    // tsgo offers `addNumbers` from `./helper` as an auto-import.
    let editor = session.eventually(Some(Command::ShowCompletion), |editor| {
        editor.completion.as_ref().is_some_and(|c| c.items.iter().any(|i| i.label == "addNumbers"))
    });
    let completion = editor.completion.unwrap();
    let index = completion.items.iter().position(|i| i.label == "addNumbers").unwrap();
    assert!(completion.items[index].source.as_deref().is_some_and(|s| s.contains("helper")), "{completion:#?}");
    session.dispatch(Command::SelectCompletionItem(index));
    let editor = session.eventually(None, |editor| editor.completion.as_ref().is_some_and(|c| c.detail.is_some()));
    let detail = editor.completion.unwrap().detail.unwrap();
    assert!(detail.contains("import"), "{detail}");

    // Accepting it adds the import.
    session.dispatch(Command::AcceptCompletion);
    let lines: Vec<String> = session.editor().lines.iter().map(|l| l.text.clone()).collect();
    assert!(lines[0].starts_with("import { addNumbers } from \"./helper"), "{lines:#?}");
    let call = lines.iter().position(|l| l == "addNumbers").expect("the completed line");

    // `(` opens signature help on the first parameter.
    session.dispatch(Command::InsertText("(".into()));
    let editor = session.eventually(None, |editor| editor.signature_help.is_some());
    let help = editor.signature_help.unwrap();
    assert!(help.label.contains("addNumbers(first: number, second: number): number"), "{help:#?}");
    let active: String = help.label.chars().skip(help.active_parameter.clone().unwrap().start).take(13).collect();
    assert_eq!(active, "first: number");

    // Hovering over the call shows its signature and documentation.
    let editor = session.eventually(Some(Command::HoverAt { line: call, column: 3 }), |editor| editor.hover.is_some());
    let contents = editor.hover.unwrap().contents;
    assert!(
        matches!(&contents[0], MarkupBlock::Code(code) if code.contains("addNumbers(first: number, second: number): number")),
        "{contents:#?}"
    );
    assert!(contents.contains(&MarkupBlock::Text("Adds two numbers.".into())), "{contents:#?}");
}
