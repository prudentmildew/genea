//! The project check (ticket #48): `tsc -b --noEmit` with the project's
//! TypeScript 7, played by `genea_testkit::FakeTsc` (which hands `--lsp`
//! starts of the same `tsc` to a `FakeLsp`).

use std::path::PathBuf;

use genea_core::{Command, ProblemSource, ProjectId, Severity, Workbench};
use genea_testkit::{FakeLsp, FakeTsc, FixtureBuilder, FixtureProject, TestHost};

/// The tsgo binary in a project with TypeScript 7 installed by pnpm.
const STORE: &str = "node_modules/.pnpm/typescript@7.0.2/node_modules";
const TSGO: &str = "node_modules/.pnpm/typescript@7.0.2/node_modules/@typescript/typescript-darwin-arm64/lib/tsc";

const TYPE_ERROR: &str = "Type 'string' is not assignable to type 'number'.";

/// A project with TypeScript 7 installed (as pnpm lays it out).
fn typescript_project() -> FixtureBuilder {
    FixtureProject::new()
        .file("package.json", r#"{ "name": "app", "devDependencies": { "typescript": "^7.0.2" } }"#)
        .file("tsconfig.json", "{}")
        .file(format!("{STORE}/typescript/package.json"), r#"{ "name": "typescript", "version": "7.0.2" }"#)
        .file(TSGO, "#!/bin/sh\n")
}

struct Session {
    fixture: FixtureProject,
    #[allow(dead_code)]
    host: TestHost,
    workbench: Workbench,
    project: ProjectId,
}

/// Opens `fixture` with `tsc` played by `tsc` (checks) and `lsp` (tsgo).
fn open_with(fixture: FixtureProject, tsc: &FakeTsc, lsp: &FakeLsp) -> Session {
    std::os::unix::fs::symlink(fixture.path(format!("{STORE}/typescript")), fixture.path("node_modules/typescript"))
        .unwrap();
    let host = TestHost::new();
    tsc.install(&host, "tsc", lsp);
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    let mut session = Session { fixture, host, workbench, project };
    session.settle();
    session
}

fn open(fixture: FixtureProject, tsc: &FakeTsc) -> Session {
    open_with(fixture, tsc, &FakeLsp::new())
}

/// A problem as the Problems view lists it: (source, path, location,
/// severity, message).
type Item = (ProblemSource, PathBuf, String, Severity, String);

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

    fn check(&mut self) {
        self.dispatch(Command::RunProjectCheck);
        self.settle();
    }

    /// The project check's problems in Problems.
    fn checked(&self) -> Vec<Item> {
        self.problems().into_iter().filter(|p| p.0 == ProblemSource::ProjectCheck).collect()
    }

    fn problems(&self) -> Vec<Item> {
        self.view().problems.into_iter().map(|p| (p.source, p.path, p.location, p.severity, p.message)).collect()
    }
}

fn error(path: &str, location: &str, message: &str) -> Item {
    (ProblemSource::ProjectCheck, path.into(), location.into(), Severity::Error, message.into())
}

#[test]
fn a_project_check_fills_problems_with_errors_from_unopened_files() {
    let fixture = typescript_project()
        .file("src/main.ts", "let a: number = \"oops\";\n")
        .file("src/util.ts", "// 😀\nfoo();\n")
        .build();
    let tsc = FakeTsc::new().reports(
        &format!(
            "src/main.ts(1,17): error TS2322: {TYPE_ERROR}\n\
             src/util.ts(2,1): error TS2304: Cannot find name 'foo'.\n"
        ),
        1,
    );
    let mut session = open(fixture, &tsc);

    session.check();

    assert_eq!(
        session.checked(),
        [error("src/main.ts", "1:17", TYPE_ERROR), error("src/util.ts", "2:1", "Cannot find name 'foo'.")]
    );
    let root = session.fixture.root().to_owned();
    let checks = tsc.checks();
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].program, root.join(TSGO), "the project's own TypeScript 7");
    assert_eq!(checks[0].args, ["-b", "--noEmit", "--pretty", "false"]);
    assert_eq!(checks[0].cwd.as_deref(), Some(root.as_path()));
    assert!(checks[0].clear_env, "it gets the project environment");
}

#[test]
fn a_second_check_replaces_the_first_ones_results() {
    let fixture =
        typescript_project().file("src/main.ts", "let a: number = \"oops\";\n").file("src/b.ts", "x;\n").build();
    let tsc = FakeTsc::new().reports(&format!("src/main.ts(1,17): error TS2322: {TYPE_ERROR}\n"), 1);
    let mut session = open(fixture, &tsc);
    session.check();

    tsc.set_report(
        "src/b.ts(1,1): error TS2304: Cannot find name 'x'.\n\
         src/b.ts(1,1): error TS2322: Type '{ n: string; }[]' is not assignable to type '{ n: number; }[]'.\n  \
         Type '{ n: string; }' is not assignable to type '{ n: number; }'.\n    \
         Types of property 'n' are incompatible.\n",
        2,
    );
    session.check();

    assert_eq!(
        session.checked(),
        [
            error("src/b.ts", "1:1", "Cannot find name 'x'."),
            error(
                "src/b.ts",
                "1:1",
                "Type '{ n: string; }[]' is not assignable to type '{ n: number; }[]'.\n\
                 Type '{ n: string; }' is not assignable to type '{ n: number; }'.\n  \
                 Types of property 'n' are incompatible."
            ),
        ]
    );
    assert_eq!(session.view().status.errors, 2);

    tsc.set_report("", 0);
    session.check();
    assert_eq!(session.checked(), [], "a clean check clears them");
}

#[test]
fn the_status_bar_shows_a_check_while_it_runs() {
    let tsc = FakeTsc::new();
    let mut session = open(typescript_project().build(), &tsc);
    assert_eq!(session.view().status.project_check, None);

    session.dispatch(Command::RunProjectCheck);
    assert_eq!(session.view().status.project_check.as_deref(), Some("Checking project…"));

    session.settle();
    assert_eq!(session.view().status.project_check, None);
}

#[test]
fn open_files_show_live_diagnostics_instead_of_the_checks() {
    let fixture = typescript_project()
        .file("src/main.ts", "let a: number = \"oops\";\n")
        .file("src/other.ts", "let b: number = \"no\";\n")
        .build();
    let tsc = FakeTsc::new().reports(
        &format!(
            "src/main.ts(1,17): error TS2322: {TYPE_ERROR}\n\
             src/other.ts(1,17): error TS2322: {TYPE_ERROR}\n"
        ),
        1,
    );
    let lsp = FakeLsp::new().error("\"oops\"", "Live: not a number.");
    let mut session = open_with(fixture, &tsc, &lsp);
    session.check();

    session.dispatch(Command::OpenFile("src/main.ts".into()));
    session.settle();

    let live =
        (ProblemSource::TypeScript, "src/main.ts".into(), "1:17".into(), Severity::Error, "Live: not a number.".into());
    assert_eq!(session.problems(), [live, error("src/other.ts", "1:17", TYPE_ERROR)]);
    let inline: Vec<String> = session.view().editor.unwrap().problems.into_iter().map(|p| p.message).collect();
    assert_eq!(inline, ["Live: not a number."]);
    assert_eq!(session.view().status.errors, 2);

    session.dispatch(Command::CloseTab { pane: 0, tab: 0 });
    session.settle();
    assert_eq!(
        session.problems(),
        [error("src/main.ts", "1:17", TYPE_ERROR), error("src/other.ts", "1:17", TYPE_ERROR)],
        "a closed file shows the check's results again"
    );
}

impl Session {
    /// The files whose project-check results show as stale.
    fn stale(&self) -> Vec<PathBuf> {
        let mut stale: Vec<PathBuf> = self.view().problems.into_iter().filter(|p| p.stale).map(|p| p.path).collect();
        stale.dedup();
        stale
    }
}

/// A check with an error in each of `src/main.ts` and `src/other.ts`.
fn two_errors() -> (FixtureProject, FakeTsc) {
    let fixture = typescript_project()
        .file("src/main.ts", "let a: number = \"oops\";\n")
        .file("src/other.ts", "let b: number = \"no\";\n")
        .build();
    let tsc = FakeTsc::new().reports(
        &format!(
            "src/main.ts(1,17): error TS2322: {TYPE_ERROR}\n\
             src/other.ts(1,17): error TS2322: {TYPE_ERROR}\n"
        ),
        1,
    );
    (fixture, tsc)
}

#[test]
fn editing_a_file_marks_its_stored_results_stale_until_the_next_check() {
    let (fixture, tsc) = two_errors();
    let mut session = open(fixture, &tsc);
    session.check();
    assert_eq!(session.stale(), Vec::<PathBuf>::new());

    session.dispatch(Command::OpenFile("src/main.ts".into()));
    session.settle();
    session.dispatch(Command::InsertText("// ".into()));
    session.dispatch(Command::Save);
    session.settle();
    session.dispatch(Command::CloseTab { pane: 0, tab: 0 });
    session.settle();

    assert_eq!(session.stale(), [PathBuf::from("src/main.ts")]);
    assert_eq!(session.checked().len(), 2, "stale results stay until the next check");

    session.check();
    assert_eq!(session.stale(), Vec::<PathBuf>::new());
}

#[test]
fn a_change_on_disk_marks_its_files_results_stale() {
    let (fixture, tsc) = two_errors();
    let mut session = open(fixture, &tsc);
    session.check();

    session.fixture.write("src/other.ts", "let b: number = 1;\n");
    session.settle();

    assert_eq!(session.stale(), [PathBuf::from("src/other.ts")]);
}

#[test]
fn a_change_while_the_check_runs_marks_its_results_stale() {
    let (fixture, tsc) = two_errors();
    let tsc = tsc.hold();
    let mut session = open(fixture, &tsc);

    session.dispatch(Command::RunProjectCheck);
    session.fixture.write("src/other.ts", "let b: number = 1;\n");
    let release = tsc.clone();
    let releasing = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        release.release();
    });
    session.settle();
    releasing.join().unwrap();

    assert_eq!(session.checked().len(), 2);
    assert_eq!(session.stale(), [PathBuf::from("src/other.ts")]);
}

impl Session {
    /// The project check's notices.
    fn notices(&self) -> Vec<String> {
        self.view().notices.into_iter().map(|n| n.message).filter(|m| m.starts_with("The project check")).collect()
    }
}

/// What TypeScript 7.0.2 prints for `tsc -b --noEmit` in a project whose
/// `b` references `a` (probed): `a` is checked, `b` isn't.
fn references_output(root: &std::path::Path) -> String {
    format!(
        "a/src/index.ts(1,14): error TS2322: {TYPE_ERROR}\n\
         b/tsconfig.json(1,132): error TS6310: Referenced project '{}/a' may not disable emit.\n\
         c/tsconfig.json(1,90): error TS6310: Referenced project '{}/a' may not disable emit.\n",
        root.display(),
        root.display()
    )
}

#[test]
fn projects_that_typescript_7_wont_check_without_emitting_are_named_in_a_notice_not_in_problems() {
    let fixture = typescript_project().file("a/src/index.ts", "export const a: number = \"x\";\n").build();
    let tsc = FakeTsc::new();
    let mut session = open(fixture, &tsc);
    tsc.set_report(&references_output(session.fixture.root()), 1);

    session.check();

    assert_eq!(session.checked(), [error("a/src/index.ts", "1:14", TYPE_ERROR)]);
    assert_eq!(
        session.notices(),
        ["The project check skipped b/tsconfig.json and c/tsconfig.json: TypeScript 7 won't check a project that \
          references other projects without emitting their output (TS6310). Their open files still get live \
          diagnostics."]
    );

    tsc.set_report("", 0);
    session.check();
    assert_eq!(session.notices(), Vec::<String>::new(), "the next check's notices replace the last one's");
}

#[test]
fn a_check_that_fails_says_why_and_keeps_the_last_results() {
    let (fixture, tsc) = two_errors();
    let mut session = open(fixture, &tsc);
    session.check();

    let root = session.fixture.root().to_owned();
    tsc.set_report(&format!("error TS5083: Cannot read file '{}/tsconfig.json'.\n", root.display()), 1);
    session.check();

    assert_eq!(
        session.notices(),
        [format!("The project check failed: Cannot read file '{}/tsconfig.json'.", root.display())]
    );
    assert_eq!(session.checked().len(), 2);

    tsc.set_report("Segmentation fault\n", 139);
    session.check();
    assert_eq!(session.notices(), ["The project check failed: tsc exited with code 139."]);
    assert_eq!(session.checked().len(), 2);
}

#[test]
fn without_typescript_7_a_check_says_it_needs_it() {
    let fixture = FixtureProject::new().file("package.json", r#"{ "name": "app" }"#).build();
    let host = TestHost::new();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::RunProjectCheck);
    workbench.settle().unwrap();

    let notices: Vec<String> = workbench.project(project).unwrap().notices.into_iter().map(|n| n.message).collect();
    let needs = "The project check needs TypeScript 7 installed in this project.".to_owned();
    assert!(notices.contains(&needs), "{notices:?}");
    assert_eq!(workbench.project(project).unwrap().status.project_check, None);
}

#[test]
fn closing_the_project_stops_a_running_check() {
    let tsc = FakeTsc::new().hold();
    let mut session = open(typescript_project().build(), &tsc);
    session.dispatch(Command::RunProjectCheck);

    session.workbench.close_project(session.project);
    session.settle();

    assert_eq!(tsc.killed(), 1);
}

#[test]
fn a_check_started_while_one_runs_replaces_it() {
    let tsc = FakeTsc::new().hold();
    let fixture = typescript_project().file("src/b.ts", "x;\n").build();
    let mut session = open(fixture, &tsc);
    session.dispatch(Command::RunProjectCheck);

    tsc.set_report("src/b.ts(1,1): error TS2304: Cannot find name 'x'.\n", 1);
    session.dispatch(Command::RunProjectCheck);
    tsc.release();
    session.settle();

    assert_eq!(session.checked(), [error("src/b.ts", "1:1", "Cannot find name 'x'.")]);
    assert_eq!(session.view().status.project_check, None);
}

#[test]
fn find_action_runs_a_project_check() {
    let fixture = typescript_project().file("src/b.ts", "x;\n").build();
    let tsc = FakeTsc::new().reports("src/b.ts(1,1): error TS2304: Cannot find name 'x'.\n", 1);
    let mut session = open(fixture, &tsc);

    session.dispatch(Command::OpenFinder(genea_core::FinderMode::Actions));
    session.dispatch(Command::SetFinderQuery("run project check".into()));
    session.settle();
    let finder = session.view().finder.unwrap();
    assert_eq!(finder.items[0].label, "Run Project Check");
    session.dispatch(Command::AcceptFinder);
    session.settle();

    assert_eq!(session.checked(), [error("src/b.ts", "1:1", "Cannot find name 'x'.")]);
}
