//! Symbols' slow lane (ticket #47; spec #19, Testing Decisions): the real
//! TypeScript 7 language server, installed into a fixture project, answers
//! File Structure (⌘F12) and Go to Symbol (⌥⌘O). It catches protocol drift
//! in `documentSymbol` and `workspace/symbol` when TypeScript is bumped.
//!
//! Opt-in, because installing TypeScript needs the network (once; pnpm
//! caches it):
//!
//! ```sh
//! cargo test -p genea-core --test symbols_slow -- --ignored
//! ```

use std::{
    ffi::OsString,
    io::Write,
    path::Path,
    process::Command as Process,
    sync::Arc,
    time::{Duration, Instant},
};

use genea_core::{Caret, Command, FinderMode, LanguageServerState, ProjectId, Workbench};
use genea_host::{Clipboard, Clock, DownloadError, Downloads, Host, Processes, Ptys, RealHost};
use genea_testkit::FixtureProject;

/// The TypeScript the fixture installs: the templates' version.
const TYPESCRIPT: &str = "7.0.2";

/// How long tsgo may take to answer.
const PATIENCE: Duration = Duration::from_secs(30);

/// The real host's clock and processes, with its own support folder, no
/// `SHELL` (no login shell runs) and no downloads.
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
        self.workbench.settle().unwrap();
    }

    /// The finder's results as the user reads them: `label  detail`.
    fn results(&self) -> Vec<String> {
        let finder = self.workbench.project(self.project).unwrap().finder.expect("the finder is open");
        let items = finder.items.iter();
        items.map(|item| if item.detail.is_empty() { item.label.clone() } else { format!("{}  {}", item.label, item.detail) }).collect()
    }

    /// Settles until the finder has results, for up to [`PATIENCE`].
    fn eventual_results(&mut self) -> Vec<String> {
        let deadline = Instant::now() + PATIENCE;
        loop {
            self.workbench.settle().unwrap();
            let results = self.results();
            if !results.is_empty() || Instant::now() > deadline {
                return results;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn caret(&self) -> (String, Caret) {
        let editor = self.workbench.project(self.project).unwrap().editor.expect("a file is open");
        (editor.path.to_string_lossy().into_owned(), editor.caret)
    }
}

const SHAPES: &str = "\
export interface Shape {
  area(): number;
}

export class Circle implements Shape {
  radius = 1;
  area() {
    return Math.PI * this.radius ** 2;
  }
}

export function circle(radius: number): Circle {
  return new Circle();
}
";

#[test]
#[ignore = "installs TypeScript 7 with pnpm (network); opt in with --ignored"]
fn real_tsgo_lists_file_and_project_symbols() {
    let fixture = installed(&[
        ("tsconfig.json", r#"{ "compilerOptions": { "strict": true, "noEmit": true }, "include": ["src"] }"#),
        ("src/shapes.ts", SHAPES),
        ("src/main.ts", "/* café */ export function drawCircle() {}\n"),
    ]);
    let host = SlowHost { real: RealHost::new(), support: tempfile::tempdir().unwrap() };
    let mut workbench = Workbench::new(Arc::new(host));
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    let mut session = Session { _fixture: fixture, workbench, project };
    let view = session.workbench.project(session.project).unwrap();
    let server = view.status.language_servers.first().cloned().expect("a language server item");
    assert_eq!(server.state, LanguageServerState::Ready, "{server:?}");

    // File Structure: tsgo's tree, flattened in the file's order.
    session.dispatch(Command::OpenFile("src/shapes.ts".into()));
    session.dispatch(Command::OpenFinder(FinderMode::FileSymbols));
    let results = session.eventual_results();
    assert_eq!(results, ["Shape", "area  Shape", "Circle", "radius  Circle", "area  Circle", "circle"], "{results:#?}");
    session.dispatch(Command::SetFinderQuery("radius".into()));
    session.dispatch(Command::AcceptFinder);
    assert_eq!(session.caret(), ("src/shapes.ts".into(), Caret { line: 5, column: 2 }));

    // Go to Symbol: a file that isn't open, after a non-ASCII comment
    // (tsgo counts UTF-8 bytes).
    session.dispatch(Command::OpenFinder(FinderMode::ProjectSymbols));
    session.dispatch(Command::SetFinderQuery("drawCircle".into()));
    let results = session.eventual_results();
    assert_eq!(results, ["drawCircle  src/main.ts"], "{results:#?}");
    session.dispatch(Command::AcceptFinder);
    assert_eq!(session.caret(), ("src/main.ts".into(), Caret { line: 0, column: 27 }));

    session.workbench.close_project(session.project);
}
