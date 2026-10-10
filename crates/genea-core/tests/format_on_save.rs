//! Format and fix on save (ticket #50): saving runs Oxfmt (`oxfmt --lsp`),
//! then Oxlint's safe fixes, then writes the file. Both servers run on the
//! pinned runtime, so the test host plays `node`, and tells the two apart
//! by the launcher script it is started with: one fake LSP server plays
//! Oxfmt and another plays Oxlint.



use std::time::{Duration, Instant};

use genea_core::{Command, FORMAT_TIMEOUT, ProjectId, Workbench};
use genea_testkit::{FakeLsp, FixtureBuilder, FixtureProject, TestHost};


const PACKAGE_JSON: &str = r#"{
  "name": "app",
  "packageManager": "pnpm@12.10.1",
  "devEngines": { "runtime": { "name": "node", "version": "24.21.0" } },
  "devDependencies": { "oxfmt": "^0.72.0", "oxlint": "^1.87.0" }
}
"#;

/// A project pinning Node, with Oxlint and Oxfmt installed.
fn oxc_project() -> FixtureBuilder {
    FixtureProject::new()
        .file("package.json", PACKAGE_JSON)
        .file("node_modules/oxlint/package.json", r#"{ "name": "oxlint", "version": "1.87.0" }"#)
        .file("node_modules/oxlint/bin/oxlint", "#!/usr/bin/env node\n")
        .file("node_modules/oxfmt/package.json", r#"{ "name": "oxfmt", "version": "0.72.0" }"#)
        .file("node_modules/oxfmt/bin/oxfmt", "#!/usr/bin/env node\n")
}

/// Oxfmt's answer for the badly formatted `main.ts` below.
fn formatter() -> FakeLsp {
    FakeLsp::new().formats(&[("const  a=1", "const a = 1;"), ("let b  =  2", "let b = 2;")])
}

/// Oxlint's safe fix for the `var` in `main.ts`.
fn linter() -> FakeLsp {
    FakeLsp::new().code_action("source.fixAll.oxc", "fix all safe fixable oxlint issues", "", &[("var c", "const c")])
}

struct Session {
    fixture: FixtureProject,
    host: TestHost,
    workbench: Workbench,
    project: ProjectId,
}

/// Opens `fixture` with `oxfmt` and `oxlint` playing the two servers.
fn open(fixture: FixtureProject, oxfmt: &FakeLsp, oxlint: &FakeLsp) -> Session {
    let host = TestHost::new();
    host.tools().node("24.21.0");
    host.tools().pnpm("12.10.1");
    let (oxfmt, oxlint) = (oxfmt.clone(), oxlint.clone());
    host.processes().script("node", move |spec, io| {
        let script = spec.args.first().map(|arg| arg.to_string_lossy().into_owned()).unwrap_or_default();
        if script.ends_with("oxfmt/bin/oxfmt") { oxfmt.run(io) } else { oxlint.run(io) }
    });
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    Session { fixture, host, workbench, project }
}

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

    fn open_file(&mut self, path: &str) {
        self.dispatch(Command::OpenFile(path.into()));
        self.settle();
    }

    fn save(&mut self) {
        self.dispatch(Command::Save);
        self.settle();
    }

    fn disk(&self, path: &str) -> String {
        self.fixture.read(path)
    }

    /// Whether the focused editor has unsaved edits.
    fn modified(&self) -> bool {
        self.view().editor.unwrap().modified
    }
}

const MAIN_TS: &str = "const  a=1\nlet b  =  2\nvar c = a + b;\n";

#[test]
fn saving_a_badly_formatted_ts_file_writes_it_formatted_with_lint_fixes_applied() {
    let fixture = oxc_project().file("src/main.ts", MAIN_TS).build();
    let mut session = open(fixture, &formatter(), &linter());
    session.open_file("src/main.ts");

    session.dispatch(Command::InsertText("\n".into()));
    session.save();

    let expected = "\nconst a = 1;\nlet b = 2;\nconst c = a + b;\n";
    assert_eq!(session.disk("src/main.ts"), expected);
    assert!(!session.modified(), "the buffer is what was written");
}

#[test]
fn every_basic_file_type_oxfmt_formats_is_formatted_on_save_and_env_is_not() {
    let files = [
        "src/app.tsx",
        "src/util.js",
        "data.json",
        "tsconfig.jsonc",
        "styles.css",
        "config.yaml",
        "ci.yml",
        "README.md",
        "index.html",
        ".env",
    ];
    let mut fixture = oxc_project();
    for file in files {
        fixture = fixture.file(file, "a  b\n");
    }
    let oxfmt = FakeLsp::new().formats(&[("a  b", "a b")]);
    let mut session = open(fixture.build(), &oxfmt, &FakeLsp::new());

    for file in files {
        session.open_file(file);
        session.save();
    }

    for file in files {
        let expected = if file == ".env" { "a  b\n" } else { "a b\n" };
        assert_eq!(session.disk(file), expected, "{file}");
    }
}

/// Saves `main.ts` in a project with this `genea.jsonc`, and returns what
/// was written.
fn save_with_config(config: &str) -> String {
    let fixture = oxc_project().file("genea.jsonc", config).file("src/main.ts", MAIN_TS).build();
    let mut session = open(fixture, &formatter(), &linter());
    session.open_file("src/main.ts");
    session.save();
    session.disk("src/main.ts")
}

#[test]
fn turning_format_on_save_off_skips_formatting_but_still_fixes() {
    let written = save_with_config(r#"{ "formatOnSave": false }"#);

    assert_eq!(written, "const  a=1\nlet b  =  2\nconst c = a + b;\n");
}

#[test]
fn turning_fix_on_save_off_skips_the_lint_fixes_but_still_formats() {
    let written = save_with_config(r#"{ "fixOnSave": false }"#);

    assert_eq!(written, "const a = 1;\nlet b = 2;\nvar c = a + b;\n");
}

#[test]
fn turning_both_off_writes_the_file_as_it_is() {
    let written = save_with_config(r#"{ "formatOnSave": false, "fixOnSave": false }"#);

    assert_eq!(written, MAIN_TS);
}

// --- A server that doesn't answer -------------------------------------------------

impl Session {
    /// Saves, then lets the format timeout pass on the host clock.
    fn save_and_time_out(&mut self) {
        self.dispatch(Command::Save);
        self.host.clock().advance(FORMAT_TIMEOUT);
        self.settle();
    }

    fn notices(&self) -> Vec<String> {
        self.view().notices.into_iter().map(|notice| notice.message).collect()
    }
}

#[test]
fn a_formatter_that_never_answers_leads_to_an_unformatted_save_and_a_notice() {
    let fixture = oxc_project().file("src/main.ts", MAIN_TS).build();
    let oxfmt = formatter().hang_on("textDocument/formatting");
    let mut session = open(fixture, &oxfmt, &linter());
    session.open_file("src/main.ts");

    session.dispatch(Command::InsertText("// one\n".into()));
    session.save_and_time_out();

    assert_eq!(session.disk("src/main.ts"), format!("// one\n{MAIN_TS}"));
    assert!(!session.modified());
    assert!(
        session.notices().contains(&"Saved src/main.ts unformatted: Oxfmt didn't answer within a second.".into()),
        "{:?}",
        session.notices()
    );
}

#[test]
fn a_linter_that_never_answers_leads_to_a_save_without_lint_fixes_and_a_notice() {
    let fixture = oxc_project().file("src/main.ts", MAIN_TS).build();
    let oxlint = linter().hang_on("textDocument/codeAction");
    let mut session = open(fixture, &formatter(), &oxlint);
    session.open_file("src/main.ts");

    session.dispatch(Command::Save);
    // Oxfmt answers first; the clock moves once Oxlint has been asked.
    session.pump_until(|| asked_for_fixes(&oxlint));
    session.host.clock().advance(FORMAT_TIMEOUT);
    session.settle();

    assert_eq!(session.disk("src/main.ts"), "const a = 1;\nlet b = 2;\nvar c = a + b;\n");
    assert!(session.notices().contains(&"Saved src/main.ts without lint fixes: Oxlint didn't answer within a second.".into()));
}

/// Whether Oxlint has been asked for its fixes on save.
fn asked_for_fixes(oxlint: &FakeLsp) -> bool {
    let asked = oxlint.received("textDocument/codeAction");
    asked.iter().any(|params| params["context"]["only"] == serde_json::json!(["source.fixAll.oxc"]))
}

impl Session {
    /// Applies background results as they come until `done` (`settle`
    /// would wait for the server that never answers); fails after 10 s.
    fn pump_until(&mut self, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "timed out");
            self.workbench.pump();
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[test]
fn a_save_whose_servers_answer_in_time_has_no_notice_when_the_timeout_passes() {
    let fixture = oxc_project().file("src/main.ts", MAIN_TS).build();
    let mut session = open(fixture, &formatter(), &linter());
    session.open_file("src/main.ts");

    session.save();
    session.host.clock().advance(FORMAT_TIMEOUT);
    session.settle();

    assert_eq!(session.disk("src/main.ts"), "const a = 1;\nlet b = 2;\nconst c = a + b;\n");
    let saved: Vec<String> = session.notices().into_iter().filter(|notice| notice.starts_with("Saved")).collect();
    assert_eq!(saved, Vec::<String>::new());
}

// --- Reformat file (⌥⌘L) ------------------------------------------------------------

impl Session {
    /// The focused editor's text.
    fn text(&self) -> String {
        let editor = self.view().editor.unwrap();
        editor.lines.iter().map(|line| format!("{}\n", line.text)).collect::<String>()
    }
}

#[test]
fn reformat_file_formats_the_buffer_without_saving_it_even_with_format_on_save_off() {
    let fixture =
        oxc_project().file("genea.jsonc", r#"{ "formatOnSave": false }"#).file("src/main.ts", "const  a=1").build();
    let oxfmt = FakeLsp::new().formats(&[("const  a=1", "const a = 1;")]);
    let mut session = open(fixture, &oxfmt, &linter());
    session.open_file("src/main.ts");

    session.dispatch(Command::ReformatFile);
    session.settle();

    assert_eq!(session.text(), "const a = 1;\n");
    assert!(session.modified());
    assert_eq!(session.disk("src/main.ts"), "const  a=1");

    session.dispatch(Command::Undo);
    assert_eq!(session.text(), "const  a=1\n", "formatting is one undo step");
}
