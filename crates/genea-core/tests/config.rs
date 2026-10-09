//! The root `genea.jsonc` config: its keys, defaults and error handling
//! (ticket #29, the schema from #13).

use genea_core::{Config, ProjectId, TerminalPosition, Theme, Workbench};
use genea_testkit::{FixtureBuilder, FixtureProject, TestHost};

fn open(fixture: FixtureBuilder) -> (FixtureProject, Workbench, ProjectId) {
    let fixture = fixture.build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    (fixture, workbench, project)
}

fn config(workbench: &Workbench, project: ProjectId) -> Config {
    workbench.project(project).unwrap().config
}

#[test]
fn a_project_without_a_config_gets_the_defaults() {
    let (_fixture, workbench, project) = open(FixtureProject::new());

    assert_eq!(
        config(&workbench, project),
        Config {
            theme: Theme::System,
            terminal_position: TerminalPosition::Right,
            format_on_save: true,
            fix_on_save: true,
            inlay_hints: false,
            code_lens: false,
            exclude: vec![],
        }
    );
    assert_eq!(workbench.project(project).unwrap().problems, []);
}

#[test]
fn every_key_is_read_from_the_root_config() {
    let (_fixture, workbench, project) = open(FixtureProject::new().file(
        "genea.jsonc",
        r#"{
            // Comments and trailing commas are fine.
            "theme": "dark",
            "terminalPosition": "bottom",
            "formatOnSave": false,
            "fixOnSave": false,
            "inlayHints": true,
            "codeLens": true,
            "exclude": ["dist/", "!dist/keep.ts",],
        }"#,
    ));

    assert_eq!(
        config(&workbench, project),
        Config {
            theme: Theme::Dark,
            terminal_position: TerminalPosition::Bottom,
            format_on_save: false,
            fix_on_save: false,
            inlay_hints: true,
            code_lens: true,
            exclude: vec!["dist/".into(), "!dist/keep.ts".into()],
        }
    );
    assert_eq!(workbench.project(project).unwrap().problems, []);
}
