//! Fixes from the code review of spec #19's first version: each test
//! names the review item it covers.

use std::{fs, path::PathBuf};

use genea_core::{Command, ProjectId, ProjectView, Workbench};
use genea_testkit::{FixtureProject, TestHost};
use serde_json::Value;

fn view(workbench: &Workbench, project: ProjectId) -> ProjectView {
    workbench.project(project).unwrap()
}

fn run(workbench: &mut Workbench, project: ProjectId, commands: impl IntoIterator<Item = Command>) {
    for command in commands {
        workbench.dispatch(project, command);
        workbench.settle().unwrap();
    }
}

/// The one session file in the host's support folder.
fn session_file(host: &TestHost) -> PathBuf {
    let files: Vec<_> = fs::read_dir(host.support_dir().join("sessions")).unwrap().map(|e| e.unwrap().path()).collect();
    let [file] = &files[..] else { panic!("one session file, not {files:?}") };
    file.clone()
}

/// Item 1: a tab whose saved carets are missing, empty or malformed comes
/// back with one caret at the start of the file instead of none.
#[test]
fn a_restored_tab_without_usable_carets_gets_one_at_the_start() {
    for selections in [None, Some(serde_json::json!([])), Some(serde_json::json!("nonsense")), Some(serde_json::json!([[1]]))] {
        let fixture = FixtureProject::new().file("a.ts", "let a = 1;\nlet b = 2;\n").build();
        let host = TestHost::new();
        let mut workbench = Workbench::new(host.shared());
        let project = workbench.open_project(fixture.root()).unwrap();
        run(&mut workbench, project, [Command::OpenFile("a.ts".into()), Command::PlaceCaret { line: 1, column: 3 }]);
        workbench.quit();
        drop(workbench);

        let file = session_file(&host);
        let mut session: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        let tab = &mut session["panes"][0]["tabs"][0];
        match &selections {
            None => drop(tab.as_object_mut().unwrap().remove("selections")),
            Some(value) => tab["selections"] = value.clone(),
        }
        fs::write(&file, session.to_string()).unwrap();

        let mut restarted = Workbench::new(host.shared());
        let project = restarted.restore_session()[0];
        restarted.settle().unwrap();
        run(&mut restarted, project, [Command::InsertText("x".into())]);

        let editor = view(&restarted, project).editor.unwrap();
        assert_eq!((editor.caret.line, editor.caret.column), (0, 1), "selections {selections:?}");
        assert_eq!(editor.lines[0].text, "xlet a = 1;", "selections {selections:?}");
    }
}

/// Item 2: saves of one file are written one at a time, the newest last,
/// so a quick second save (⌘S twice) never leaves an older text on disk or
/// marks an older text as saved.
#[test]
fn quick_successive_saves_leave_the_newest_text_saved() {
    let big = "x".repeat(2_000_000);
    for round in 0..8 {
        let fixture = FixtureProject::new().file("a.ts", &format!("{big}\n")).build();
        let mut workbench = Workbench::new(TestHost::new().shared());
        let project = workbench.open_project(fixture.root()).unwrap();
        run(&mut workbench, project, [Command::OpenFile("a.ts".into())]);

        for text in ["1", "2", "3"] {
            workbench.dispatch(project, Command::InsertText(text.into()));
            workbench.dispatch(project, Command::Save);
        }
        workbench.settle().unwrap();

        let disk = fixture.read("a.ts");
        assert!(
            disk == format!("123{big}\n"),
            "round {round}: disk has {} bytes starting {:?}",
            disk.len(),
            &disk[..disk.len().min(8)]
        );
        assert!(!view(&workbench, project).editor.unwrap().modified, "round {round}: the newest text is saved");
    }
}

/// Item 6: formatter config at the root counts without a root
/// `package.json` (or with one that doesn't parse).
#[test]
fn root_formatter_config_counts_without_a_readable_root_package_json() {
    for package_json in [None, Some("{ not json")] {
        let mut builder = FixtureProject::new().file(".prettierrc", "{}\n");
        if let Some(text) = package_json {
            builder = builder.file("package.json", text);
        }
        let fixture = builder.build();
        let mut workbench = Workbench::new(TestHost::new().shared());
        let project = workbench.open_project(fixture.root()).unwrap();
        workbench.settle().unwrap();

        assert_eq!(
            view(&workbench, project).status.foreign_tools.as_deref(),
            Some("Reduced mode: Prettier"),
            "package.json {package_json:?}"
        );
    }
}
