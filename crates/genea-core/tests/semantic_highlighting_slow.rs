//! Semantic highlighting, inlay hints and code lenses from the real
//! TypeScript 7 language server (ticket #46's slow lane; spec #19, Testing
//! Decisions). It catches protocol drift when TypeScript is bumped: the
//! token legend, the settings tsgo reads for hints and lenses, and its
//! refreshes once it has them.
//!
//! Opt-in, because installing TypeScript needs the network (once; pnpm
//! caches it):
//!
//! ```sh
//! cargo test -p genea-core --test semantic_highlighting_slow -- --ignored
//! ```

use std::{
    ffi::OsString,
    io::Write,
    path::Path,
    process::Command as Process,
    sync::Arc,
    time::{Duration, Instant},
};

use genea_core::{Command, Highlight, ProjectId, VisibleLine, Workbench};
use genea_host::{Clipboard, Clock, DownloadError, Downloads, Host, Processes, Ptys, RealHost};
use genea_testkit::FixtureProject;

/// The TypeScript the fixture installs: the templates' version.
const TYPESCRIPT: &str = "7.0.2";

/// How long tsgo may take to answer after a change.
const PATIENCE: Duration = Duration::from_secs(30);

/// The real host's clock and processes, with its own support folder, no
/// `SHELL` and no downloads (as in `language_server_slow.rs`).
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

const MAIN: &str = "\
export interface Size { width: number; height: number }
export function area(size: Size) {
  const total = size.width * size.height;
  return total;
}
area({ width: 2, height: 3 });
";

#[test]
#[ignore = "installs TypeScript 7 with pnpm (network); opt in with --ignored"]
fn real_tsgo_recolours_parameters_and_types_and_shows_hints_and_lenses_when_turned_on() {
    let package_json =
        format!(r#"{{ "name": "slow-lane", "private": true, "devDependencies": {{ "typescript": "{TYPESCRIPT}" }} }}"#);
    let fixture = FixtureProject::new()
        .file("package.json", package_json)
        .file("tsconfig.json", r#"{ "compilerOptions": { "strict": true, "noEmit": true }, "include": ["src"] }"#)
        .file("src/main.ts", MAIN)
        .build();
    let output = Process::new("pnpm")
        .args(["install", "--config.confirmModulesPurge=false"])
        .current_dir(fixture.root())
        .env("CI", "1")
        .output()
        .expect("run pnpm (the slow lane needs it on PATH)");
    assert!(output.status.success(), "pnpm install failed:\n{}", String::from_utf8_lossy(&output.stderr));

    let host = SlowHost { real: RealHost::new(), support: tempfile::tempdir().unwrap() };
    let mut workbench = Workbench::new(Arc::new(host));
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::SetViewport { rows: 20.0 });
    workbench.dispatch(project, Command::OpenFile("src/main.ts".into()));

    // Semantic tokens: `size` where it is used is a parameter, `Size` a type.
    let lines = eventually(&mut workbench, project, |lines| highlight_of(&lines[2], "size", 1) == Some(Highlight::Parameter));
    assert_eq!(highlight_of(&lines[2], "size", 1), Some(Highlight::Parameter), "{:?}", lines[2]);
    assert_eq!(highlight_of(&lines[1], "Size", 0), Some(Highlight::Type), "{:?}", lines[1]);
    assert!(!lines[2].text.contains(": number"), "no hints by default: {:?}", lines[2].text);

    // Inlay hints, once turned on.
    fixture.write("genea.jsonc", r#"{ "inlayHints": true }"#);
    let lines = eventually(&mut workbench, project, |lines| lines[2].text.contains("total: number"));
    assert_eq!(lines[2].text, "  const total: number = size.width * size.height;");

    // Code lenses, once turned on; the hints go.
    fixture.write("genea.jsonc", r#"{ "codeLens": true }"#);
    let lines = eventually(&mut workbench, project, |lines| lines[0].text.contains("reference"));
    assert!(lines[0].text.starts_with("export interface Size { width: number; height: number }  "), "{:?}", lines[0].text);
    assert!(lines[1].text.ends_with("1 reference"), "{:?}", lines[1].text);
    assert_eq!(lines[2].text, "  const total = size.width * size.height;");
}

/// Settles until the visible lines satisfy `done`, for up to
/// [`PATIENCE`], and returns them.
fn eventually(workbench: &mut Workbench, project: ProjectId, done: impl Fn(&[VisibleLine]) -> bool) -> Vec<VisibleLine> {
    let deadline = Instant::now() + PATIENCE;
    loop {
        workbench.settle().unwrap();
        let lines = workbench.project(project).unwrap().editor.map(|e| e.lines).unwrap_or_default();
        if (lines.len() > 2 && done(&lines)) || Instant::now() > deadline {
            return lines;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The highlight of the `nth` stretch of a line that is exactly `text`.
fn highlight_of(line: &VisibleLine, text: &str, nth: usize) -> Option<Highlight> {
    let chars: Vec<char> = line.text.chars().collect();
    line.highlights
        .iter()
        .filter(|s| chars[s.columns.clone()].iter().collect::<String>() == text)
        .nth(nth)
        .map(|s| s.highlight)
}
