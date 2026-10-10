//! Navigation and rename's slow lane (ticket #44): go to definition, find
//! usages and rename against the real TypeScript 7 language server,
//! installed into a fixture project. It catches protocol drift (location
//! links, `prepareRename`, workspace edits) when TypeScript is bumped.
//!
//! Opt-in, because installing TypeScript needs the network (once; pnpm
//! caches it):
//!
//! ```sh
//! cargo test -p genea-core --test navigation_slow -- --ignored
//! ```

use std::{
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
    process::Command as Process,
    sync::Arc,
    time::{Duration, Instant},
};

use genea_core::{Caret, Command, LanguageServerState, ProjectId, ProjectView, RenamePrompt, Workbench};
use genea_host::{Clipboard, Clock, DownloadError, Downloads, Host, Processes, Ptys, RealHost};
use genea_testkit::FixtureProject;

const TYPESCRIPT: &str = "7.0.2";

/// How long tsgo may take to start.
const PATIENCE: Duration = Duration::from_secs(30);

/// The real host's clock and processes, without downloads or a login shell
/// (as in `language_server_slow.rs`).
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

const GREETING: &str = "export function greet(name: string): string {\n  return `Hello, ${name}`;\n}\n";
const MAIN: &str = "import { greet } from \"./greeting.ts\";\n\nconsole.log(greet(\"world\"));\n";
const OTHER: &str = "import { greet } from \"./greeting.ts\";\nexport const twice = greet(\"a\") + greet(\"b\");\n";

/// A fixture project with TypeScript 7 installed by pnpm.
fn installed() -> FixtureProject {
    let package_json =
        format!(r#"{{ "name": "slow-lane", "private": true, "devDependencies": {{ "typescript": "{TYPESCRIPT}" }} }}"#);
    let fixture = FixtureProject::new()
        .file("package.json", package_json)
        .file("tsconfig.json", r#"{ "compilerOptions": { "strict": true, "noEmit": true, "module": "nodenext", "allowImportingTsExtensions": true }, "include": ["src"] }"#)
        .file("src/greeting.ts", GREETING)
        .file("src/main.ts", MAIN)
        .file("src/other.ts", OTHER)
        .build();
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
    fixture: FixtureProject,
    workbench: Workbench,
    project: ProjectId,
}

impl Session {
    fn open(fixture: FixtureProject) -> Self {
        let host = SlowHost { real: RealHost::new(), support: tempfile::tempdir().unwrap() };
        let mut workbench = Workbench::new(Arc::new(host));
        let project = workbench.open_project(fixture.root()).unwrap();
        let mut session = Session { fixture, workbench, project };
        let deadline = Instant::now() + PATIENCE;
        loop {
            session.settle();
            let ready = session.view().status.language_servers.first().is_some_and(|s| s.state == LanguageServerState::Ready);
            if ready || Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        session
    }

    fn dispatch(&mut self, command: Command) {
        self.workbench.dispatch(self.project, command);
    }

    fn settle(&mut self) {
        self.workbench.settle().unwrap();
    }

    fn view(&self) -> ProjectView {
        self.workbench.project(self.project).unwrap()
    }

    fn focused(&self) -> (PathBuf, Caret) {
        let editor = self.view().editor.expect("a focused file");
        (editor.path, editor.caret)
    }
}

#[test]
#[ignore = "installs TypeScript 7 with pnpm (network); opt in with --ignored"]
fn real_tsgo_goes_to_definitions_finds_usages_and_renames_across_files() {
    let mut session = Session::open(installed());
    session.dispatch(Command::OpenFile("src/main.ts".into()));
    session.settle();
    // On `greet` in `console.log(greet("world"))`.
    session.dispatch(Command::PlaceCaret { line: 2, column: 14 });

    session.dispatch(Command::GoToDefinition);
    session.settle();
    assert_eq!(session.focused(), ("src/greeting.ts".into(), Caret { line: 0, column: 16 }), "{:?}", session.view().hint);

    session.dispatch(Command::FindUsages);
    session.settle();
    let view = session.view();
    let usages = view.usages.expect("the Usages view");
    let places: Vec<(PathBuf, String)> =
        usages.files.iter().flat_map(|f| f.matches.iter().map(|m| (f.path.clone(), m.location.clone()))).collect();
    for expected in [("src/main.ts", "3:13"), ("src/other.ts", "2:22"), ("src/other.ts", "2:35")] {
        assert!(places.contains(&(expected.0.into(), expected.1.into())), "{places:#?}");
    }

    session.dispatch(Command::StartRename);
    session.settle();
    assert_eq!(session.view().rename, Some(RenamePrompt { name: "greet".into() }), "{:?}", session.view().hint);
    session.dispatch(Command::Rename("welcome".into()));
    session.settle();

    let view = session.view();
    assert_eq!(view.hint, None);
    let tabs: Vec<(PathBuf, bool)> = view.panes[0].tabs.iter().map(|t| (t.path.clone(), t.modified)).collect();
    for path in ["src/main.ts", "src/greeting.ts", "src/other.ts"] {
        assert!(tabs.contains(&(path.into(), true)), "{path} is edited: {tabs:?}");
    }
    let tab = tabs.iter().position(|(path, _)| path == Path::new("src/other.ts")).unwrap();
    session.dispatch(Command::SelectTab { pane: 0, tab });
    session.dispatch(Command::Save);
    session.settle();
    assert_eq!(session.fixture.read("src/other.ts"), OTHER.replace("greet(", "welcome(").replace("{ greet }", "{ welcome }"));
}
