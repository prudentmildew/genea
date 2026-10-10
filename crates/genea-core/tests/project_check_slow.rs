//! The project check's slow lane (ticket #48; spec #19, Testing
//! Decisions): the real TypeScript 7 `tsc -b --noEmit`, installed into a
//! fixture project with project references. It catches drift in tsc's
//! output format and in how TypeScript 7 treats references (TS6310) when
//! TypeScript is bumped.
//!
//! Opt-in, because installing TypeScript needs the network (once; pnpm
//! caches it):
//!
//! ```sh
//! cargo test -p genea-core --test project_check_slow -- --ignored
//! ```

use std::{
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
    process::Command as Process,
    sync::Arc,
};

use genea_core::{Command, ProblemSource, Severity, Workbench};
use genea_host::{Clipboard, Clock, DownloadError, Downloads, Host, Processes, Ptys, RealHost};
use genea_testkit::FixtureProject;

/// The TypeScript the fixture installs: the templates' version.
const TYPESCRIPT: &str = "7.0.2";

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

const PROJECT: &str = r#"{ "compilerOptions": { "composite": true, "strict": true, "outDir": "dist", "rootDir": "src" }, "include": ["src"] }"#;
const REFERENCING: &str = r#"{ "compilerOptions": { "composite": true, "strict": true, "outDir": "dist", "rootDir": "src" }, "include": ["src"], "references": [{ "path": "../a" }] }"#;

#[test]
#[ignore = "installs TypeScript 7 with pnpm (network); opt in with --ignored"]
fn real_tsc_checks_the_project_and_projects_with_references_are_named_as_skipped() {
    let fixture = installed(&[
        ("tsconfig.json", r#"{ "files": [], "references": [{ "path": "./a" }, { "path": "./b" }] }"#),
        ("a/tsconfig.json", PROJECT),
        ("a/src/index.ts", "export const s = \"😀\"; export const a: number = \"x\";\nexport const ok = 1;\n"),
        ("b/tsconfig.json", REFERENCING),
        ("b/src/index.ts", "import { ok } from \"../../a/src/index\";\nexport const b: string = ok;\n"),
    ]);
    let host = SlowHost { real: RealHost::new(), support: tempfile::tempdir().unwrap() };
    let mut workbench = Workbench::new(Arc::new(host));
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::RunProjectCheck);
    workbench.settle().unwrap();

    let view = workbench.project(project).unwrap();
    let checked: Vec<(PathBuf, String, Severity, String)> = view
        .problems
        .into_iter()
        .filter(|p| p.source == ProblemSource::ProjectCheck)
        .map(|p| (p.path, p.location, p.severity, p.message))
        .collect();
    assert_eq!(
        checked,
        [(
            "a/src/index.ts".into(),
            // tsc counts UTF-16 units (the emoji is two); Problems counts chars.
            "1:36".into(),
            Severity::Error,
            "Type 'string' is not assignable to type 'number'.".into()
        )]
    );
    let notices: Vec<String> = view.notices.into_iter().map(|n| n.message).collect();
    assert!(
        notices.iter().any(|n| n.starts_with("The project check skipped b/tsconfig.json:")),
        "TypeScript 7 still won't check b under -b --noEmit (TS6310)? {notices:#?}"
    );
    assert_eq!(view.status.project_check, None);
}
