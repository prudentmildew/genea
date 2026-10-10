//! Quick fixes and organize imports against real tsgo (ticket #45; the
//! slow lane of spec #19, Testing Decisions): it catches drift in how tsgo
//! answers `textDocument/codeAction`.
//!
//! Opt-in, because installing TypeScript needs the network (once; pnpm
//! caches it):
//!
//! ```sh
//! cargo test -p genea-core --test quick_fixes_slow -- --ignored
//! ```

use std::{
    ffi::OsString,
    io::Write,
    path::Path,
    process::Command as Process,
    sync::Arc,
    time::{Duration, Instant},
};

use genea_core::{Command, ProjectId, QuickFixesView, Workbench};
use genea_host::{Clipboard, Clock, DownloadError, Downloads, Host, Processes, Ptys, RealHost};
use genea_testkit::FixtureProject;

/// The TypeScript the fixture installs: the templates' version.
const TYPESCRIPT: &str = "7.0.2";

/// How long tsgo may take to report or answer.
const PATIENCE: Duration = Duration::from_secs(30);

/// The real host's clock and processes, with its own support folder, no
/// `SHELL` (so no login shell runs) and no downloads.
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

    fn view(&self) -> genea_core::ProjectView {
        self.workbench.project(self.project).unwrap()
    }

    fn text(&self) -> String {
        let lines: Vec<String> = self.view().editor.unwrap().lines.into_iter().map(|line| line.text).collect();
        lines.join("\n")
    }

    /// Settles until `done`, for up to [`PATIENCE`]; tsgo checks in the
    /// background and debounces.
    fn eventually(&mut self, done: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            self.workbench.settle().unwrap();
            if done(self) || Instant::now() > deadline {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

const MAIN: &str = "import { b } from \"./b.ts\";\nimport { a } from \"./a.ts\";\nexport const x: number = helper() + a + b;\n";

#[test]
#[ignore = "installs TypeScript 7 with pnpm (network); opt in with --ignored"]
fn real_tsgo_offers_and_applies_a_missing_import_and_organizes_imports() {
    let fixture = installed(&[
        ("tsconfig.json", r#"{ "compilerOptions": { "strict": true, "noEmit": true, "module": "nodenext", "allowImportingTsExtensions": true }, "include": ["src"] }"#),
        ("src/a.ts", "export const a = 1;\n"),
        ("src/b.ts", "export const b = 2;\n"),
        ("src/util.ts", "export function helper(): number { return 1; }\n"),
        ("src/main.ts", MAIN),
    ]);
    let mut session = Session::open(fixture);
    session.dispatch(Command::OpenFile("src/main.ts".into()));
    session.eventually(|s| s.view().problems.iter().any(|p| p.message.contains("helper")));

    // At the end of the line, away from the problem: its line's problems count.
    session.dispatch(Command::PlaceCaret { line: 2, column: 44 });
    session.dispatch(Command::ShowQuickFixes);
    session.eventually(|s| s.view().quick_fixes.is_some());
    let QuickFixesView { items, .. } = session.view().quick_fixes.unwrap();
    let index = items.iter().position(|title| title.contains("./util")).unwrap_or_else(|| panic!("{items:?}"));

    session.dispatch(Command::ApplyQuickFix(index));
    let text = session.text();
    assert!(text.contains("import { helper } from \"./util"), "{text}");
    assert!(text.ends_with("export const x: number = helper() + a + b;\n"), "{text}");

    session.dispatch(Command::OrganizeImports);
    session.eventually(|s| s.text().starts_with("import { a }"));
    let text = session.text();
    let imports: Vec<&str> = text.lines().take_while(|line| line.starts_with("import")).collect();
    assert_eq!(imports.len(), 3, "{text}");
    assert!(imports[0].contains("./a.ts") && imports[1].contains("./b.ts") && imports[2].contains("./util"), "{text}");
}
