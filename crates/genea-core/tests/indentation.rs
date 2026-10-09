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

/// The indentation shown for each file, opened one after another.
fn indentations(workbench: &mut Workbench, project: ProjectId, files: &[&str]) -> Vec<String> {
    files
        .iter()
        .map(|file| {
            workbench.dispatch(project, Command::OpenFile((*file).into()));
            workbench.settle().unwrap();
            indentation(workbench, project)
        })
        .collect()
}

#[test]
fn editorconfig_sets_the_indentation_with_its_glob_sections() {
    let editorconfig = "\
root = true

[*]
indent_style = space
indent_size = 4

[*.md]
indent_size = 3

[Makefile]
indent_style = tab
";
    let fixture = FixtureProject::new()
        .file(".editorconfig", editorconfig)
        .file("src/main.ts", "")
        .file("docs/guide.md", "")
        .file("Makefile", "");
    let (_fixture, mut workbench, project) = open(fixture, "src/main.ts");

    let shown = indentations(&mut workbench, project, &["src/main.ts", "docs/guide.md", "Makefile"]);

    assert_eq!(shown, ["4 spaces", "3 spaces", "Tabs"]);
}

#[test]
fn oxfmtrc_wins_over_editorconfig_which_fills_in_only_what_it_leaves_unset() {
    let editorconfig = "[*]\nindent_style = tab\nindent_size = 8\n\n[*.md]\nindent_style = space\n";
    let oxfmtrc = r#"{ "overrides": [{ "files": ["*.ts"], "options": { "useTabs": false } }] }"#;
    let fixture = FixtureProject::new()
        .file(".editorconfig", editorconfig)
        .file(".oxfmtrc.json", oxfmtrc)
        .file("src/main.ts", "")
        .file("src/main.css", "")
        .file("README.md", "");
    let (_fixture, mut workbench, project) = open(fixture, "src/main.ts");

    let shown = indentations(&mut workbench, project, &["src/main.ts", "src/main.css", "README.md"]);

    // main.ts: spaces from Oxfmt, the width from `indent_size`. main.css:
    // tabs from `.editorconfig`. README.md: its section's spaces.
    assert_eq!(shown, ["8 spaces", "Tabs", "8 spaces"]);
}

#[test]
fn editing_either_config_file_updates_the_open_file_without_a_reopen() {
    let (fixture, mut workbench, project) = open(FixtureProject::new().file("src/main.ts", ""), "src/main.ts");
    assert_eq!(indentation(&workbench, project), "2 spaces");

    fixture.write(".editorconfig", "[*]\nindent_style = tab\n");
    workbench.settle().unwrap();
    assert_eq!(indentation(&workbench, project), "Tabs");

    fixture.write(".oxfmtrc.json", r#"{ "useTabs": false, "tabWidth": 4 }"#);
    workbench.settle().unwrap();
    assert_eq!(indentation(&workbench, project), "4 spaces");

    fixture.write(".oxfmtrc.json", r#"{ "tabWidth": 4 }"#);
    workbench.settle().unwrap();
    assert_eq!(indentation(&workbench, project), "Tabs");

    fixture.remove(".editorconfig");
    workbench.settle().unwrap();
    assert_eq!(indentation(&workbench, project), "4 spaces");
}
