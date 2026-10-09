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

#[test]
fn oxfmtrc_overrides_win_over_its_root_options_for_the_files_they_match() {
    let oxfmtrc = r#"{
        // JSONC, like Oxfmt reads it.
        "tabWidth": 4,
        "overrides": [
            { "files": ["*.md"], "options": { "tabWidth": 3 } },
            { "files": ["docs/**"], "excludeFiles": ["docs/keep.md"], "options": { "useTabs": true } },
        ],
    }"#;
    let fixture = FixtureProject::new()
        .file(".oxfmtrc.json", oxfmtrc)
        .file("src/main.ts", "")
        .file("src/notes.md", "")
        .file("docs/guide.md", "")
        .file("docs/keep.md", "");
    let (_fixture, mut workbench, project) = open(fixture, "src/main.ts");
    assert_eq!(indentation(&workbench, project), "4 spaces");

    let mut shown = Vec::new();
    for file in ["src/notes.md", "docs/guide.md", "docs/keep.md"] {
        workbench.dispatch(project, Command::OpenFile(file.into()));
        workbench.settle().unwrap();
        shown.push(indentation(&workbench, project));
    }

    // A pattern without a slash matches the file name in any folder.
    assert_eq!(shown, ["3 spaces", "Tabs", "3 spaces"]);
}
