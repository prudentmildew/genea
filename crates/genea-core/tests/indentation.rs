//! Indentation (ticket #26): each file's `useTabs` and `tabWidth`, resolved
//! the way Oxfmt does: `.oxfmtrc.json` overrides, then its root options,
//! then the nearest `.editorconfig` (with its glob sections), then 2 spaces.
//! It drives Tab, ⇧Tab, auto-indent and the status bar.

use genea_core::{Command, ProjectId, Workbench};
use genea_testkit::{FixtureProject, FixtureBuilder, TestHost};

/// Opens `file` in a project built from `fixture`, settled.
fn open(fixture: FixtureBuilder, file: &str) -> (FixtureProject, Workbench, ProjectId) {
    let fixture = fixture.build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::SetViewport { rows: 20.0 });
    workbench.dispatch(project, Command::OpenFile(file.into()));
    workbench.settle().unwrap();
    (fixture, workbench, project)
}

/// The status bar's indentation item.
fn indentation(workbench: &Workbench, project: ProjectId) -> String {
    workbench.project(project).unwrap().status.indentation.unwrap()
}

#[test]
fn a_project_without_either_config_indents_with_two_spaces() {
    let (_fixture, workbench, project) = open(FixtureProject::new().file("src/main.ts", "let a = 1;\n"), "src/main.ts");

    assert_eq!(indentation(&workbench, project), "2 spaces");
}

#[test]
fn oxfmtrc_root_options_set_the_indentation() {
    let fixture = FixtureProject::new()
        .file(".oxfmtrc.json", r#"{ "tabWidth": 4 }"#)
        .file("src/main.ts", "let a = 1;\n");
    let (_fixture, workbench, project) = open(fixture, "src/main.ts");

    assert_eq!(indentation(&workbench, project), "4 spaces");
}
