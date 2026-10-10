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
