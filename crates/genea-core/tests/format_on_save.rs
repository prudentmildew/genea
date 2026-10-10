//! Format and fix on save (ticket #50): saving runs Oxfmt (`oxfmt --lsp`),
//! then Oxlint's safe fixes, then writes the file. Both servers run on the
//! pinned runtime, so the test host plays `node`, and tells the two apart
//! by the launcher script it is started with: one fake LSP server plays
//! Oxfmt and another plays Oxlint.



use genea_core::{Command, ProjectId, Workbench};
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
