//! Foreign formatters and linters (ticket #51, ADR 0001): ESLint, Prettier,
//! Biome or dprint config at the project root or a package root turns off
//! format and fix on save, with one warning in Problems and the status-bar
//! item. Oxfmt and Oxlint are played as in `format_on_save.rs`: two fake LSP
//! servers on `node`, told apart by launcher script.

use genea_core::{Command, ProblemItem, ProblemSource, ProjectId, ProjectView, Severity, TextPosition, Workbench};
use genea_testkit::{FakeLsp, FixtureBuilder, FixtureProject, TestHost};

const PACKAGE_JSON: &str = r#"{
  "name": "app",
  "packageManager": "pnpm@12.10.1",
  "devEngines": { "runtime": { "name": "node", "version": "24.21.0" } },
  "devDependencies": { "oxfmt": "^0.72.0", "oxlint": "^1.87.0" }
}
"#;

/// A pnpm workspace with a package in `apps/web`, pinning Node, with Oxlint
/// and Oxfmt installed.
fn oxc_project() -> FixtureBuilder {
    FixtureProject::new()
        .file("package.json", PACKAGE_JSON)
        .file("pnpm-workspace.yaml", "packages:\n  - apps/*\n")
        .file("apps/web/package.json", r#"{ "name": "web" }"#)
        .file("node_modules/oxlint/package.json", r#"{ "name": "oxlint", "version": "1.87.0" }"#)
        .file("node_modules/oxlint/bin/oxlint", "#!/usr/bin/env node\n")
        .file("node_modules/oxfmt/package.json", r#"{ "name": "oxfmt", "version": "0.72.0" }"#)
        .file("node_modules/oxfmt/bin/oxfmt", "#!/usr/bin/env node\n")
        .file("src/main.ts", MAIN_TS)
}

const MAIN_TS: &str = "const  a=1\nvar c = a;\n";
/// `MAIN_TS` formatted and fixed.
const CLEAN_TS: &str = "const a = 1;\nconst c = a;\n";

struct Session {
    fixture: FixtureProject,
    workbench: Workbench,
    project: ProjectId,
}

/// Opens `fixture` with Oxfmt and Oxlint played by fakes that format and
/// fix `MAIN_TS`.
fn open(fixture: FixtureProject) -> Session {
    let host = TestHost::new();
    host.tools().node("24.21.0");
    host.tools().pnpm("12.10.1");
    let oxfmt = FakeLsp::new().formats(&[("const  a=1", "const a = 1;")]);
    let oxlint =
        FakeLsp::new().code_action("source.fixAll.oxc", "fix all safe fixable oxlint issues", "", &[("var c", "const c")]);
    host.processes().script("node", move |spec, io| {
        let script = spec.args.first().map(|arg| arg.to_string_lossy().into_owned()).unwrap_or_default();
        if script.ends_with("oxfmt/bin/oxfmt") { oxfmt.run(io) } else { oxlint.run(io) }
    });
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    Session { fixture, workbench, project }
}

impl Session {
    fn view(&self) -> ProjectView {
        self.workbench.project(self.project).unwrap()
    }

    /// Opens `src/main.ts`, saves it, and returns what was written.
    fn save_main(&mut self) -> String {
        self.workbench.dispatch(self.project, Command::OpenFile("src/main.ts".into()));
        self.workbench.settle().unwrap();
        self.workbench.dispatch(self.project, Command::Save);
        self.workbench.settle().unwrap();
        self.fixture.read("src/main.ts")
    }

    fn foreign_problems(&self) -> Vec<ProblemItem> {
        self.view().problems.into_iter().filter(|p| p.source == ProblemSource::ForeignTools).collect()
    }
}

fn warning(path: &str, message: &str) -> ProblemItem {
    ProblemItem {
        source: ProblemSource::ForeignTools,
        severity: Severity::Warning,
        path: path.into(),
        position: TextPosition::default(),
        location: "1:1".into(),
        message: message.into(),
        stale: false,
    }
}

#[test]
fn without_foreign_config_saving_formats_and_fixes() {
    let mut session = open(oxc_project().build());

    assert_eq!(session.save_main(), CLEAN_TS);
    assert_eq!(session.foreign_problems(), []);
    assert_eq!(session.view().status.foreign_tools, None);
}

#[test]
fn prettier_config_at_the_root_turns_off_format_and_fix_on_save() {
    let fixture = oxc_project().file(".prettierrc", "{}\n").build();
    let mut session = open(fixture);

    assert_eq!(session.save_main(), MAIN_TS);
    assert_eq!(
        session.foreign_problems(),
        [warning(".prettierrc", "Prettier is configured in .prettierrc. Genea doesn't run Prettier, so format and fix on save are off.")]
    );
    assert_eq!(session.view().status.foreign_tools.as_deref(), Some("Reduced mode: Prettier"));
}

#[test]
fn eslint_prettier_biome_and_dprint_config_each_turn_off_format_and_fix_on_save() {
    for (tool, file) in [
        ("ESLint", "eslint.config.js"),
        ("ESLint", ".eslintrc.json"),
        ("Prettier", "prettier.config.mjs"),
        ("Biome", "biome.json"),
        ("Biome", "biome.jsonc"),
        ("dprint", "dprint.json"),
        ("dprint", ".dprint.jsonc"),
    ] {
        let mut session = open(oxc_project().file(file, "{}\n").build());

        assert_eq!(session.save_main(), MAIN_TS, "{file}");
        let message = format!("{tool} is configured in {file}. Genea doesn't run {tool}, so format and fix on save are off.");
        assert_eq!(session.foreign_problems(), [warning(file, &message)]);
        assert_eq!(session.view().status.foreign_tools, Some(format!("Reduced mode: {tool}")), "{file}");
    }
}

#[test]
fn config_at_a_package_root_counts_and_config_elsewhere_does_not() {
    let fixture = oxc_project().file("src/.prettierrc", "{}\n").file("node_modules/x/eslint.config.js", "").build();
    let mut session = open(fixture);
    assert_eq!(session.save_main(), CLEAN_TS, "src/ isn't a package");
    assert_eq!(session.foreign_problems(), []);

    let fixture = oxc_project().file("apps/web/biome.json", "{}\n").build();
    let mut session = open(fixture);
    assert_eq!(session.save_main(), MAIN_TS);
    assert_eq!(
        session.foreign_problems(),
        [warning(
            "apps/web/biome.json",
            "Biome is configured in apps/web/biome.json. Genea doesn't run Biome, so format and fix on save are off."
        )]
    );
}

#[test]
fn a_prettier_key_in_package_json_counts_and_several_tools_share_one_warning() {
    let package_json = PACKAGE_JSON.replace("\"devEngines\"", "\"prettier\": {},\n  \"devEngines\"");
    let fixture = oxc_project().file("package.json", &package_json).file("eslint.config.js", "").build();
    let mut session = open(fixture);

    assert_eq!(session.save_main(), MAIN_TS);
    assert_eq!(
        session.foreign_problems(),
        [warning(
            "eslint.config.js",
            "ESLint and Prettier are configured in eslint.config.js and package.json. \
             Genea doesn't run them, so format and fix on save are off."
        )]
    );
    assert_eq!(session.view().status.foreign_tools.as_deref(), Some("Reduced mode: ESLint, Prettier"));
}

#[test]
fn adding_or_removing_config_while_open_turns_format_and_fix_on_save_off_or_on() {
    let mut session = open(oxc_project().build());

    session.fixture.write("apps/web/.prettierrc", "{}\n");
    session.workbench.settle().unwrap();

    assert_eq!(session.view().status.foreign_tools.as_deref(), Some("Reduced mode: Prettier"));
    assert_eq!(session.save_main(), MAIN_TS);

    session.fixture.remove("apps/web/.prettierrc");
    session.workbench.settle().unwrap();

    assert_eq!(session.foreign_problems(), []);
    assert_eq!(session.view().status.foreign_tools, None);
    assert_eq!(session.save_main(), CLEAN_TS);
}
