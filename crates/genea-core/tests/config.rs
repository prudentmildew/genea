//! The root `genea.jsonc` config: its keys, defaults and error handling
//! (ticket #29, the schema from #13).

use genea_core::{Config, ProblemSource, ProjectId, Severity, TerminalPosition, Theme, Workbench};
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

/// Problems as (severity, path, 1-based location, message) for compact asserts.
fn problems(workbench: &Workbench, project: ProjectId) -> Vec<(Severity, String, String, String)> {
    workbench
        .project(project)
        .unwrap()
        .problems
        .into_iter()
        .map(|p| {
            assert_eq!(p.source, ProblemSource::Config);
            (p.severity, p.path.display().to_string(), p.location, p.message)
        })
        .collect()
}

#[test]
fn a_syntax_error_falls_back_to_all_defaults_with_one_error() {
    let (_fixture, workbench, project) =
        open(FixtureProject::new().file("genea.jsonc", "{\n  \"theme\": \"dark\",\n  \"codeLens\": tru\n}\n"));

    assert_eq!(config(&workbench, project), Config::default());
    let problems = problems(&workbench, project);
    assert_eq!(problems.len(), 1, "{problems:?}");
    let (severity, path, location, message) = &problems[0];
    assert_eq!((*severity, path.as_str(), location.as_str()), (Severity::Error, "genea.jsonc", "3:15"));
    assert!(message.contains("isn't applied"), "{message}");
}

#[test]
fn a_wrong_type_or_value_falls_back_for_that_key_only() {
    let (_fixture, workbench, project) = open(FixtureProject::new().file(
        "genea.jsonc",
        "{\n  \"theme\": \"blue\",\n  \"terminalPosition\": \"bottom\",\n  \"formatOnSave\": \"no\",\n  \"exclude\": \"dist\"\n}\n",
    ));

    assert_eq!(config(&workbench, project), Config { terminal_position: TerminalPosition::Bottom, ..Config::default() });
    assert_eq!(
        problems(&workbench, project),
        [
            (
                Severity::Error,
                "genea.jsonc".into(),
                "2:12".into(),
                r#""theme" must be "system", "light" or "dark". Using the default."#.into()
            ),
            (
                Severity::Error,
                "genea.jsonc".into(),
                "4:19".into(),
                r#""formatOnSave" must be true or false. Using the default."#.into()
            ),
            (
                Severity::Error,
                "genea.jsonc".into(),
                "5:14".into(),
                r#""exclude" must be a list of strings. Using the default."#.into()
            ),
        ]
    );
}

#[test]
fn a_bad_exclude_pattern_falls_back_to_no_excludes() {
    let (_fixture, workbench, project) =
        open(FixtureProject::new().file("genea.jsonc", "{\n  \"exclude\": [\"dist/\", \"src/[z-a].ts\"]\n}\n"));

    assert_eq!(config(&workbench, project).exclude, Vec::<String>::new());
    let problems = problems(&workbench, project);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!((problems[0].0, problems[0].2.as_str()), (Severity::Error, "2:24"));
    assert!(problems[0].3.starts_with("\"exclude\" has a bad pattern"), "{}", problems[0].3);
}

#[test]
fn an_unknown_key_warns_and_the_rest_applies() {
    let (_fixture, workbench, project) =
        open(FixtureProject::new().file("genea.jsonc", "{\n  \"fontSize\": 14,\n  \"theme\": \"light\"\n}\n"));

    assert_eq!(config(&workbench, project), Config { theme: Theme::Light, ..Config::default() });
    assert_eq!(
        problems(&workbench, project),
        [(Severity::Warning, "genea.jsonc".into(), "2:3".into(), r#"Unknown key "fontSize". It is ignored."#.into())]
    );
}

#[test]
fn schema_is_accepted_and_ignored() {
    let (_fixture, workbench, project) = open(
        FixtureProject::new()
            .file("genea.jsonc", r#"{ "$schema": "https://example.com/genea.schema.json", "inlayHints": true }"#),
    );

    assert_eq!(config(&workbench, project), Config { inlay_hints: true, ..Config::default() });
    assert_eq!(problems(&workbench, project), []);
}

#[test]
fn a_config_below_the_root_is_ignored_with_a_warning() {
    let (_fixture, workbench, project) = open(
        FixtureProject::new()
            .file("packages/web/genea.jsonc", r#"{ "theme": "dark" }"#)
            .file("node_modules/some-dep/genea.jsonc", r#"{ "theme": "dark" }"#),
    );

    assert_eq!(config(&workbench, project), Config::default());
    assert_eq!(
        problems(&workbench, project),
        [(
            Severity::Warning,
            "packages/web/genea.jsonc".into(),
            "1:1".into(),
            "Only the config at the project root applies. This genea.jsonc is ignored.".into()
        )]
    );
}
