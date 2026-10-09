//! tsgo's slow lane (ticket #42; spec #19, Testing Decisions): the real
//! TypeScript 7 language server, installed into a fixture project, gives
//! Genea real diagnostics. It catches protocol drift when TypeScript is
//! bumped.
//!
//! Opt-in, because installing TypeScript needs the network (once; pnpm
//! caches it):
//!
//! ```sh
//! cargo test -p genea-core --test language_server_slow -- --ignored
//! ```
//!
//! It runs `pnpm` from PATH to install `typescript` at `TYPESCRIPT` below.
//! tsgo checks in the background and debounces, so after an edit the test
//! settles and looks again until the diagnostics are there (or 30 s pass),
//! rather than settling once.

use std::{
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
    process::Command as Process,
    sync::Arc,
    time::{Duration, Instant},
};

use genea_core::{CaretMove, Command, LanguageServerState, ProblemSource, ProjectId, Severity, Workbench};
use genea_host::{Clipboard, Clock, DownloadError, Downloads, Host, Processes, Ptys, RealHost};
use genea_testkit::FixtureProject;

/// The TypeScript the fixture installs: the templates' version.
const TYPESCRIPT: &str = "7.0.2";

/// How long tsgo may take to report after an edit.
const PATIENCE: Duration = Duration::from_secs(30);

/// The real host's clock and processes, with its own support folder, no
/// `SHELL` (so no login shell runs: tsgo gets Genea's own environment), and
/// no downloads (the toolchain isn't what this lane checks).
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
    fn open(fixture: FixtureProject) -> Self {
        let host = SlowHost { real: RealHost::new(), support: tempfile::tempdir().unwrap() };
        let mut workbench = Workbench::new(Arc::new(host));
        let project = workbench.open_project(fixture.root()).unwrap();
        workbench.settle().unwrap();
        Session { _fixture: fixture, workbench, project }
    }

    fn dispatch(&mut self, command: Command) {
        self.workbench.dispatch(self.project, command);
    }

    /// TypeScript's problems: (path, location, severity, message).
    fn problems(&self) -> Vec<(PathBuf, String, Severity, String)> {
        let view = self.workbench.project(self.project).unwrap();
        view.problems
            .into_iter()
            .filter(|p| p.source == ProblemSource::TypeScript)
            .map(|p| (p.path, p.location, p.severity, p.message))
            .collect()
    }

    /// Settles until TypeScript's problems satisfy `done`, for up to
    /// [`PATIENCE`], and returns them.
    fn eventually(
        &mut self,
        done: impl Fn(&[(PathBuf, String, Severity, String)]) -> bool,
    ) -> Vec<(PathBuf, String, Severity, String)> {
        let deadline = Instant::now() + PATIENCE;
        loop {
            self.workbench.settle().unwrap();
            let problems = self.problems();
            if done(&problems) || Instant::now() > deadline {
                return problems;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[test]
#[ignore = "installs TypeScript 7 with pnpm (network); opt in with --ignored"]
fn real_tsgo_reports_type_errors_in_open_files_and_follows_edits_and_new_files() {
    let fixture = installed(&[
        ("tsconfig.json", r#"{ "compilerOptions": { "strict": true, "noEmit": true, "module": "nodenext", "allowImportingTsExtensions": true }, "include": ["src"] }"#),
        ("src/main.ts", "let count: number = \"three\";\nimport { helper } from \"./helper.ts\";\nexport { count, helper };\n"),
    ]);
    let mut session = Session::open(fixture);
    let view = session.workbench.project(session.project).unwrap();
    let server = view.status.language_servers.first().cloned().expect("a language server item");
    assert_eq!(server.state, LanguageServerState::Ready, "{server:?}");
    assert!(server.label.starts_with("TypeScript 7"), "{server:?}");

    session.dispatch(Command::OpenFile("src/main.ts".into()));
    let problems = session.eventually(|problems| problems.len() >= 2);
    let main = PathBuf::from("src/main.ts");
    assert!(
        problems.contains(&(main.clone(), "1:5".into(), Severity::Error, "Type 'string' is not assignable to type 'number'.".into())),
        "{problems:#?}"
    );
    assert!(problems.iter().any(|(_, location, _, message)| location == "2:24" && message.contains("./helper.ts")), "{problems:#?}");

    // Fixing the type error in the editor clears it.
    session.dispatch(Command::PlaceCaret { line: 0, column: 20 });
    session.dispatch(Command::Select(CaretMove::LineEnd));
    session.dispatch(Command::InsertText("3;".into()));
    let problems = session.eventually(|problems| problems.len() == 1);
    assert!(problems.iter().all(|(_, location, _, _)| location == "2:24"), "{problems:#?}");

    // Creating the missing module on disk reaches tsgo through Genea's watcher.
    session._fixture.write("src/helper.ts", "export const helper = 1;\n");
    let problems = session.eventually(|problems| problems.is_empty());
    assert_eq!(problems, []);
}
